//! The browser view: the single per-workspace pane item that owns browser
//! tabs and their chrome (ADR-0004). M2 scope (ticket #9): full browser-tab
//! management inside the view — an internal tab strip, open/close/switch,
//! pin/unpin with pinned-first ordering, reopen-closed-tab, next/previous
//! tab, per-tab favicons and titles. Switching browser tabs swaps the
//! presented engine surface: each tab owns its backend and presenter
//! (`browser_tab.rs`).
//!
//! Everything here sits above the two seams: engine communication flows
//! through the tab-backend trait and frame presentation through the frame
//! presenter, so this file is platform-neutral and testable with a scripted
//! stub backend.

use crate::browser_tab::BrowserTab;
use crate::omnibox::{Omnibox, OmniboxEvent};
use crate::tab_backend::TabBackend;
use gpui::{
    AnyElement, App, Bounds, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable,
    KeyDownEvent, KeyUpEvent, Keystroke, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Render, ScrollWheelEvent, SharedString, Window, actions, canvas, div, img,
};
use ui::{CommonAnimationExt, IconButtonShape, Tab, TabPosition, Tooltip, prelude::*};
#[cfg(any(feature = "cef", test))]
use workspace::Workspace;
use workspace::item::{Item, ItemEvent, TabContentParams, TabTooltipContent};

actions!(
    browser,
    [
        /// Opens the browser view in the workspace.
        OpenBrowser,
        /// Navigates back in the browser history.
        GoBack,
        /// Navigates forward in the browser history.
        GoForward,
        /// Reloads the current page.
        Reload,
        /// Focuses the browser's URL and search entry.
        FocusOmnibox,
        /// Opens a new browser tab.
        NewTab,
        /// Closes the active browser tab.
        CloseTab,
        /// Reopens the most recently closed browser tab.
        ReopenClosedTab,
        /// Activates the next browser tab.
        NextTab,
        /// Activates the previous browser tab.
        PreviousTab,
        /// Pins the active browser tab.
        PinTab,
        /// Unpins the active browser tab.
        UnpinTab,
    ]
);

pub const DEFAULT_URL: &str = "https://zed.dev";

/// How many closed browser tabs the reopen stack remembers
/// (`Glass:crates/browser/src/browser_view.rs:41`).
const MAX_CLOSED_TABS: usize = 20;

/// Creates the engine backend for each new browser tab.
pub(crate) type TabBackendFactory = Box<dyn Fn() -> Box<dyn TabBackend>>;

#[cfg(feature = "cef")]
pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _window, _cx| {
        workspace.register_action(|workspace, _: &OpenBrowser, window, cx| {
            BrowserView::open(workspace, window, cx);
        });
    })
    .detach();
}

pub struct BrowserView {
    focus_handle: FocusHandle,
    backend_factory: TabBackendFactory,
    /// All browser tabs, pinned tabs first. Never empty: closing the last tab
    /// replaces it with a fresh one, so `active_tab_index` always indexes a
    /// live tab.
    tabs: Vec<BrowserTab>,
    active_tab_index: usize,
    /// Recently closed tabs, most recent last (the reopen stack).
    closed_tabs: Vec<crate::browser_tab::ClosedTab>,
    next_tab_id: usize,
    omnibox: Entity<Omnibox>,
    /// Window-relative bounds of the page content area, captured at draw time;
    /// pointer events are translated into content-relative coordinates with
    /// its origin before crossing the tab-backend seam.
    content_bounds: Bounds<Pixels>,
}

/// M1 subset of Glass's three-way key dispatch
/// (`Glass:crates/browser/src/text_input.rs:61`): app-first classification
/// only — ctrl/platform-modified chords belong to Zed even when no binding
/// matched them, so pages cannot shadow app shortcuts. The text-input route
/// (editable-field state + IME) joins with ticket #16.
fn is_app_keystroke(keystroke: &Keystroke) -> bool {
    keystroke.modifiers.platform || keystroke.modifiers.control
}

impl BrowserView {
    pub fn new(
        backend_factory: TabBackendFactory,
        initial_url: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let weak = cx.weak_entity();
        crate::observe_pumps(cx, move |cx| {
            weak.update(cx, |view, cx| view.drain_engine_events(cx))
                .is_ok()
        });

        let window_handle = window.window_handle();
        cx.on_release(move |this, cx| {
            // The window may already be gone during app teardown; the sprite
            // atlas dies with it, so a failed update needs no handling.
            window_handle
                .update(cx, |_, window, _| {
                    for tab in &mut this.tabs {
                        tab.release_presenter(window);
                    }
                })
                .ok();
            // Engine shutdown must not depend on the window still existing.
            for tab in &mut this.tabs {
                tab.close();
            }
        })
        .detach();

        let omnibox = cx.new(|cx| Omnibox::new(window, cx));
        cx.subscribe_in(&omnibox, window, |this, _, event, window, cx| {
            let OmniboxEvent::Navigate(url) = event;
            this.navigate_to(url.clone(), window, cx);
        })
        .detach();

        let mut this = Self {
            focus_handle: cx.focus_handle(),
            backend_factory,
            tabs: Vec::new(),
            active_tab_index: 0,
            closed_tabs: Vec::new(),
            next_tab_id: 0,
            omnibox,
            content_bounds: Bounds::default(),
        };
        let tab = this.create_tab(initial_url);
        this.tabs.push(tab);
        this
    }

    #[cfg(feature = "cef")]
    pub fn open(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
        Self::open_with_factory(workspace, window, cx, || {
            Box::new(crate::tab::CefTab::new())
        });
    }

    #[cfg(any(feature = "cef", test))]
    fn open_with_factory(
        workspace: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Workspace>,
        backend_factory: impl Fn() -> Box<dyn TabBackend> + 'static,
    ) {
        // One browser view per workspace (ADR-0004); reopening focuses it.
        let existing = workspace.items_of_type::<BrowserView>(cx).next();
        if let Some(existing) = existing {
            workspace.activate_item(&existing, true, true, window, cx);
            return;
        }
        let view = cx.new(|cx| {
            BrowserView::new(
                Box::new(backend_factory),
                DEFAULT_URL.to_string(),
                window,
                cx,
            )
        });
        workspace.add_item_to_active_pane(Box::new(view), None, true, window, cx);
    }

    fn allocate_tab_id(&mut self) -> usize {
        util::post_inc(&mut self.next_tab_id)
    }

    fn create_tab(&mut self, url: String) -> BrowserTab {
        let id = self.allocate_tab_id();
        BrowserTab::new(id, (self.backend_factory)(), url)
    }

    // `tabs` is never empty and `active_tab_index` is kept in bounds by every
    // tab-list mutation, so these lookups cannot fail.
    fn active_tab(&self) -> &BrowserTab {
        &self.tabs[self.active_tab_index]
    }

    fn active_tab_mut(&mut self) -> &mut BrowserTab {
        &mut self.tabs[self.active_tab_index]
    }

    pub fn url(&self) -> &str {
        self.active_tab().url()
    }

    pub fn title(&self) -> &str {
        self.active_tab().title()
    }

    pub fn is_loading(&self) -> bool {
        self.active_tab().is_loading()
    }

    pub fn can_go_back(&self) -> bool {
        self.active_tab().can_go_back()
    }

