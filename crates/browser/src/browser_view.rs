//! The browser view: the single per-workspace pane item that owns browser
//! tabs and their chrome (ADR-0004). M2 scope: full browser-tab management
//! inside the view — an internal tab strip, open/close/switch, pin/unpin with
//! pinned-first ordering, reopen-closed-tab, next/previous tab, per-tab
//! favicons and titles (ticket #9); visit recording into browsing history and
//! the app-rendered new-tab page for fresh tabs (ticket #11). Switching
//! browser tabs swaps the presented engine surface: each tab owns its backend
//! and presenter (`browser_tab.rs`).
//!
//! Everything here sits above the two seams: engine communication flows
//! through the tab-backend trait and frame presentation through the frame
//! presenter, so this file is platform-neutral and testable with a scripted
//! stub backend.

use crate::browser_tab::{BrowserTab, ClosedTab};
use crate::history::BrowserHistory;
use crate::omnibox::{Omnibox, OmniboxEvent};
use crate::session;
use crate::tab_backend::TabBackend;
use anyhow::anyhow;
use db::kvp::KeyValueStore;
use gpui::{
    AnyElement, App, Bounds, ClickEvent, Context, Entity, EntityId, EventEmitter, FocusHandle,
    Focusable, Global, KeyDownEvent, KeyUpEvent, Keystroke, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Render, ScrollWheelEvent, SharedString, SharedUri,
    Subscription, Task, WeakEntity, Window, actions, canvas, div, img,
};
use project::Project;
use std::sync::Arc;
use std::time::Duration;
use ui::{CommonAnimationExt, IconButtonShape, Tab, TabPosition, Tooltip, prelude::*};
use util::ResultExt as _;
use workspace::item::{Item, ItemEvent, SerializableItem, TabContentParams, TabTooltipContent};
use workspace::{ItemId, Workspace, WorkspaceId};

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

/// How many closed browser tabs the reopen stack remembers
/// (`Glass:crates/browser/src/browser_view.rs:41`).
const MAX_CLOSED_TABS: usize = 20;

/// How long tab mutations batch before the session is written
/// (`Glass:crates/browser/src/browser_view/session.rs:166`).
const SESSION_SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

/// Creates the engine backend for each new browser tab. Installed as a GPUI
/// global by `init` so workspace item deserialization can construct browser
/// views; tests install a stub factory.
#[derive(Clone)]
pub struct TabBackendFactory(Arc<dyn Fn() -> Box<dyn TabBackend>>);

impl TabBackendFactory {
    pub fn new(create: impl Fn() -> Box<dyn TabBackend> + 'static) -> Self {
        Self(Arc::new(create))
    }

    fn create_backend(&self) -> Box<dyn TabBackend> {
        (self.0)()
    }
}

impl Global for TabBackendFactory {}

/// The single designated writer of the persisted session (plan §3.5): the
/// first non-incognito browser view claims ownership, restores the saved
/// session, and is the only view that saves it — so browser views in other
/// workspaces cannot fight over the global key.
#[derive(Default)]
struct SessionOwner(Option<EntityId>);

impl Global for SessionOwner {}