    pub fn can_go_forward(&self) -> bool {
        self.active_tab().can_go_forward()
    }

    /// Drain pending engine events into per-tab state. Invoked after every
    /// message-pump iteration; tests reach it by scripting stub events and
    /// calling `crate::simulate_message_pump`.
    fn drain_engine_events(&mut self, cx: &mut Context<Self>) {
        // The engine may have just become ready (first pumps after init);
        // kick a render so the waiting active tab gets created with real
        // bounds. Inactive tabs stay engine-less until activated (draws only
        // sync the active tab's viewport), so they must not keep requesting
        // renders that cannot start them.
        let mut needs_notify = self.active_tab().wants_start();

        let mut active_identity_changed = false;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            let changes = tab.drain_events();
            if changes.identity_changed || changes.needs_notify {
                // Any tab's title or favicon shows in the tab strip.
                needs_notify = true;
            }
            if changes.identity_changed && index == self.active_tab_index {
                active_identity_changed = true;
            }
        }

        if active_identity_changed {
            cx.emit(ItemEvent::UpdateTab);
        }
        if needs_notify {
            cx.notify();
        }
    }

    /// Record the content bounds and push the viewport into the active tab's
    /// engine. Called from the content canvas during every draw of this view,
    /// so it always sees the laid-out bounds — including the final frame of a
    /// resize and the first draw after a tab switch.
    fn handle_content_bounds(&mut self, bounds: Bounds<Pixels>, scale_factor: f32) {
        self.content_bounds = bounds;
        let width = f32::from(bounds.size.width) as u32;
        let height = f32::from(bounds.size.height) as u32;
        self.active_tab_mut()
            .sync_viewport(width, height, scale_factor);
    }

    /// Navigate the active tab to `url` (already heuristic-resolved) and hand
    /// focus back to the page.
    fn navigate_to(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        self.active_tab_mut().navigate(url);
        window.focus(&self.focus_handle, cx);
        self.active_tab_mut().set_focus(true);
        cx.emit(ItemEvent::UpdateTab);
        cx.notify();
    }

    fn go_back(&mut self, cx: &mut Context<Self>) {
        self.active_tab_mut().go_back();
        cx.notify();
    }

    fn go_forward(&mut self, cx: &mut Context<Self>) {
        self.active_tab_mut().go_forward();
        cx.notify();
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.active_tab_mut().reload();
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        self.active_tab_mut().stop();
        cx.notify();
    }

    fn focus_omnibox(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.omnibox.update(cx, |omnibox, cx| {
            omnibox.focus_and_select_all(window, cx);
        });
    }

    /// Switch the presented engine surface to the tab at `index`: the old tab
    /// is blurred and hidden (its engine stops painting; its last frame stays
    /// with its presenter), the new tab is shown and focused. Its engine
    /// browser is created on the next draw if it does not exist yet.
    fn activate_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() || index == self.active_tab_index {
            return;
        }

        let previous = self.active_tab_mut();
        previous.set_focus(false);
        previous.set_hidden(true);

        self.active_tab_index = index;
        let tab = self.active_tab_mut();
        tab.set_hidden(false);
        tab.set_focus(true);
        window.focus(&self.focus_handle, cx);

        cx.emit(ItemEvent::UpdateTab);
        cx.notify();
    }

    fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self.create_tab(DEFAULT_URL.to_string());
        self.tabs.push(tab);
        self.activate_tab(self.tabs.len() - 1, window, cx);
        // A fresh tab has no meaningful page yet; the user's next step is
        // typing a destination. The new-tab page arrives with ticket #10.
        self.focus_omnibox(window, cx);
    }

    fn close_tab_at(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        let was_active = index == self.active_tab_index;
        let mut tab = self.tabs.remove(index);
        self.remember_closed_tab(&tab);
        tab.release_presenter(window);
        tab.close();

        if self.tabs.is_empty() {
            // The view always shows at least one tab; closing the last one
            // resets to a fresh tab, like Glass
            // (`Glass:crates/browser/src/browser_view/tabs.rs:455`).
            let tab = self.create_tab(DEFAULT_URL.to_string());
            self.tabs.push(tab);
            self.active_tab_index = 0;
            self.focus_omnibox(window, cx);
        } else if index < self.active_tab_index {
            self.active_tab_index -= 1;
        } else if was_active {
            self.active_tab_index = self.active_tab_index.min(self.tabs.len() - 1);
            let tab = self.active_tab_mut();
            tab.set_hidden(false);
            tab.set_focus(true);
        }

        cx.emit(ItemEvent::UpdateTab);
        cx.notify();
    }

    fn remember_closed_tab(&mut self, tab: &BrowserTab) {
        if tab.url().is_empty() {
            return;
        }
        self.closed_tabs.push(tab.to_closed_tab());
        if self.closed_tabs.len() > MAX_CLOSED_TABS {
            self.closed_tabs.remove(0);
        }
    }

    fn reopen_closed_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(closed) = self.closed_tabs.pop() else {
            return;
        };
        let id = self.allocate_tab_id();
        let tab = BrowserTab::restore(id, (self.backend_factory)(), closed);
        self.tabs.push(tab);
        self.activate_tab(self.tabs.len() - 1, window, cx);
        self.resort_tabs_pinned_first(cx);
    }

    fn activate_next_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.len() <= 1 {
            return;
        }
        let index = (self.active_tab_index + 1) % self.tabs.len();
        self.activate_tab(index, window, cx);
    }

    fn activate_previous_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.len() <= 1 {
            return;
        }
        let index = if self.active_tab_index == 0 {
            self.tabs.len() - 1
        } else {
            self.active_tab_index - 1
        };
        self.activate_tab(index, window, cx);
    }

    fn pin_tab_at(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get_mut(index) {
            tab.set_pinned(true);
            self.resort_tabs_pinned_first(cx);
        }
    }

    fn unpin_tab_at(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get_mut(index) {
            tab.set_pinned(false);
            self.resort_tabs_pinned_first(cx);
        }
    }

    /// Restore pinned-first ordering after a pin state change, keeping the
    /// relative order within each group (stable sort) and the active tab
    /// active.
    fn resort_tabs_pinned_first(&mut self, cx: &mut Context<Self>) {
        let active_id = self.active_tab().id;
        self.tabs.sort_by_key(|tab| !tab.is_pinned());
        if let Some(index) = self.tabs.iter().position(|tab| tab.id == active_id) {
            self.active_tab_index = index;
        }
        cx.notify();
    }

    fn handle_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Clicking the page focuses both the pane item (so browser-routed
        // keystrokes dispatch here) and the engine browser (so the page shows
        // carets and selection).
        window.focus(&self.focus_handle, cx);
        let position = event.position - self.content_bounds.origin;
        let tab = self.active_tab_mut();
        tab.set_focus(true);
        tab.send_mouse_down(position, event.button, event.click_count, event.modifiers);
    }

    fn handle_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let position = event.position - self.content_bounds.origin;
        self.active_tab_mut()
            .send_mouse_up(position, event.button, event.modifiers);
    }

    fn handle_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let position = event.position - self.content_bounds.origin;
        self.active_tab_mut()
            .send_mouse_move(position, event.pressed_button, event.modifiers);
    }

    fn handle_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let position = event.position - self.content_bounds.origin;
        self.active_tab_mut()
            .send_scroll_wheel(position, event.delta, event.modifiers);
    }

    /// Key listeners run only for keystrokes no Zed binding consumed (GPUI
    /// matches bindings before key listeners), so anything arriving here is
    /// either page input or an unbound app chord.
    ///
    /// Unlike Glass, the engine send is not deferred: Glass's tab was a GPUI
    /// entity it could not update re-entrantly mid-dispatch, while this
    /// backend is plain owned state and the engine's synchronous callbacks
    /// touch only atomics and channels.
    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Key listeners fire along the whole focus path, so a keystroke aimed
        // at the omnibox editor also reaches this ancestor; only forward to
        // the page when the page content itself has focus.
        if !self.focus_handle.is_focused(window) || is_app_keystroke(&event.keystroke) {
            return;
        }
        self.active_tab_mut()
            .send_key_down(&event.keystroke, event.is_held);
        cx.stop_propagation();
    }

    fn handle_key_up(&mut self, event: &KeyUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus_handle.is_focused(window) || is_app_keystroke(&event.keystroke) {
            return;
        }
        self.active_tab_mut().send_key_up(&event.keystroke);
        cx.stop_propagation();
    }

    fn render_placeholder(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let message: SharedString = match self.active_tab().engine_error() {
            Some(error) => format!("Failed to start the browser engine: {error}").into(),
            None => "Loading…".into(),
        };
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .text_color(cx.theme().colors().text_muted)
                    .child(message),
            )
    }

    /// Whether `tab_content` renders its own identity glyph (spinner or
    /// favicon) in place of the pane's `tab_icon` slot.
    fn shows_custom_tab_glyph(&self) -> bool {
        self.active_tab().is_loading() || self.active_tab().favicon_url().is_some()
    }

    fn globe_icon() -> AnyElement {
        Icon::new(IconName::ToolWeb)
            .size(IconSize::Small)
            .color(Color::Muted)
            .into_any_element()
    }

    /// A browser tab's identity glyph: a spinner while loading, else the
    /// favicon, else a generic globe. Shared by the chrome, the tab strip,
    /// and the pane tab.
    fn favicon_element(tab: &BrowserTab) -> AnyElement {
        if tab.is_loading() {
            Icon::new(IconName::ArrowCircle)
                .size(IconSize::Small)
                .color(Color::Muted)
                .with_rotate_animation(2)
                .into_any_element()
        } else if let Some(favicon_url) = tab.favicon_url() {
            img(favicon_url)
                .size_4()
                .with_fallback(Self::globe_icon)
                .into_any_element()
        } else {
            Self::globe_icon()
        }
    }

    fn render_tab_strip_tab(
        &self,
        index: usize,
        tab: &BrowserTab,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_active = index == self.active_tab_index;
        let is_pinned = tab.is_pinned();
        let url: SharedString = tab.url().to_string().into();

        let end_slot = if is_pinned {
            IconButton::new(("unpin-browser-tab", tab.id), IconName::Pin)
                .shape(IconButtonShape::Square)
                .icon_color(Color::Muted)
                .size(ButtonSize::None)
                .icon_size(IconSize::Small)
                .tooltip(Tooltip::text("Unpin Tab"))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.unpin_tab_at(index, cx);
                }))
        } else {
            IconButton::new(("close-browser-tab", tab.id), IconName::Close)
                .shape(IconButtonShape::Square)
                .icon_color(Color::Muted)
                .size(ButtonSize::None)
                .icon_size(IconSize::Small)
                .tooltip(Tooltip::text("Close Tab"))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.close_tab_at(index, window, cx);
                }))
        };

        Tab::new(("browser-tab", tab.id))
            .toggle_state(is_active)
            .position(if index == 0 {
                TabPosition::First
            } else if index == self.tabs.len() - 1 {
                TabPosition::Last
            } else {
                TabPosition::Middle(index.cmp(&self.active_tab_index))
            })
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.activate_tab(index, window, cx);
            }))
            .on_aux_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if event.is_middle_click() && !is_pinned {
                    this.close_tab_at(index, window, cx);
                    cx.stop_propagation();
                }
            }))
            .when(!url.is_empty(), |this| this.tooltip(Tooltip::text(url)))
            .end_slot(end_slot)
            .child(
                h_flex()
                    .gap_1()
                    .child(Self::favicon_element(tab))
                    // Pinned tabs are compact: favicon and pin glyph only.
                    .when(!is_pinned, |this| {
                        this.child(
                            div().max_w_40().child(
                                Label::new(tab.display_title("New Tab"))
                                    .size(LabelSize::Small)
                                    .truncate(),
                            ),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_tab_strip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = self
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| self.render_tab_strip_tab(index, tab, cx))
            .collect::<Vec<_>>();

        h_flex()
            .w_full()
            .flex_none()
            .bg(cx.theme().colors().tab_bar_background)
            .child(
                h_flex()
                    .id("browser-tab-strip")
                    .flex_1()
                    .h(Tab::container_height(cx))
                    .overflow_x_scroll()
                    .children(tabs),
            )
            .child(
                h_flex()
                    .flex_none()
                    .h(Tab::container_height(cx))
                    .px_1()
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .child(
                        IconButton::new("browser-new-tab", IconName::Plus)
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text("New Tab"))
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.new_tab(window, cx);
                            })),
                    ),
            )
    }

    fn render_chrome(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active_tab = self.active_tab();
        let title = active_tab.title().trim().to_string();
        let is_loading = active_tab.is_loading();
        h_flex()
            .w_full()
            .flex_none()
            .gap_1()
            .px_1p5()
            .py_1()
            .border_b_1()
            .border_color(cx.theme().colors().border)
            .bg(cx.theme().colors().tab_bar_background)
            .child(
                IconButton::new("browser-back", IconName::ArrowLeft)
                    .icon_size(IconSize::Small)
                    .disabled(!active_tab.can_go_back())
                    .tooltip(Tooltip::text("Go Back"))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.go_back(cx))),
            )
            .child(
                IconButton::new("browser-forward", IconName::ArrowRight)
                    .icon_size(IconSize::Small)
                    .disabled(!active_tab.can_go_forward())
                    .tooltip(Tooltip::text("Go Forward"))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.go_forward(cx))),
            )
            .child(if is_loading {
                IconButton::new("browser-stop", IconName::Close)
                    .icon_size(IconSize::Small)
                    .tooltip(Tooltip::text("Stop Loading"))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.stop(cx)))
            } else {
                IconButton::new("browser-reload", IconName::RotateCw)
                    .icon_size(IconSize::Small)
                    .tooltip(Tooltip::text("Reload"))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.reload(cx)))
            })
            .child(
                div()
                    .flex_none()
                    .px_0p5()
                    .child(Self::favicon_element(active_tab)),
            )
            .child(self.omnibox.clone())
            .when(!title.is_empty(), |this| {
                this.child(div().flex_none().max_w_64().child(
                    Label::new(title).size(LabelSize::Small).color(Color::Muted).truncate(),
                ))
            })
    }
}

impl Render for BrowserView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let frame = {
            let tab = self.active_tab_mut();
            tab.present_pending_frame();
            tab.render_frame(window)
        };
        let has_frame = frame.is_some();

        // The omnibox editor mirror is synced here rather than where the URL
        // changes because `drain_engine_events` runs from the pump observer,
        // which has no `Window` (required to set editor text).
        let url = self.active_tab().url().to_string();
        self.omnibox.update(cx, |omnibox, cx| {
            omnibox.set_current_url(&url, window, cx);
        });

        let this = cx.entity();
        let bounds_tracker = canvas(
            move |bounds, window, cx| {
                let scale_factor = window.scale_factor();
                this.update(cx, |view, _| {
                    view.handle_content_bounds(bounds, scale_factor)
                });
                bounds
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();

        let content = div()
            .id("browser-content")
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .bg(cx.theme().colors().editor_background)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::handle_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::handle_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::handle_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::handle_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::handle_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::handle_mouse_up))
            .on_mouse_move(cx.listener(Self::handle_mouse_move))
            .on_scroll_wheel(cx.listener(Self::handle_scroll_wheel))
            .child(bounds_tracker)
            .when_some(frame, |this, frame| this.child(frame))
            .when(!has_frame, |this| this.child(self.render_placeholder(cx)));

        div()
            .id("browser-view")
            .key_context("BrowserView")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .on_action(cx.listener(|this, _: &GoBack, _, cx| this.go_back(cx)))
            .on_action(cx.listener(|this, _: &GoForward, _, cx| this.go_forward(cx)))
            .on_action(cx.listener(|this, _: &Reload, _, cx| this.reload(cx)))
            .on_action(
                cx.listener(|this, _: &FocusOmnibox, window, cx| this.focus_omnibox(window, cx)),
            )
            .on_action(cx.listener(|this, _: &NewTab, window, cx| this.new_tab(window, cx)))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                this.close_tab_at(this.active_tab_index, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ReopenClosedTab, window, cx| {
                this.reopen_closed_tab(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &NextTab, window, cx| this.activate_next_tab(window, cx)),
            )
            .on_action(cx.listener(|this, _: &PreviousTab, window, cx| {
                this.activate_previous_tab(window, cx)
            }))
            .on_action(cx.listener(|this, _: &PinTab, _, cx| {
                this.pin_tab_at(this.active_tab_index, cx)
            }))
            .on_action(cx.listener(|this, _: &UnpinTab, _, cx| {
                this.unpin_tab_at(this.active_tab_index, cx)
            }))
            .on_key_down(cx.listener(Self::handle_key_down))
            .on_key_up(cx.listener(Self::handle_key_up))
            .child(self.render_tab_strip(cx))
            .child(self.render_chrome(cx))
            .child(content)
    }
}