#[cfg(feature = "cef")]
pub fn init(cx: &mut App) {
    cx.set_global(TabBackendFactory::new(|| {
        Box::new(crate::tab::CefTab::new())
    }));
    workspace::register_serializable_item::<BrowserView>(cx);
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
    closed_tabs: Vec<ClosedTab>,
    next_tab_id: usize,
    omnibox: Entity<Omnibox>,
    /// The app-wide shared browsing history; this view records visits into it
    /// (unless incognito) and the omnibox suggests from it.
    history: Entity<BrowserHistory>,
    /// Window-relative bounds of the page content area, captured at draw time;
    /// pointer events are translated into content-relative coordinates with
    /// its origin before crossing the tab-backend seam.
    content_bounds: Bounds<Pixels>,
    /// Excluded from session persistence (CONTEXT.md "incognito window"). The
    /// incognito UI arrives with its own ticket; only the persistence
    /// exclusion is modeled here.
    is_incognito: bool,
    /// Debounced session write; replacing it pushes the deadline out.
    pending_session_save: Option<Task<()>>,
    _quit_flush: Subscription,
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
        Self::build(backend_factory, Some(initial_url), false, window, cx)
    }

    /// A browser view excluded from session persistence: it never claims
    /// session ownership, never restores, and never saves (CONTEXT.md
    /// "incognito window"). The incognito UI arrives with its own ticket.
    pub fn new_incognito(
        backend_factory: TabBackendFactory,
        initial_url: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::build(backend_factory, Some(initial_url), true, window, cx)
    }

    /// A browser view that restores the persisted session if it becomes the
    /// session owner and one is saved; otherwise it starts with a fresh
    /// new-tab page. Used when the view opens organically (`OpenBrowser`) and
    /// when the workspace restores the item.
    fn restore_or_new(
        backend_factory: TabBackendFactory,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::build(backend_factory, None, false, window, cx)
    }

    fn build(
        backend_factory: TabBackendFactory,
        initial_url: Option<String>,
        is_incognito: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let weak = cx.weak_entity();
        crate::observe_pumps(cx, move |cx| {
            weak.update(cx, |view, cx| view.drain_engine_events(cx))
                .is_ok()
        });

        let entity_id = cx.entity_id();
        Self::claim_session_ownership(is_incognito, cx);

        let window_handle = window.window_handle();
        cx.on_release(move |this, cx| {
            // Backstop for teardown paths that skip `on_removed`, e.g. the
            // whole window closing with the item still in its pane.
            Self::release_session_ownership(entity_id, cx);
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

        let history = BrowserHistory::global(cx);
        let omnibox = cx.new(|cx| Omnibox::new(history.clone(), window, cx));
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
            history,
            content_bounds: Bounds::default(),
            is_incognito,
            pending_session_save: None,
            _quit_flush: cx.on_app_quit(Self::flush_session_on_quit),
        };

        let saved = if initial_url.is_none() && Self::is_session_owner(cx) {
            session::restore(cx).filter(|saved| !saved.tabs.is_empty())
        } else {
            None
        };
        match saved {
            Some(saved) => this.restore_session(saved),
            None => {
                let tab = match initial_url {
                    Some(url) => this.create_tab(url),
                    None => this.create_new_tab_page(),
                };
                this.tabs.push(tab);
            }
        }
        this
    }

    /// Rebuild browser tabs from the saved session. Restoration is lazy: only
    /// the active tab creates its engine browser (on the first draw); the
    /// others wait until activated.
    fn restore_session(&mut self, saved: session::SerializedBrowserTabs) {
        if saved.tabs.is_empty() {
            return;
        }
        for serialized in saved.tabs {
            let id = self.allocate_tab_id();
            let tab = if serialized.is_new_tab_page {
                BrowserTab::new_tab_page(id, self.backend_factory.create_backend())
            } else {
                BrowserTab::restore(
                    id,
                    self.backend_factory.create_backend(),
                    ClosedTab {
                        url: serialized.url,
                        title: serialized.title,
                        favicon_url: serialized.favicon_url.map(SharedUri::from),
                        is_pinned: serialized.is_pinned,
                    },
                )
            };
            self.tabs.push(tab);
        }
        self.active_tab_index = saved.active_index.min(self.tabs.len() - 1);
        // The saved order is strip order, but re-sort so an older or
        // hand-edited blob cannot violate the pinned-first invariant.
        let active_id = self.tabs[self.active_tab_index].id;
        self.tabs.sort_by_key(|tab| !tab.is_pinned());
        if let Some(index) = self.tabs.iter().position(|tab| tab.id == active_id) {
            self.active_tab_index = index;
        }
    }

    #[cfg(feature = "cef")]
    pub fn open(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
        let backend_factory = cx.global::<TabBackendFactory>().clone();
        Self::open_internal(workspace, backend_factory, window, cx);
    }

    #[cfg(test)]
    fn open_with_factory(
        workspace: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Workspace>,
        backend_factory: impl Fn() -> Box<dyn TabBackend> + 'static,
    ) {
        Self::open_internal(
            workspace,
            TabBackendFactory::new(backend_factory),
            window,
            cx,
        );
    }

    #[cfg(any(feature = "cef", test))]
    fn open_internal(
        workspace: &mut Workspace,
        backend_factory: TabBackendFactory,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        // One browser view per workspace (ADR-0004); reopening focuses it.
        let existing = workspace.items_of_type::<BrowserView>(cx).next();
        if let Some(existing) = existing {
            workspace.activate_item(&existing, true, true, window, cx);
            return;
        }
        let view = cx.new(|cx| BrowserView::restore_or_new(backend_factory, window, cx));
        workspace.add_item_to_active_pane(Box::new(view), None, true, window, cx);
    }

    fn allocate_tab_id(&mut self) -> usize {
        util::post_inc(&mut self.next_tab_id)
    }

    fn create_tab(&mut self, url: String) -> BrowserTab {
        let id = self.allocate_tab_id();
        BrowserTab::new(id, self.backend_factory.create_backend(), url)
    }

    fn create_new_tab_page(&mut self) -> BrowserTab {
        let id = self.allocate_tab_id();
        BrowserTab::new_tab_page(id, self.backend_factory.create_backend())
    }

    /// Whether this view holds the session-owner slot: the single designated
    /// writer of the persisted session.
    fn is_session_owner(cx: &Context<Self>) -> bool {
        cx.try_global::<SessionOwner>()
            .is_some_and(|owner| owner.0 == Some(cx.entity_id()))
    }

    /// Claim the session-owner slot if it is free. Incognito views never
    /// participate.
    fn claim_session_ownership(is_incognito: bool, cx: &mut Context<Self>) {
        if is_incognito {
            return;
        }
        let entity_id = cx.entity_id();
        let owner = cx.default_global::<SessionOwner>();
        if owner.0.is_none() {
            owner.0 = Some(entity_id);
        }
    }

    fn release_session_ownership(entity_id: EntityId, cx: &mut App) {
        let owner = cx.default_global::<SessionOwner>();
        if owner.0 == Some(entity_id) {
            owner.0 = None;
        }
    }

    /// The session as saved: all tabs in strip order plus the active index.
    /// `None` when this view must not write the session (incognito, or not
    /// the session owner).
    fn serialize_session(&self, cx: &Context<Self>) -> Option<String> {
        if self.is_incognito || !Self::is_session_owner(cx) {
            return None;
        }
        let tabs = self
            .tabs
            .iter()
            .map(|tab| session::SerializedTab {
                url: tab.url().to_string(),
                title: tab.title().to_string(),
                is_new_tab_page: tab.is_new_tab_page(),
                is_pinned: tab.is_pinned(),
                favicon_url: tab.favicon_url().map(|url| url.to_string()),
            })
            .collect();
        serde_json::to_string(&session::SerializedBrowserTabs {
            tabs,
            active_index: self.active_tab_index,
        })
        .log_err()
    }

    /// Debounced session write: tab mutations within the window batch into
    /// one KV write. Quit flushes immediately instead
    /// ([`flush_session_on_quit`](Self::flush_session_on_quit)).
    fn schedule_session_save(&mut self, cx: &mut Context<Self>) {
        if self.is_incognito || !Self::is_session_owner(cx) {
            return;
        }
        self.pending_session_save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SESSION_SAVE_DEBOUNCE).await;
            let Ok((session, store)) = this.update(cx, |this, cx| {
                this.pending_session_save = None;
                (this.serialize_session(cx), KeyValueStore::global(cx))
            }) else {
                return;
            };
            if let Some(session) = session {
                session::save(store, session).await.log_err();
            }
        }));
    }

    fn flush_session_on_quit(&mut self, cx: &mut Context<Self>) -> Task<()> {
        self.pending_session_save = None;
        let Some(session) = self.serialize_session(cx) else {
            return Task::ready(());
        };
        let store = KeyValueStore::global(cx);
        cx.background_spawn(async move {
            session::save(store, session).await.log_err();
        })
    }

    // `tabs` is never empty and `active_tab_index` is kept in bounds by every
    // tab-list mutation, so these lookups cannot fail.
    fn active_tab(&self) -> &BrowserTab {
        &self.tabs[self.active_tab_index]
    }

    fn active_tab_mut(&mut self) -> &mut BrowserTab {
        &mut self.tabs[self.active_tab_index]
    }

    /// The active tab, if it has an engine page to receive input — a new-tab
    /// page has none, so user input aimed at the content area goes nowhere.
    fn active_engine_tab_mut(&mut self) -> Option<&mut BrowserTab> {
        let tab = &mut self.tabs[self.active_tab_index];
        if tab.is_new_tab_page() {
            None
        } else {
            Some(tab)
        }
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
        let mut session_changed = false;
        let mut visits = Vec::new();
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            let changes = tab.drain_events();
            if changes.identity_changed || changes.needs_notify {
                // Any tab's title or favicon shows in the tab strip.
                needs_notify = true;
            }
            if changes.identity_changed {
                // URL, title, and favicon are part of the saved session.
                session_changed = true;
                if index == self.active_tab_index {
                    active_identity_changed = true;
                }
            }
            if changes.visited {
                // One visit per drain, however many address/title events the
                // batch held: the entry ends up with the batch's final state.
                visits.push((tab.url().to_string(), tab.title().to_string()));
            }
        }

        if !self.is_incognito {
            for (url, title) in visits {
                self.history.update(cx, |history, cx| {
                    history.record_visit(&url, &title, cx);
                });
            }
        }
        if session_changed {
            self.schedule_session_save(cx);
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
        self.schedule_session_save(cx);
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

    /// Put the caret in an emptied omnibox, for a freshly created new-tab
    /// page.
    fn focus_omnibox_blank(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.omnibox.update(cx, |omnibox, cx| {
            omnibox.start_blank_entry(window, cx);
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

        self.schedule_session_save(cx);
        cx.emit(ItemEvent::UpdateTab);
        cx.notify();
    }

    fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self.create_new_tab_page();
        self.tabs.push(tab);
        self.activate_tab(self.tabs.len() - 1, window, cx);
        // A fresh tab shows the new-tab page; the user's next step is typing
        // a destination.
        self.focus_omnibox_blank(window, cx);
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
            // resets to a fresh new-tab page, like Glass
            // (`Glass:crates/browser/src/browser_view/tabs.rs:455`).
            let tab = self.create_new_tab_page();
            self.tabs.push(tab);
            self.active_tab_index = 0;
            self.focus_omnibox_blank(window, cx);
        } else if index < self.active_tab_index {
            self.active_tab_index -= 1;
        } else if was_active {
            self.active_tab_index = self.active_tab_index.min(self.tabs.len() - 1);
            let tab = self.active_tab_mut();
            tab.set_hidden(false);
            tab.set_focus(true);
        }

        self.schedule_session_save(cx);
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
        let tab = BrowserTab::restore(id, self.backend_factory.create_backend(), closed);
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
        self.schedule_session_save(cx);
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
        let Some(tab) = self.active_engine_tab_mut() else {
            return;
        };
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
        let Some(tab) = self.active_engine_tab_mut() else {
            return;
        };
        tab.send_mouse_up(position, event.button, event.modifiers);
    }

    fn handle_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let position = event.position - self.content_bounds.origin;
        let Some(tab) = self.active_engine_tab_mut() else {
            return;
        };
        tab.send_mouse_move(position, event.pressed_button, event.modifiers);
    }

    fn handle_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let position = event.position - self.content_bounds.origin;
        let Some(tab) = self.active_engine_tab_mut() else {
            return;
        };
        tab.send_scroll_wheel(position, event.delta, event.modifiers);
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
        let Some(tab) = self.active_engine_tab_mut() else {
            return;
        };
        tab.send_key_down(&event.keystroke, event.is_held);
        cx.stop_propagation();
    }

    fn handle_key_up(&mut self, event: &KeyUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus_handle.is_focused(window) || is_app_keystroke(&event.keystroke) {
            return;
        }
        let Some(tab) = self.active_engine_tab_mut() else {
            return;
        };
        tab.send_key_up(&event.keystroke);
        cx.stop_propagation();
    }

    /// The app-rendered page a fresh tab shows before its first navigation
    /// (ticket #11). Input lives in the omnibox, which `new_tab` focuses.
    fn render_new_tab_page(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .child(
                Icon::new(IconName::ToolWeb)
                    .size(IconSize::XLarge)
                    .color(Color::Muted),
            )
            .child(
                Label::new("New Tab")
                    .size(LabelSize::Large)
                    .color(Color::Muted),
            )
            .child(
                Label::new("Search or enter an address in the omnibox")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
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
                this.child(
                    div().flex_none().max_w_64().child(
                        Label::new(title)
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .truncate(),
                    ),
                )
            })
    }
}