impl Focusable for BrowserView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<ItemEvent> for BrowserView {}

impl Item for BrowserView {
    type Event = ItemEvent;

    fn to_item_events(event: &Self::Event, f: &mut dyn FnMut(ItemEvent)) {
        f(*event);
    }

    fn tab_content(&self, params: TabContentParams, _window: &Window, cx: &App) -> AnyElement {
        let text = self.tab_content_text(params.detail.unwrap_or_default(), cx);
        let label = Label::new(text).color(params.text_color());
        if self.shows_custom_tab_glyph() {
            h_flex()
                .gap_1()
                .child(Self::favicon_element(self.active_tab()))
                .child(label)
                .into_any_element()
        } else {
            label.into_any_element()
        }
    }

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        self.active_tab().display_title("Browser")
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        // The pane renders this icon slot in addition to `tab_content`, so
        // yield it whenever `tab_content` is already showing a spinner or
        // favicon. ToolWeb is upstream's globe glyph; the icon set has no
        // dedicated Globe variant.
        if self.shows_custom_tab_glyph() {
            None
        } else {
            Some(Icon::new(IconName::ToolWeb))
        }
    }

    fn tab_tooltip_content(&self, _cx: &App) -> Option<TabTooltipContent> {
        let url = self.active_tab().url();
        if url.is_empty() {
            None
        } else {
            Some(TabTooltipContent::Text(url.to_string().into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stub_tab_backend::{
        RecordedCommand, StubBackendFactory, StubTabController,
    };
    use crate::tab_backend::{SoftwareFrame, TabBackendEvent};
    use gpui::{
        Modifiers, ScrollDelta, TestAppContext, TouchPhase, VisualTestContext, point, size,
    };
    use project::Project;
    use std::sync::Arc;
    use workspace::AppState;

    fn init_test(cx: &mut TestAppContext) -> Arc<AppState> {
        cx.update(|cx| {
            let app_state = AppState::test(cx);
            editor::init(cx);
            app_state
        })
    }

    fn backend_factory(factory: &StubBackendFactory) -> TabBackendFactory {
        let factory = factory.clone();
        Box::new(move || factory.create_backend())
    }

    /// A browser view with a stub-backed tab, opened in a bare test window.
    fn stub_view<'a>(
        engine_ready: bool,
        initial_url: &str,
        cx: &'a mut TestAppContext,
    ) -> (
        Entity<BrowserView>,
        &'a mut VisualTestContext,
        StubBackendFactory,
    ) {
        let factory = StubBackendFactory::new(engine_ready);
        let initial_url = initial_url.to_string();
        let (view, cx) = cx.add_window_view(|window, cx| {
            BrowserView::new(backend_factory(&factory), initial_url, window, cx)
        });
        cx.run_until_parked();
        (view, cx, factory)
    }

    /// One simulated message-pump iteration followed by settling: scripted
    /// stub events drain into views through the same post-pump path real
    /// engine events take.
    fn pump(cx: &mut VisualTestContext) {
        cx.update(|_, cx| crate::simulate_message_pump(cx));
        cx.run_until_parked();
    }

    fn viewport_count(controller: &StubTabController) -> usize {
        controller
            .commands()
            .iter()
            .filter(|command| matches!(command, RecordedCommand::SetViewport { .. }))
            .count()
    }

    /// The tab ids in strip order plus the active tab's id.
    fn tab_order_and_active(view: &Entity<BrowserView>, cx: &mut VisualTestContext) -> (Vec<usize>, usize) {
        view.update(cx, |view, _| {
            (
                view.tabs.iter().map(|tab| tab.id).collect(),
                view.active_tab().id,
            )
        })
    }

    #[gpui::test]
    async fn test_engine_events_update_item_state(cx: &mut TestAppContext) {
        init_test(cx);
        let (view, cx, factory) = stub_view(true, "https://example.com", cx);
        let controller = factory.controller(0);

        // The first render starts the engine tab with the initial URL and a
        // real viewport.
        let window_scale = cx.update(|window, _| window.scale_factor());
        assert_eq!(
            controller.started_with().as_deref(),
            Some("https://example.com")
        );
        let (width, height, scale_factor) = controller.last_viewport().unwrap();
        assert!(width > 0 && height > 0);
        assert_eq!(scale_factor, window_scale);
        assert_eq!(controller.focus_calls(), vec![true]);

        // A page load plays out end to end: scripted engine events drain
        // through the post-pump path into the item's state.
        controller.script_events([
            TabBackendEvent::AddressChanged("https://example.com/docs".into()),
            TabBackendEvent::TitleChanged("Example Docs".into()),
            TabBackendEvent::LoadingStateChanged {
                is_loading: true,
                can_go_back: false,
                can_go_forward: false,
            },
        ]);
        pump(cx);

        view.update_in(cx, |view, window, cx| {
            assert_eq!(view.url(), "https://example.com/docs");
            assert_eq!(view.title(), "Example Docs");
            assert!(view.is_loading());
            assert!(
                view.tab_icon(window, cx).is_none(),
                "while loading, the spinner replaces the pane tab's icon slot"
            );
        });

        controller.script_events([TabBackendEvent::LoadingStateChanged {
            is_loading: false,
            can_go_back: true,
            can_go_forward: false,
        }]);
        pump(cx);

        view.update(cx, |view, cx| {
            assert!(!view.is_loading());
            assert!(view.can_go_back());
            assert!(!view.can_go_forward());
            assert_eq!(
                view.tab_content_text(0, cx).as_ref(),
                "Example Docs",
                "pane tab shows the page title"
            );
        });
    }

    #[gpui::test]
    async fn test_frames_flow_through_the_presenter_seam(cx: &mut TestAppContext) {
        init_test(cx);
        let (view, cx, factory) = stub_view(true, DEFAULT_URL, cx);
        let controller = factory.controller(0);
        assert!(!view.update(cx, |view, _| view.active_tab().has_frame()));

        let (width, height) = (4, 4);
        controller.script_frame(SoftwareFrame {
            width,
            height,
            bgra: vec![0xff; (width * height * 4) as usize],
        });
        pump(cx);

        assert!(
            view.update(cx, |view, _| view.active_tab().has_frame()),
            "presenter should hold the frame after the FrameReady render"
        );
        assert!(
            !controller.has_staged_frame(),
            "render should have taken the paint output from the backend"
        );
    }

    #[gpui::test]
    async fn test_resize_propagates_scaled_viewport(cx: &mut TestAppContext) {
        init_test(cx);
        let (_view, cx, factory) = stub_view(true, DEFAULT_URL, cx);
        let controller = factory.controller(0);
        let viewports_before = viewport_count(&controller);

        cx.simulate_resize(size(px(500.), px(400.)));
        cx.run_until_parked();

        let window_scale = cx.update(|window, _| window.scale_factor());
        assert!(viewport_count(&controller) > viewports_before);
        let (width, height, scale_factor) = controller.last_viewport().unwrap();
        assert_eq!(
            width, 500,
            "engine viewport tracks the resized logical bounds"
        );
        assert!(
            height > 0 && height < 400,
            "engine viewport height excludes the navigation chrome, got {height}"
        );
        assert_eq!(scale_factor, window_scale);
    }

    #[gpui::test]
    async fn test_engine_not_ready_defers_start_until_a_pump(cx: &mut TestAppContext) {
        init_test(cx);
        let (_view, cx, factory) = stub_view(false, DEFAULT_URL, cx);
        let controller = factory.controller(0);
        assert_eq!(controller.started_with(), None);

        // Engine comes up; the post-pump drain notices and triggers a render
        // that starts the tab.
        controller.set_engine_ready(true);
        pump(cx);
        assert_eq!(controller.started_with().as_deref(), Some(DEFAULT_URL));
    }

    #[gpui::test]
    async fn test_start_failure_shows_the_error_and_is_not_retried(cx: &mut TestAppContext) {
        init_test(cx);
        let factory = StubBackendFactory::new(true);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let view =
                BrowserView::new(backend_factory(&factory), DEFAULT_URL.to_string(), window, cx);
            factory.controller(0).fail_next_start("engine exploded");
            view
        });
        cx.run_until_parked();
        let controller = factory.controller(0);

        assert_eq!(controller.started_with(), None);
        view.update(cx, |view, _| {
            assert_eq!(
                view.active_tab().engine_error(),
                Some("engine exploded")
            );
        });

        // Further pumps and renders must not retry the failed start.
        pump(cx);
        let starts = controller
            .commands()
            .iter()
            .filter(|command| matches!(command, RecordedCommand::Start { .. }))
            .count();
        assert_eq!(starts, 1);
    }

    #[gpui::test]
    async fn test_plain_keys_route_to_the_page_but_app_chords_do_not(cx: &mut TestAppContext) {
        init_test(cx);
        let (view, cx, factory) = stub_view(true, DEFAULT_URL, cx);
        let controller = factory.controller(0);
        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });
        controller.take_commands();

        cx.simulate_keystrokes("a");
        assert_eq!(
            controller.take_commands(),
            vec![RecordedCommand::KeyDown {
                key: "a".into(),
                is_held: false,
            }],
            "an unmodified printable key is forwarded to the page"
        );

        cx.simulate_keystrokes("ctrl-y ctrl-shift-y");
        assert_eq!(
            controller.take_commands(),
            vec![],
            "ctrl-modified chords are app-classified and never reach the page"
        );
    }

    #[gpui::test]
    async fn test_pointer_events_are_content_relative(cx: &mut TestAppContext) {
        let app_state = init_test(cx);
        let project = Project::test(app_state.fs.clone(), [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

        let factory = StubBackendFactory::new(true);
        workspace.update_in(cx, |workspace, window, cx| {
            let factory = factory.clone();
            BrowserView::open_with_factory(workspace, window, cx, move || {
                factory.create_backend()
            });
        });
        cx.run_until_parked();
        let controller = factory.controller(0);

        let view = workspace
            .update(cx, |workspace, cx| {
                workspace.items_of_type::<BrowserView>(cx).next()
            })
            .unwrap();
        let content_origin = view.update(cx, |view, _| view.content_bounds.origin);
        assert!(
            content_origin.y > px(0.),
            "inside a workspace pane the content sits below the tab bar"
        );

        controller.take_commands();
        let click_offset = point(px(15.), px(25.));
        cx.simulate_click(content_origin + click_offset, Modifiers::default());
        assert_eq!(
            controller.take_commands(),
            vec![
                RecordedCommand::SetFocus { focused: true },
                RecordedCommand::MouseDown {
                    position: click_offset,
                    button: MouseButton::Left,
                    click_count: 1,
                },
                RecordedCommand::MouseUp {
                    position: click_offset,
                    button: MouseButton::Left,
                },
            ],
            "click coordinates are translated by the pane offset"
        );

        cx.simulate_event(ScrollWheelEvent {
            position: content_origin + click_offset,
            delta: ScrollDelta::Lines(point(0.0, -2.0)),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
        });
        assert_eq!(
            controller.take_commands(),
            vec![RecordedCommand::ScrollWheel {
                position: click_offset,
                delta: (0.0, -2.0, true),
            }],
            "scroll coordinates are translated by the pane offset"
        );
    }

    #[gpui::test]
    async fn test_navigation_actions_drive_the_backend(cx: &mut TestAppContext) {
        init_test(cx);
        let (view, cx, factory) = stub_view(true, DEFAULT_URL, cx);
        let controller = factory.controller(0);
        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });
        controller.take_commands();

        cx.dispatch_action(GoBack);
        cx.dispatch_action(GoForward);
        cx.dispatch_action(Reload);
        assert_eq!(
            controller.take_commands(),
            vec![
                RecordedCommand::GoBack,
                RecordedCommand::GoForward,
                RecordedCommand::Reload,
            ]
        );
    }

    #[gpui::test]
    async fn test_omnibox_confirm_resolves_text_and_navigates(cx: &mut TestAppContext) {
        init_test(cx);
        let (view, cx, factory) = stub_view(true, DEFAULT_URL, cx);
        let controller = factory.controller(0);
        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });

        cx.dispatch_action(FocusOmnibox);
        view.update_in(cx, |view, window, cx| {
            assert!(
                view.omnibox.focus_handle(cx).is_focused(window),
                "FocusOmnibox moves focus into the omnibox editor"
            );
            view.omnibox.update(cx, |omnibox, cx| {
                omnibox.set_editor_text("example.com", window, cx);
            });
        });

        controller.take_commands();
        cx.dispatch_action(menu::Confirm);
        cx.run_until_parked();

        assert_eq!(
            controller.take_commands(),
            vec![
                RecordedCommand::Navigate {
                    url: "https://example.com".into(),
                },
                RecordedCommand::SetFocus { focused: true },
            ],
            "confirmed text is resolved through the URL heuristic and the \
             engine browser is refocused"
        );
        view.update_in(cx, |view, window, cx| {
            assert_eq!(view.url(), "https://example.com");
            assert!(
                view.focus_handle.is_focused(window),
                "focus returns to the page content after navigating"
            );
            assert_eq!(
                view.omnibox.read(cx).editor_text(cx),
                "https://example.com",
                "the omnibox mirrors the new address"
            );
        });
    }

    #[gpui::test]
    async fn test_omnibox_cancel_reverts_to_the_current_url(cx: &mut TestAppContext) {
        init_test(cx);
        let (view, cx, factory) = stub_view(true, "https://example.com", cx);
        let controller = factory.controller(0);
        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });

        cx.dispatch_action(FocusOmnibox);
        view.update_in(cx, |view, window, cx| {
            view.omnibox.update(cx, |omnibox, cx| {
                omnibox.set_editor_text("half-typed query", window, cx);
            });
        });

        controller.take_commands();
        cx.dispatch_action(menu::Cancel);
        cx.run_until_parked();

        view.update(cx, |view, cx| {
            assert_eq!(view.omnibox.read(cx).editor_text(cx), "https://example.com");
        });
        assert_eq!(
            controller.take_commands(),
            vec![],
            "cancelling never navigates"
        );
    }

    #[gpui::test]
    async fn test_typing_in_the_omnibox_does_not_reach_the_page(cx: &mut TestAppContext) {
        init_test(cx);
        let (view, cx, factory) = stub_view(true, DEFAULT_URL, cx);
        let controller = factory.controller(0);
        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });

        cx.dispatch_action(FocusOmnibox);
        controller.take_commands();
        cx.simulate_input("zed");

        assert_eq!(
            controller.take_commands(),
            vec![],
            "keystrokes aimed at the omnibox editor are not forwarded to the page"
        );
        view.update(cx, |view, cx| {
            assert_eq!(view.omnibox.read(cx).editor_text(cx), "zed");
        });
    }

    #[gpui::test]
    async fn test_favicon_updates_tab_and_chrome_state(cx: &mut TestAppContext) {
        init_test(cx);
        let (view, cx, factory) = stub_view(true, DEFAULT_URL, cx);
        let controller = factory.controller(0);

        view.update_in(cx, |view, window, cx| {
            assert!(
                view.tab_icon(window, cx).is_some(),
                "without a favicon the tab shows the generic globe icon"
            );
        });

        controller.script_events([TabBackendEvent::FaviconUrlsChanged(vec![
            "https://example.com/favicon.ico".to_string(),
        ])]);
        pump(cx);

        view.update_in(cx, |view, window, cx| {
            assert_eq!(
                view.active_tab().favicon_url().map(|url| url.to_string()),
                Some("https://example.com/favicon.ico".to_string())
            );
            assert!(
                view.tab_icon(window, cx).is_none(),
                "the favicon replaces the icon slot in the pane tab"
            );
        });

        controller.script_events([TabBackendEvent::FaviconUrlsChanged(Vec::new())]);
        pump(cx);
        view.update_in(cx, |view, window, cx| {
            assert!(view.active_tab().favicon_url().is_none());
            assert!(view.tab_icon(window, cx).is_some());
        });
    }

    #[gpui::test]
    async fn test_open_is_a_workspace_singleton(cx: &mut TestAppContext) {
        let app_state = init_test(cx);
        let project = Project::test(app_state.fs.clone(), [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

        workspace.update_in(cx, |workspace, window, cx| {
            BrowserView::open_with_factory(workspace, window, cx, || {
                Box::new(crate::stub_tab_backend::StubTabBackend::new(false).0)
            });
        });
        cx.run_until_parked();

        let first = workspace.update(cx, |workspace, cx| {
            let items: Vec<_> = workspace.items_of_type::<BrowserView>(cx).collect();
            assert_eq!(items.len(), 1);
            items[0].clone()
        });

        // Opening again focuses the existing view instead of creating another.
        workspace.update_in(cx, |workspace, window, cx| {
            BrowserView::open_with_factory(workspace, window, cx, || {
                Box::new(crate::stub_tab_backend::StubTabBackend::new(false).0)
            });
        });
        cx.run_until_parked();

        workspace.update(cx, |workspace, cx| {
            let items: Vec<_> = workspace.items_of_type::<BrowserView>(cx).collect();
            assert_eq!(items.len(), 1);
            assert_eq!(items[0], first);
            assert_eq!(workspace.active_item_as::<BrowserView>(cx), Some(first));
        });

        // The item closes like any workspace item.
        let pane = workspace.update(cx, |workspace, _| workspace.active_pane().clone());
        pane.update_in(cx, |pane, window, cx| {
            pane.close_active_item(&Default::default(), window, cx)
        })
        .await
        .unwrap();
        workspace.update(cx, |workspace, cx| {
            assert_eq!(workspace.items_of_type::<BrowserView>(cx).count(), 0);
        });
    }

    #[gpui::test]
    async fn test_pump_observers_unregister_with_their_views(cx: &mut TestAppContext) {
        let app_state = init_test(cx);
        let project = Project::test(app_state.fs.clone(), [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

        fn observer_count(cx: &mut VisualTestContext) -> usize {
            cx.update(|_, cx| cx.default_global::<crate::PumpObservers>().0.len())
        }

        assert_eq!(observer_count(cx), 0);
        workspace.update_in(cx, |workspace, window, cx| {
            BrowserView::open_with_factory(workspace, window, cx, || {
                Box::new(crate::stub_tab_backend::StubTabBackend::new(true).0)
            });
        });
        cx.run_until_parked();
        assert_eq!(observer_count(cx), 1);

        pump(cx);
        assert_eq!(observer_count(cx), 1, "live observers survive pumps");

        let pane = workspace.update(cx, |workspace, _| workspace.active_pane().clone());
        pane.update_in(cx, |pane, window, cx| {
            pane.close_active_item(&Default::default(), window, cx)
        })
        .await
        .unwrap();
        cx.run_until_parked();

        // The released view's observer is dropped by the next pump.
        pump(cx);
        assert_eq!(observer_count(cx), 0);
    }

    #[gpui::test]
    async fn test_new_tab_opens_activates_and_starts_a_fresh_engine_tab(
        cx: &mut TestAppContext,
    ) {
        init_test(cx);
        let (view, cx, factory) = stub_view(true, "https://example.com", cx);
        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });

        cx.dispatch_action(NewTab);
        cx.run_until_parked();

        assert_eq!(factory.created_count(), 2);
        assert_eq!(
            factory.controller(1).started_with().as_deref(),
            Some(DEFAULT_URL),
            "the new tab's engine browser starts on the next draw"
        );
        view.update_in(cx, |view, window, cx| {
            assert_eq!(view.tabs.len(), 2);
            assert_eq!(view.active_tab_index, 1);
            assert!(
                view.omnibox.focus_handle(cx).is_focused(window),
                "a fresh tab puts the caret in the omnibox"
            );
        });

        // The old tab was blurred and hidden when the new one took over.
        let old_commands = factory.controller(0).commands();
        assert!(
            old_commands
                .windows(2)
                .any(|pair| pair
                    == [
                        RecordedCommand::SetFocus { focused: false },
                        RecordedCommand::SetHidden { hidden: true },
                    ]),
            "switching away blurs and hides the previous tab, got {old_commands:?}"
        );
    }

    #[gpui::test]
    async fn test_close_tab_closes_engine_and_activates_neighbor(cx: &mut TestAppContext) {
        init_test(cx);
        let (view, cx, factory) = stub_view(true, "https://one.example", cx);
        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });
        cx.dispatch_action(NewTab);
        cx.dispatch_action(NewTab);
        cx.run_until_parked();
        let (order, active) = tab_order_and_active(&view, cx);
        assert_eq!((order, active), (vec![0, 1, 2], 2));

        // Close the middle tab via its strip button path.
        view.update_in(cx, |view, window, cx| {
            view.close_tab_at(1, window, cx);
        });
        cx.run_until_parked();

        assert_eq!(
            factory.controller(1).take_commands().last(),
            Some(&RecordedCommand::Close),
            "closing a tab closes its engine browser"
        );
        let (order, active) = tab_order_and_active(&view, cx);
        assert_eq!(
            (order, active),
            (vec![0, 2], 2),
            "the active tab stays active when a tab before it closes"
        );

        // Closing the active (last) tab activates the tab now at the end.
        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });
        cx.dispatch_action(CloseTab);
        cx.run_until_parked();
        let (order, active) = tab_order_and_active(&view, cx);
        assert_eq!((order, active), (vec![0], 0));
        assert_eq!(
            factory.controller(0).commands().last(),
            Some(&RecordedCommand::SetFocus { focused: true }),
            "the surviving tab is shown and refocused"
        );
    }

    #[gpui::test]
    async fn test_closing_the_last_tab_replaces_it_with_a_fresh_tab(cx: &mut TestAppContext) {
        init_test(cx);
        let (view, cx, factory) = stub_view(true, "https://example.com", cx);
        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });

        cx.dispatch_action(CloseTab);
        cx.run_until_parked();

        assert_eq!(
            factory.controller(0).commands().last(),
            Some(&RecordedCommand::Close)
        );
        view.update(cx, |view, _| {
            assert_eq!(view.tabs.len(), 1, "the view never shows zero tabs");
            assert_eq!(view.url(), DEFAULT_URL);
        });
        assert_eq!(
            factory.controller(1).started_with().as_deref(),
            Some(DEFAULT_URL),
            "the replacement tab starts its own engine browser"
        );
    }

    #[gpui::test]
    async fn test_pinning_orders_pinned_tabs_first_and_tracks_the_active_tab(
        cx: &mut TestAppContext,
    ) {
        init_test(cx);
        let (view, cx, _factory) = stub_view(true, "https://one.example", cx);
        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });
        cx.dispatch_action(NewTab);
        cx.dispatch_action(NewTab);
        cx.run_until_parked();

        // Pin the active (third) tab: it moves to the front, stays active.
        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });
        cx.dispatch_action(PinTab);
        cx.run_until_parked();
        let (order, active) = tab_order_and_active(&view, cx);
        assert_eq!((order, active), (vec![2, 0, 1], 2));
        view.update(cx, |view, _| {
            assert!(view.active_tab().is_pinned());
        });

        // Pin another: pinned group keeps pin order, unpinned keep theirs.
        view.update_in(cx, |view, window, cx| {
            view.activate_tab(2, window, cx); // tab id 1
        });
        cx.run_until_parked();
        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });
        cx.dispatch_action(PinTab);
        cx.run_until_parked();
        let (order, active) = tab_order_and_active(&view, cx);
        assert_eq!((order, active), (vec![2, 1, 0], 1));

        // Unpinning sends the tab back into the unpinned group, stable order.
        cx.dispatch_action(UnpinTab);
        cx.run_until_parked();
        let (order, active) = tab_order_and_active(&view, cx);
        assert_eq!((order, active), (vec![2, 1, 0], 1));
        view.update(cx, |view, _| {
            assert!(!view.tabs[1].is_pinned());
            assert!(view.tabs[0].is_pinned());
        });
    }

    #[gpui::test]
    async fn test_reopen_closed_tab_restores_the_page_and_pin_state(cx: &mut TestAppContext) {
        init_test(cx);
        let (view, cx, factory) = stub_view(true, "https://one.example", cx);
        let controller = factory.controller(0);
        controller.script_events([
            TabBackendEvent::TitleChanged("Page One".into()),
            TabBackendEvent::FaviconUrlsChanged(vec!["https://one.example/icon.png".into()]),
        ]);
        pump(cx);

        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });
        cx.dispatch_action(NewTab);
        cx.run_until_parked();

        // Close the first tab, then reopen it.
        view.update_in(cx, |view, window, cx| {
            view.close_tab_at(0, window, cx);
        });
        cx.run_until_parked();
        view.update(cx, |view, _| assert_eq!(view.tabs.len(), 1));

        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });
        cx.dispatch_action(ReopenClosedTab);
        cx.run_until_parked();

        view.update(cx, |view, _| {
            assert_eq!(view.tabs.len(), 2);
            assert_eq!(view.url(), "https://one.example");
            assert_eq!(view.title(), "Page One");
            assert_eq!(
                view.active_tab().favicon_url().map(|url| url.to_string()),
                Some("https://one.example/icon.png".to_string())
            );
        });
        assert_eq!(
            factory.controller(2).started_with().as_deref(),
            Some("https://one.example"),
            "the reopened tab starts a fresh engine browser at the restored URL"
        );

        // Reopening with an empty stack is a no-op.
        cx.dispatch_action(ReopenClosedTab);
        cx.run_until_parked();
        view.update(cx, |view, _| assert_eq!(view.tabs.len(), 2));
    }

    #[gpui::test]
    async fn test_reopened_pinned_tab_returns_to_the_pinned_group(cx: &mut TestAppContext) {
        init_test(cx);
        let (view, cx, _factory) = stub_view(true, "https://pinned.example", cx);
        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });
        cx.dispatch_action(PinTab);
        cx.dispatch_action(NewTab);
        cx.run_until_parked();

        // Close the pinned tab (id 0), leaving the unpinned tab (id 1).
        view.update_in(cx, |view, window, cx| {
            view.close_tab_at(0, window, cx);
        });
        cx.run_until_parked();

        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });
        cx.dispatch_action(ReopenClosedTab);
        cx.run_until_parked();

        let (order, active) = tab_order_and_active(&view, cx);
        assert_eq!(
            (order, active),
            (vec![2, 1], 2),
            "the reopened pinned tab sorts into the pinned group and is active"
        );
        view.update(cx, |view, _| {
            assert!(view.active_tab().is_pinned());
            assert_eq!(view.url(), "https://pinned.example");
        });
    }

    #[gpui::test]
    async fn test_next_and_previous_tab_wrap_around(cx: &mut TestAppContext) {
        init_test(cx);
        let (view, cx, _factory) = stub_view(true, "https://one.example", cx);
        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });
        cx.dispatch_action(NewTab);
        cx.dispatch_action(NewTab);
        cx.run_until_parked();
        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });
        assert_eq!(view.update(cx, |view, _| view.active_tab_index), 2);

        cx.dispatch_action(NextTab);
        assert_eq!(
            view.update(cx, |view, _| view.active_tab_index),
            0,
            "NextTab wraps from the last tab to the first"
        );
        cx.dispatch_action(NextTab);
        assert_eq!(view.update(cx, |view, _| view.active_tab_index), 1);
        cx.dispatch_action(PreviousTab);
        assert_eq!(view.update(cx, |view, _| view.active_tab_index), 0);
        cx.dispatch_action(PreviousTab);
        assert_eq!(
            view.update(cx, |view, _| view.active_tab_index),
            2,
            "PreviousTab wraps from the first tab to the last"
        );
    }

    #[gpui::test]
    async fn test_switching_tabs_swaps_presented_frames_without_bleed(cx: &mut TestAppContext) {
        init_test(cx);
        let (view, cx, factory) = stub_view(true, "https://one.example", cx);

        // Tab 0 paints a frame.
        factory.controller(0).script_frame(SoftwareFrame {
            width: 4,
            height: 4,
            bgra: vec![0x11; 64],
        });
        pump(cx);
        assert!(view.update(cx, |view, _| view.active_tab().has_frame()));

        // A fresh tab presents no frame — tab 0's pixels must not show.
        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });
        cx.dispatch_action(NewTab);
        cx.run_until_parked();
        view.update(cx, |view, _| {
            assert!(
                !view.active_tab().has_frame(),
                "a newly opened tab must not present the previous tab's frame"
            );
            assert!(
                view.tabs[0].has_frame(),
                "the hidden tab keeps its own last frame"
            );
        });

        // The new tab paints; switching back and forth presents each tab's
        // own last frame.
        factory.controller(1).script_frame(SoftwareFrame {
            width: 4,
            height: 4,
            bgra: vec![0x22; 64],
        });
        pump(cx);
        assert!(view.update(cx, |view, _| view.active_tab().has_frame()));

        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });
        cx.dispatch_action(PreviousTab);
        cx.run_until_parked();
        view.update(cx, |view, _| {
            assert_eq!(view.active_tab_index, 0);
            assert!(view.active_tab().has_frame());
        });

        // The engine halves were told about visibility on every switch.
        let hidden_calls: Vec<_> = factory
            .controller(0)
            .commands()
            .iter()
            .filter_map(|command| match command {
                RecordedCommand::SetHidden { hidden } => Some(*hidden),
                _ => None,
            })
            .collect();
        assert_eq!(
            hidden_calls,
            vec![true, false],
            "tab 0 was hidden on switch-away and shown on switch-back"
        );
    }

    #[gpui::test]
    async fn test_closing_the_view_closes_all_engine_tabs(cx: &mut TestAppContext) {
        let app_state = init_test(cx);
        let project = Project::test(app_state.fs.clone(), [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

        let factory = StubBackendFactory::new(true);
        workspace.update_in(cx, |workspace, window, cx| {
            let factory = factory.clone();
            BrowserView::open_with_factory(workspace, window, cx, move || {
                factory.create_backend()
            });
        });
        cx.run_until_parked();

        let view = workspace
            .update(cx, |workspace, cx| {
                workspace.items_of_type::<BrowserView>(cx).next()
            })
            .unwrap();
        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });
        cx.dispatch_action(NewTab);
        cx.dispatch_action(NewTab);
        cx.run_until_parked();
        assert_eq!(factory.created_count(), 3);
        drop(view);

        let pane = workspace.update(cx, |workspace, _| workspace.active_pane().clone());
        pane.update_in(cx, |pane, window, cx| {
            pane.close_active_item(&Default::default(), window, cx)
        })
        .await
        .unwrap();
        cx.run_until_parked();

        assert_eq!(
            factory.closed_count(),
            3,
            "closing the browser view closes every engine tab"
        );
    }

    #[gpui::test]
    async fn test_input_routes_to_the_active_tab_only(cx: &mut TestAppContext) {
        init_test(cx);
        let (view, cx, factory) = stub_view(true, "https://one.example", cx);
        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });
        cx.dispatch_action(NewTab);
        cx.run_until_parked();
        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });
        factory.controller(0).take_commands();
        factory.controller(1).take_commands();

        cx.simulate_keystrokes("x");
        assert_eq!(
            factory.controller(1).take_commands(),
            vec![RecordedCommand::KeyDown {
                key: "x".into(),
                is_held: false,
            }],
            "keystrokes go to the active tab"
        );
        assert_eq!(
            factory.controller(0).take_commands(),
            vec![],
            "inactive tabs receive no input"
        );
    }

    #[gpui::test]
    async fn test_inactive_tab_starts_lazily_on_activation(cx: &mut TestAppContext) {
        init_test(cx);
        let (view, cx, factory) = stub_view(false, "https://one.example", cx);
        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });
        cx.dispatch_action(NewTab);
        cx.run_until_parked();

        // Engine comes up while tab 1 is active: only the active tab starts.
        factory.controller(0).set_engine_ready(true);
        factory.controller(1).set_engine_ready(true);
        pump(cx);
        assert_eq!(
            factory.controller(1).started_with().as_deref(),
            Some(DEFAULT_URL)
        );
        assert_eq!(
            factory.controller(0).started_with(),
            None,
            "an inactive tab does not create its engine browser"
        );

        // Activating the waiting tab starts it at its URL.
        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });
        cx.dispatch_action(NextTab);
        cx.run_until_parked();
        assert_eq!(
            factory.controller(0).started_with().as_deref(),
            Some("https://one.example"),
            "activation creates the engine browser with real bounds"
        );
    }

    #[gpui::test]
    async fn test_reopen_stack_is_capped(cx: &mut TestAppContext) {
        init_test(cx);
        let (view, cx, _factory) = stub_view(true, "https://one.example", cx);

        view.update_in(cx, |view, window, cx| {
            for index in 0..(MAX_CLOSED_TABS + 5) {
                let tab = view.create_tab(format!("https://site{index}.example"));
                view.tabs.push(tab);
                let closing_index = view.tabs.len() - 1;
                view.close_tab_at(closing_index, window, cx);
            }
        });

        view.update(cx, |view, _| {
            assert_eq!(view.closed_tabs.len(), MAX_CLOSED_TABS);
            assert_eq!(
                view.closed_tabs.last().map(|closed| closed.url.as_str()),
                Some("https://site24.example"),
                "the newest closed tab is on top of the stack"
            );
        });
    }
}