impl Render for BrowserView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_new_tab_page = self.active_tab().is_new_tab_page();
        let frame = if is_new_tab_page {
            None
        } else {
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

        // Weak: the window retains the last frame's element tree, so a strong
        // handle here would keep a closed view (and its session ownership)
        // alive until an unrelated redraw.
        let this = cx.weak_entity();
        let bounds_tracker = canvas(
            move |bounds, window, cx| {
                let scale_factor = window.scale_factor();
                this.update(cx, |view, _| {
                    view.handle_content_bounds(bounds, scale_factor)
                })
                .ok();
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
            .when(is_new_tab_page, |this| {
                this.child(self.render_new_tab_page(cx))
            })
            .when(!has_frame && !is_new_tab_page, |this| {
                this.child(self.render_placeholder(cx))
            });

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
            .on_action(
                cx.listener(|this, _: &PinTab, _, cx| this.pin_tab_at(this.active_tab_index, cx)),
            )
            .on_action(
                cx.listener(|this, _: &UnpinTab, _, cx| {
                    this.unpin_tab_at(this.active_tab_index, cx)
                }),
            )
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

    fn added_to_workspace(
        &mut self,
        _workspace: &mut Workspace,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Re-claim after the release in `on_removed` when the removal was a
        // move to another pane; a no-op when this view already owns the slot.
        Self::claim_session_ownership(self.is_incognito, cx);
    }

    fn on_removed(&self, cx: &mut Context<Self>) {
        // Deterministic release when the item leaves its pane: waiting for
        // entity release would leave the slot taken for a few more frames
        // (the window retains recent element trees), blocking a browser view
        // opened right after this one closes.
        Self::release_session_ownership(cx.entity_id(), cx);
    }
}

impl SerializableItem for BrowserView {
    fn serialized_item_kind() -> &'static str {
        "Browser"
    }

    fn cleanup(
        _workspace_id: WorkspaceId,
        _alive_items: Vec<ItemId>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Task<anyhow::Result<()>> {
        // The session is one global KV blob, not per-item rows; there is
        // nothing to prune when items disappear from a workspace.
        Task::ready(Ok(()))
    }

    fn serialize(
        &mut self,
        _workspace: &mut Workspace,
        _item_id: ItemId,
        closing: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Task<anyhow::Result<()>>> {
        if closing {
            let session = self.serialize_session(cx)?;
            // This write captures the current state, so a pending debounced
            // write would only repeat it.
            self.pending_session_save = None;
            let store = KeyValueStore::global(cx);
            Some(cx.background_spawn(async move { session::save(store, session).await }))
        } else {
            // Steady-state saves stay on this view's own debounce; writing
            // here would put the workspace's 200ms serialization throttle in
            // charge of the cadence instead.
            self.schedule_session_save(cx);
            None
        }
    }

    fn should_serialize(&self, event: &Self::Event) -> bool {
        matches!(event, ItemEvent::UpdateTab)
    }

    fn deserialize(
        _project: Entity<Project>,
        _workspace: WeakEntity<Workspace>,
        _workspace_id: WorkspaceId,
        _item_id: ItemId,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<Entity<Self>>> {
        // Installed by `init`; absent when the engine failed to initialize,
        // in which case the item cannot come back with the workspace.
        let Some(backend_factory) = cx.try_global::<TabBackendFactory>().cloned() else {
            return Task::ready(Err(anyhow!(
                "no browser engine available to restore the browser view"
            )));
        };
        let view = cx.new(|cx| BrowserView::restore_or_new(backend_factory, window, cx));
        Task::ready(Ok(view))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::omnibox::OmniboxSuggestion;
    use crate::stub_tab_backend::{RecordedCommand, StubBackendFactory, StubTabController};
    use crate::tab_backend::{SoftwareFrame, TabBackendEvent};
    use gpui::{
        Modifiers, ScrollDelta, TestAppContext, TouchPhase, VisualTestContext, point, size,
    };
    use project::Project;
    use std::sync::Arc;
    use workspace::AppState;

    const DEFAULT_URL: &str = "https://zed.dev";

    fn init_test(cx: &mut TestAppContext) -> Arc<AppState> {
        cx.update(|cx| {
            // Every test gets its own in-memory database: the session lives
            // under one global KV key, so tests sharing the process-wide
            // fallback database would see each other's saves.
            cx.set_global(db::AppDatabase::test_new());
            let app_state = AppState::test(cx);
            editor::init(cx);
            app_state
        })
    }

    fn backend_factory(factory: &StubBackendFactory) -> TabBackendFactory {
        let factory = factory.clone();
        TabBackendFactory::new(move || factory.create_backend())
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
    fn tab_order_and_active(
        view: &Entity<BrowserView>,
        cx: &mut VisualTestContext,
    ) -> (Vec<usize>, usize) {
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
            let view = BrowserView::new(
                backend_factory(&factory),
                DEFAULT_URL.to_string(),
                window,
                cx,
            );
            factory.controller(0).fail_next_start("engine exploded");
            view
        });
        cx.run_until_parked();
        let controller = factory.controller(0);

        assert_eq!(controller.started_with(), None);
        view.update(cx, |view, _| {
            assert_eq!(view.active_tab().engine_error(), Some("engine exploded"));
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
            BrowserView::open_with_factory(workspace, window, cx, move || factory.create_backend());
        });
        cx.run_until_parked();
        let controller = factory.controller(0);

        let view = workspace
            .update(cx, |workspace, cx| {
                workspace.items_of_type::<BrowserView>(cx).next()
            })
            .unwrap();
        // Leave the fresh view's new-tab page: pointer events only forward to
        // tabs with an engine page.
        view.update_in(cx, |view, window, cx| {
            view.navigate_to("https://one.example".to_string(), window, cx);
        });
        cx.run_until_parked();
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
    async fn test_new_tab_shows_the_new_tab_page_and_navigates_from_the_omnibox(
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
            factory.controller(1).started_with(),
            None,
            "a new-tab page starts no engine browser"
        );
        view.update_in(cx, |view, window, cx| {
            assert_eq!(view.tabs.len(), 2);
            assert_eq!(view.active_tab_index, 1);
            assert!(view.active_tab().is_new_tab_page());
            assert_eq!(view.url(), "");
            assert!(
                view.omnibox.focus_handle(cx).is_focused(window),
                "a fresh tab puts the caret in the omnibox"
            );
            assert_eq!(
                view.omnibox.read(cx).editor_text(cx),
                "",
                "the previous page's URL does not linger in the omnibox"
            );
        });

        // The old tab was blurred and hidden when the new one took over.
        let old_commands = factory.controller(0).commands();
        assert!(
            old_commands.windows(2).any(|pair| pair
                == [
                    RecordedCommand::SetFocus { focused: false },
                    RecordedCommand::SetHidden { hidden: true },
                ]),
            "switching away blurs and hides the previous tab, got {old_commands:?}"
        );

        // Committing the omnibox leaves the new-tab page and starts the
        // engine at the resolved URL.
        view.update_in(cx, |view, window, cx| {
            view.omnibox.update(cx, |omnibox, cx| {
                omnibox.set_editor_text("example.org", window, cx);
            });
        });
        cx.dispatch_action(menu::Confirm);
        cx.run_until_parked();

        view.update(cx, |view, _| {
            assert!(!view.active_tab().is_new_tab_page());
            assert_eq!(view.url(), "https://example.org");
        });
        assert_eq!(
            factory.controller(1).started_with().as_deref(),
            Some("https://example.org"),
            "the first navigation creates the engine browser at the resolved URL"
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
    async fn test_closing_the_last_tab_replaces_it_with_a_new_tab_page(cx: &mut TestAppContext) {
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
            assert!(
                view.active_tab().is_new_tab_page(),
                "the replacement tab is a fresh new-tab page"
            );
        });
        assert_eq!(
            factory.controller(1).started_with(),
            None,
            "the replacement new-tab page starts no engine browser"
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

        // The new tab navigates (starting its engine), paints; switching back
        // and forth presents each tab's own last frame.
        view.update_in(cx, |view, window, cx| {
            view.navigate_to("https://two.example".to_string(), window, cx);
        });
        cx.run_until_parked();
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
            BrowserView::open_with_factory(workspace, window, cx, move || factory.create_backend());
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
            view.navigate_to("https://two.example".to_string(), window, cx);
        });
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
        // The new tab leaves its new-tab page for a real URL while the engine
        // is still down; the navigation is only recorded.
        view.update_in(cx, |view, window, cx| {
            view.navigate_to("https://two.example".to_string(), window, cx);
        });
        cx.run_until_parked();

        // Engine comes up while tab 1 is active: only the active tab starts.
        factory.controller(0).set_engine_ready(true);
        factory.controller(1).set_engine_ready(true);
        pump(cx);
        assert_eq!(
            factory.controller(1).started_with().as_deref(),
            Some("https://two.example")
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

    /// Write a session blob to the KV store, as a previous run would have.
    async fn write_saved_session(session: session::SerializedBrowserTabs, cx: &mut TestAppContext) {
        let json = serde_json::to_string(&session).unwrap();
        cx.update(|cx| {
            let store = KeyValueStore::global(cx);
            cx.background_spawn(async move { session::save(store, json).await })
        })
        .await
        .unwrap();
    }

    #[gpui::test]
    async fn test_session_round_trips_through_the_kv_store(cx: &mut TestAppContext) {
        let app_state = init_test(cx);
        let project = Project::test(app_state.fs.clone(), [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

        let factory = StubBackendFactory::new(true);
        workspace.update_in(cx, |workspace, window, cx| {
            let factory = factory.clone();
            BrowserView::open_with_factory(workspace, window, cx, move || factory.create_backend());
        });
        cx.run_until_parked();
        let view = workspace
            .update(cx, |workspace, cx| {
                workspace.items_of_type::<BrowserView>(cx).next()
            })
            .unwrap();

        // With nothing saved, opening falls back to a fresh new-tab page and
        // this view becomes the session owner.
        view.update(cx, |view, cx| {
            assert!(BrowserView::is_session_owner(cx));
            assert_eq!(view.tabs.len(), 1);
            assert!(view.active_tab().is_new_tab_page());
        });

        // Build a session: the first tab navigates to a page that gets a
        // title and favicon and is pinned; a second tab is opened, navigated,
        // and titled.
        view.update_in(cx, |view, window, cx| {
            view.navigate_to(DEFAULT_URL.to_string(), window, cx);
        });
        cx.run_until_parked();
        factory.controller(0).script_events([
            TabBackendEvent::TitleChanged("Zed".into()),
            TabBackendEvent::FaviconUrlsChanged(vec!["https://zed.dev/favicon.ico".into()]),
        ]);
        pump(cx);
        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });
        cx.dispatch_action(PinTab);
        cx.dispatch_action(NewTab);
        cx.run_until_parked();
        view.update_in(cx, |view, window, cx| {
            view.navigate_to("https://example.com".to_string(), window, cx);
        });
        factory
            .controller(1)
            .script_events([TabBackendEvent::TitleChanged("Example".into())]);
        pump(cx);

        cx.executor().advance_clock(SESSION_SAVE_DEBOUNCE);
        cx.run_until_parked();
        let saved = cx.update(|_, cx| session::restore(cx)).unwrap();
        assert_eq!(saved.tabs.len(), 2);
        assert_eq!(saved.active_index, 1);

        // Close the pane item; the released view frees the owner slot.
        let pane = workspace.update(cx, |workspace, _| workspace.active_pane().clone());
        pane.update_in(cx, |pane, window, cx| {
            pane.close_active_item(&Default::default(), window, cx)
        })
        .await
        .unwrap();
        drop(view);
        cx.run_until_parked();

        // Reopening restores the whole session: tabs, titles, favicons,
        // pinned state, and the active index.
        let factory_after_restart = StubBackendFactory::new(true);
        workspace.update_in(cx, |workspace, window, cx| {
            let factory = factory_after_restart.clone();
            BrowserView::open_with_factory(workspace, window, cx, move || factory.create_backend());
        });
        cx.run_until_parked();
        let view = workspace
            .update(cx, |workspace, cx| {
                workspace.items_of_type::<BrowserView>(cx).next()
            })
            .unwrap();

        view.update(cx, |view, cx| {
            assert!(BrowserView::is_session_owner(cx));
            assert_eq!(view.tabs.len(), 2);
            assert!(view.tabs[0].is_pinned());
            assert_eq!(view.tabs[0].url(), DEFAULT_URL);
            assert_eq!(view.tabs[0].title(), "Zed");
            assert_eq!(
                view.tabs[0].favicon_url().map(|url| url.to_string()),
                Some("https://zed.dev/favicon.ico".to_string())
            );
            assert!(!view.tabs[1].is_pinned());
            assert_eq!(view.active_tab_index, 1);
            assert_eq!(view.url(), "https://example.com");
            assert_eq!(view.title(), "Example");
        });

        // Restoration is lazy: only the active tab started its engine.
        assert_eq!(
            factory_after_restart
                .controller(1)
                .started_with()
                .as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            factory_after_restart.controller(0).started_with(),
            None,
            "inactive restored tabs wait for activation to start their engines"
        );
    }

    #[gpui::test]
    async fn test_session_saves_are_debounced(cx: &mut TestAppContext) {
        init_test(cx);
        let (view, cx, _factory) = stub_view(true, "https://one.example", cx);

        view.update_in(cx, |view, window, cx| {
            view.navigate_to("https://two.example".to_string(), window, cx);
        });
        cx.run_until_parked();
        assert!(
            cx.update(|_, cx| session::restore(cx)).is_none(),
            "no write lands before the debounce elapses"
        );

        cx.executor().advance_clock(Duration::from_millis(300));
        view.update_in(cx, |view, window, cx| {
            view.navigate_to("https://three.example".to_string(), window, cx);
        });
        cx.executor().advance_clock(Duration::from_millis(300));
        cx.run_until_parked();
        assert!(
            cx.update(|_, cx| session::restore(cx)).is_none(),
            "a new mutation restarts the debounce window"
        );

        cx.executor().advance_clock(Duration::from_millis(200));
        cx.run_until_parked();
        let saved = cx.update(|_, cx| session::restore(cx)).unwrap();
        assert_eq!(saved.tabs.len(), 1);
        assert_eq!(
            saved.tabs[0].url, "https://three.example",
            "one write captures the batched mutations"
        );
    }

    #[gpui::test]
    async fn test_quit_flushes_the_pending_session_save(cx: &mut TestAppContext) {
        init_test(cx);
        let factory = StubBackendFactory::new(true);
        {
            let (view, cx) = cx.add_window_view(|window, cx| {
                BrowserView::new(
                    backend_factory(&factory),
                    "https://one.example".to_string(),
                    window,
                    cx,
                )
            });
            cx.run_until_parked();
            view.update_in(cx, |view, window, cx| {
                view.navigate_to("https://two.example".to_string(), window, cx);
            });
            cx.run_until_parked();
            assert!(
                cx.update(|_, cx| session::restore(cx)).is_none(),
                "the debounced write is still pending at quit"
            );
        }

        cx.update(|cx| cx.shutdown());

        let saved = cx.update(|cx| session::restore(cx)).unwrap();
        assert_eq!(
            saved.tabs[0].url, "https://two.example",
            "quit flushes the session without waiting for the debounce"
        );
    }

    #[gpui::test]
    async fn test_deserialize_restores_the_view_through_the_item_mechanism(
        cx: &mut TestAppContext,
    ) {
        let app_state = init_test(cx);
        write_saved_session(
            session::SerializedBrowserTabs {
                tabs: vec![
                    session::SerializedTab {
                        url: "https://pinned.example".to_string(),
                        title: "Pinned".to_string(),
                        is_new_tab_page: false,
                        is_pinned: true,
                        favicon_url: None,
                    },
                    session::SerializedTab {
                        url: "https://active.example".to_string(),
                        title: "Active".to_string(),
                        is_new_tab_page: false,
                        is_pinned: false,
                        favicon_url: Some("https://active.example/icon.png".to_string()),
                    },
                ],
                active_index: 1,
            },
            cx,
        )
        .await;

        let project = Project::test(app_state.fs.clone(), [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

        // Without an installed engine backend factory (init never ran), the
        // item cannot be restored.
        let restored = workspace.update_in(cx, |workspace, window, cx| {
            BrowserView::deserialize(
                workspace.project().clone(),
                workspace.weak_handle(),
                WorkspaceId::from_i64(1),
                1,
                window,
                cx,
            )
        });
        assert!(restored.await.is_err());

        let factory = StubBackendFactory::new(true);
        cx.update(|_, cx| {
            let factory = factory.clone();
            cx.set_global(TabBackendFactory::new(move || factory.create_backend()));
            workspace::register_serializable_item::<BrowserView>(cx);
        });

        let view = workspace
            .update_in(cx, |workspace, window, cx| {
                BrowserView::deserialize(
                    workspace.project().clone(),
                    workspace.weak_handle(),
                    WorkspaceId::from_i64(1),
                    1,
                    window,
                    cx,
                )
            })
            .await
            .unwrap();
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(Box::new(view.clone()), None, true, window, cx);
        });
        cx.run_until_parked();

        view.update(cx, |view, _| {
            assert_eq!(view.tabs.len(), 2);
            assert!(view.tabs[0].is_pinned());
            assert_eq!(view.tabs[0].url(), "https://pinned.example");
            assert_eq!(view.active_tab_index, 1);
            assert_eq!(view.url(), "https://active.example");
            assert_eq!(view.title(), "Active");
            assert_eq!(
                view.active_tab().favicon_url().map(|url| url.to_string()),
                Some("https://active.example/icon.png".to_string())
            );
        });
        assert_eq!(
            factory.controller(1).started_with().as_deref(),
            Some("https://active.example"),
            "the active restored tab starts on the first draw"
        );

        // The registered mechanism recognizes the view as a serializable
        // item under the kind deserialize is dispatched on.
        let kind = cx.update(|_, cx| {
            use workspace::item::ItemHandle as _;
            view.to_serializable_item_handle(cx)
                .map(|handle| handle.serialized_item_kind())
        });
        assert_eq!(kind, Some("Browser"));
    }

    #[gpui::test]
    async fn test_only_the_session_owner_writes_the_session(cx: &mut TestAppContext) {
        init_test(cx);
        let factory = StubBackendFactory::new(true);
        let (first, cx) = cx.add_window_view(|window, cx| {
            BrowserView::new(
                backend_factory(&factory),
                "https://one.example".to_string(),
                window,
                cx,
            )
        });
        cx.run_until_parked();

        let second_factory = StubBackendFactory::new(true);
        let second = cx.update(|window, cx| {
            cx.new(|cx| {
                BrowserView::new(
                    backend_factory(&second_factory),
                    "https://two.example".to_string(),
                    window,
                    cx,
                )
            })
        });

        first.update(cx, |_, cx| assert!(BrowserView::is_session_owner(cx)));
        second.update(cx, |_, cx| {
            assert!(
                !BrowserView::is_session_owner(cx),
                "the owner slot is taken by the first view"
            )
        });

        second.update_in(cx, |view, window, cx| {
            view.navigate_to("https://second.example".to_string(), window, cx);
        });
        cx.executor().advance_clock(SESSION_SAVE_DEBOUNCE);
        cx.run_until_parked();
        assert!(
            cx.update(|_, cx| session::restore(cx)).is_none(),
            "a non-owner view never writes the session"
        );

        first.update_in(cx, |view, window, cx| {
            view.navigate_to("https://first.example".to_string(), window, cx);
        });
        cx.executor().advance_clock(SESSION_SAVE_DEBOUNCE);
        cx.run_until_parked();
        let saved = cx.update(|_, cx| session::restore(cx)).unwrap();
        assert_eq!(saved.tabs[0].url, "https://first.example");
    }

    #[gpui::test]
    async fn test_moving_the_view_between_panes_keeps_session_ownership(cx: &mut TestAppContext) {
        let app_state = init_test(cx);
        let project = Project::test(app_state.fs.clone(), [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

        let factory = StubBackendFactory::new(true);
        workspace.update_in(cx, |workspace, window, cx| {
            let factory = factory.clone();
            BrowserView::open_with_factory(workspace, window, cx, move || factory.create_backend());
        });
        cx.run_until_parked();
        let view = workspace
            .update(cx, |workspace, cx| {
                workspace.items_of_type::<BrowserView>(cx).next()
            })
            .unwrap();

        workspace.update_in(cx, |workspace, window, cx| {
            let source = workspace.active_pane().clone();
            let destination =
                workspace.split_pane(source.clone(), workspace::SplitDirection::Right, window, cx);
            workspace::move_item(&source, &destination, view.entity_id(), 0, true, window, cx);
        });
        cx.run_until_parked();

        // Removal from the source pane released the owner slot, but adding to
        // the destination pane re-claimed it within the same move.
        view.update(cx, |_, cx| assert!(BrowserView::is_session_owner(cx)));

        view.update_in(cx, |view, window, cx| {
            view.navigate_to("https://moved.example".to_string(), window, cx);
        });
        cx.executor().advance_clock(SESSION_SAVE_DEBOUNCE);
        cx.run_until_parked();
        let saved = cx.update(|_, cx| session::restore(cx)).unwrap();
        assert_eq!(
            saved.tabs[0].url, "https://moved.example",
            "the moved view still writes the session"
        );
    }

    #[gpui::test]
    async fn test_incognito_views_are_excluded_from_persistence(cx: &mut TestAppContext) {
        init_test(cx);
        write_saved_session(
            session::SerializedBrowserTabs {
                tabs: vec![session::SerializedTab {
                    url: "https://saved.example".to_string(),
                    title: String::new(),
                    is_new_tab_page: false,
                    is_pinned: false,
                    favicon_url: None,
                }],
                active_index: 0,
            },
            cx,
        )
        .await;

        let factory = StubBackendFactory::new(true);
        let (view, cx) = cx.add_window_view(|window, cx| {
            BrowserView::new_incognito(
                backend_factory(&factory),
                "https://incognito.example".to_string(),
                window,
                cx,
            )
        });
        cx.run_until_parked();

        view.update(cx, |view, cx| {
            assert!(view.is_incognito);
            assert!(
                !BrowserView::is_session_owner(cx),
                "incognito views never claim the owner slot"
            );
            assert_eq!(view.tabs.len(), 1);
            assert_eq!(view.url(), "https://incognito.example");
        });

        view.update_in(cx, |view, window, cx| {
            view.navigate_to("https://secret.example".to_string(), window, cx);
        });
        cx.executor().advance_clock(SESSION_SAVE_DEBOUNCE);
        cx.run_until_parked();

        let saved = cx.update(|_, cx| session::restore(cx)).unwrap();
        assert_eq!(
            saved.tabs[0].url, "https://saved.example",
            "incognito browsing never touches the saved session"
        );
    }

    #[gpui::test]
    async fn test_new_tab_pages_round_trip_through_the_session(cx: &mut TestAppContext) {
        init_test(cx);
        write_saved_session(
            session::SerializedBrowserTabs {
                tabs: vec![
                    session::SerializedTab {
                        url: "https://a.example".to_string(),
                        title: "A".to_string(),
                        is_new_tab_page: false,
                        is_pinned: false,
                        favicon_url: None,
                    },
                    session::SerializedTab {
                        url: String::new(),
                        title: String::new(),
                        is_new_tab_page: true,
                        is_pinned: false,
                        favicon_url: None,
                    },
                ],
                active_index: 1,
            },
            cx,
        )
        .await;

        let factory = StubBackendFactory::new(true);
        let (view, cx) = cx.add_window_view(|window, cx| {
            BrowserView::restore_or_new(backend_factory(&factory), window, cx)
        });
        cx.run_until_parked();

        view.update(cx, |view, _| {
            assert_eq!(view.tabs.len(), 2);
            assert!(!view.tabs[0].is_new_tab_page());
            assert!(view.tabs[1].is_new_tab_page());
            assert_eq!(view.active_tab_index, 1);
        });
        assert_eq!(
            factory.controller(1).started_with(),
            None,
            "a restored new-tab page still starts no engine browser"
        );

        // The restored new-tab page saves back as one.
        view.update_in(cx, |view, window, cx| {
            view.navigate_to("https://b.example".to_string(), window, cx);
        });
        cx.executor().advance_clock(SESSION_SAVE_DEBOUNCE);
        cx.run_until_parked();
        let saved = cx.update(|_, cx| session::restore(cx)).unwrap();
        assert!(!saved.tabs[1].is_new_tab_page);
        assert_eq!(saved.tabs[1].url, "https://b.example");
    }

    #[gpui::test]
    async fn test_engine_page_loads_are_recorded_in_history(cx: &mut TestAppContext) {
        init_test(cx);
        let (view, cx, factory) = stub_view(true, "https://one.example", cx);
        let controller = factory.controller(0);

        controller.script_events([
            TabBackendEvent::AddressChanged("https://one.example/docs".into()),
            TabBackendEvent::TitleChanged("One Docs".into()),
            TabBackendEvent::FaviconUrlsChanged(vec!["https://one.example/icon.png".into()]),
        ]);
        pump(cx);

        view.update(cx, |view, cx| {
            let entries = view.history.read(cx).entries();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].url, "https://one.example/docs");
            assert_eq!(entries[0].title, "One Docs");
            assert_eq!(
                entries[0].visit_count, 1,
                "one drained load records one visit, favicon updates none"
            );
        });

        // Returning to the page later bumps its visit count.
        controller.script_events([TabBackendEvent::AddressChanged(
            "https://one.example/docs".into(),
        )]);
        pump(cx);
        view.update(cx, |view, cx| {
            assert_eq!(view.history.read(cx).entries()[0].visit_count, 2);
        });
    }

    #[gpui::test]
    async fn test_incognito_views_do_not_record_history(cx: &mut TestAppContext) {
        init_test(cx);
        let factory = StubBackendFactory::new(true);
        let (view, cx) = cx.add_window_view(|window, cx| {
            BrowserView::new_incognito(
                backend_factory(&factory),
                "https://incognito.example".to_string(),
                window,
                cx,
            )
        });
        cx.run_until_parked();

        factory.controller(0).script_events([
            TabBackendEvent::AddressChanged("https://secret.example".into()),
            TabBackendEvent::TitleChanged("Secret".into()),
        ]);
        pump(cx);

        view.update(cx, |view, cx| {
            assert!(
                view.history.read(cx).entries().is_empty(),
                "incognito page loads never reach browsing history"
            );
        });
    }

    #[gpui::test]
    async fn test_history_persists_across_restart(cx: &mut TestAppContext) {
        init_test(cx);
        let history = cx.update(BrowserHistory::global);
        history.update(cx, |history, cx| {
            history.record_visit("https://example.com", "Example", cx);
        });
        cx.executor()
            .advance_clock(crate::history::HISTORY_SAVE_DEBOUNCE);
        cx.run_until_parked();

        // A fresh entity — as a restarted app would create — restores the
        // persisted entries.
        let restored = cx.update(|cx| cx.new(BrowserHistory::new));
        restored.update(cx, |history, _| {
            assert_eq!(history.entries().len(), 1);
            assert_eq!(history.entries()[0].url, "https://example.com");
            assert_eq!(history.entries()[0].title, "Example");
        });
    }

    #[gpui::test]
    async fn test_quit_flushes_the_pending_history_save(cx: &mut TestAppContext) {
        init_test(cx);
        let history = cx.update(BrowserHistory::global);
        history.update(cx, |history, cx| {
            history.record_visit("https://example.com", "Example", cx);
        });
        assert!(
            cx.update(|cx| session::restore_history(cx)).is_none(),
            "the debounced write is still pending at quit"
        );

        cx.update(|cx| cx.shutdown());

        let entries = cx.update(|cx| session::restore_history(cx)).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].url, "https://example.com");
    }

    #[gpui::test]
    async fn test_omnibox_suggests_from_history_and_navigates(cx: &mut TestAppContext) {
        init_test(cx);
        let (view, cx, factory) = stub_view(true, "https://start.example", cx);
        let controller = factory.controller(0);

        view.update(cx, |view, cx| {
            view.history.update(cx, |history, cx| {
                history.record_visit("https://example.com/docs", "Example Docs", cx);
                history.record_visit("https://unrelated.example", "Unrelated", cx);
            });
        });

        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });
        cx.dispatch_action(FocusOmnibox);
        cx.simulate_input("docs");
        view.update(cx, |view, cx| {
            assert!(
                !view.omnibox.read(cx).is_dropdown_open(),
                "the search is debounced; nothing opens mid-typing"
            );
        });
        cx.executor().advance_clock(crate::omnibox::SEARCH_DEBOUNCE);
        cx.run_until_parked();

        view.update(cx, |view, cx| {
            let omnibox = view.omnibox.read(cx);
            assert!(omnibox.is_dropdown_open());
            let suggestions = omnibox.suggestions();
            assert_eq!(suggestions.len(), 2, "search row plus the one match");
            assert!(
                matches!(&suggestions[0], OmniboxSuggestion::Search(query) if query == "docs"),
                "non-URL text defaults to the search row"
            );
            assert!(matches!(
                &suggestions[1],
                OmniboxSuggestion::History { url, .. } if url == "https://example.com/docs"
            ));
            assert_eq!(omnibox.selected_index(), 0);
        });

        // Arrow down onto the history row and commit: the omnibox navigates
        // to the remembered page, not to a search.
        cx.dispatch_action(zed_actions::editor::MoveDown);
        view.update(cx, |view, cx| {
            assert_eq!(view.omnibox.read(cx).selected_index(), 1);
        });
        controller.take_commands();
        cx.dispatch_action(menu::Confirm);
        cx.run_until_parked();

        view.update(cx, |view, cx| {
            assert_eq!(view.url(), "https://example.com/docs");
            assert!(
                !view.omnibox.read(cx).is_dropdown_open(),
                "navigating closes the dropdown"
            );
        });
        assert!(
            controller
                .take_commands()
                .contains(&RecordedCommand::Navigate {
                    url: "https://example.com/docs".into(),
                }),
            "the selected suggestion drives the engine navigation"
        );
    }

    #[gpui::test]
    async fn test_omnibox_url_like_text_keeps_enter_on_the_url(cx: &mut TestAppContext) {
        init_test(cx);
        let (view, cx, factory) = stub_view(true, "https://start.example", cx);
        let controller = factory.controller(0);

        view.update(cx, |view, cx| {
            view.history.update(cx, |history, cx| {
                history.record_visit("https://example.com", "Example", cx);
            });
        });

        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });
        cx.dispatch_action(FocusOmnibox);
        cx.simulate_input("example.com");
        cx.executor().advance_clock(crate::omnibox::SEARCH_DEBOUNCE);
        cx.run_until_parked();

        view.update(cx, |view, cx| {
            let omnibox = view.omnibox.read(cx);
            assert!(omnibox.is_dropdown_open());
            let suggestions = omnibox.suggestions();
            assert!(
                matches!(&suggestions[0], OmniboxSuggestion::Url(text) if text == "example.com"),
                "URL-like text keeps the URL row first, where enter has always gone"
            );
            assert!(matches!(&suggestions[1], OmniboxSuggestion::Search(_)));
        });

        controller.take_commands();
        cx.dispatch_action(menu::Confirm);
        cx.run_until_parked();
        assert!(
            controller
                .take_commands()
                .contains(&RecordedCommand::Navigate {
                    url: "https://example.com".into(),
                }),
            "enter with the dropdown open still resolves URL-like text as a URL"
        );
    }

    #[gpui::test]
    async fn test_omnibox_escape_closes_the_dropdown_and_restores_the_url(cx: &mut TestAppContext) {
        init_test(cx);
        let (view, cx, factory) = stub_view(true, "https://start.example", cx);
        let controller = factory.controller(0);

        view.update(cx, |view, cx| {
            view.history.update(cx, |history, cx| {
                history.record_visit("https://example.com/docs", "Example Docs", cx);
            });
        });
        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });
        cx.dispatch_action(FocusOmnibox);
        cx.simulate_input("docs");
        cx.executor().advance_clock(crate::omnibox::SEARCH_DEBOUNCE);
        cx.run_until_parked();
        view.update(cx, |view, cx| {
            assert!(view.omnibox.read(cx).is_dropdown_open());
        });

        controller.take_commands();
        cx.dispatch_action(menu::Cancel);
        cx.run_until_parked();

        view.update(cx, |view, cx| {
            let omnibox = view.omnibox.read(cx);
            assert!(!omnibox.is_dropdown_open());
            assert_eq!(omnibox.editor_text(cx), "https://start.example");
        });
        assert_eq!(
            controller.take_commands(),
            vec![],
            "cancelling never navigates"
        );
    }
}
