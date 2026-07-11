//! The browser view: the single per-workspace pane item that owns browser
//! tabs and their chrome (ADR-0004). M1 scope: one browser tab, no chrome —
//! the internal tab strip arrives with ticket #9 and the navigation chrome
//! (omnibox, back/forward/reload buttons) with ticket #7.
//!
//! Everything here sits above the two seams: engine communication flows
//! through the tab-backend trait and frame presentation through the frame
//! presenter, so this file is platform-neutral and testable with a scripted
//! stub backend.

use crate::frame_presenter::{FramePresenter, SoftwarePresenter};
use crate::tab_backend::{TabBackend, TabBackendEvent};
use gpui::{
    App, Bounds, Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, KeyUpEvent,
    Keystroke, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Render,
    ScrollWheelEvent, SharedString, Window, actions, canvas, div,
};
use ui::prelude::*;
use workspace::Workspace;
use workspace::item::{Item, ItemEvent, TabTooltipContent};

actions!(
    browser,
    [
        /// Opens the browser view in the workspace.
        OpenBrowser
    ]
);

pub const DEFAULT_URL: &str = "https://zed.dev";

fn default_url() -> String {
    // Interim escape hatch: until the omnibox lands (ticket #7) this env var
    // is the only way to point the browser at a different page.
    std::env::var("ZED_BROWSER_URL").unwrap_or_else(|_| DEFAULT_URL.to_string())
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
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
    backend: Box<dyn TabBackend>,
    presenter: Box<dyn FramePresenter>,
    url: String,
    title: String,
    is_loading: bool,
    can_go_back: bool,
    can_go_forward: bool,
    engine_error: Option<String>,
    /// Last viewport pushed to the engine: logical width and height, plus the
    /// scale factor in thousandths (to keep the key comparable).
    last_viewport: Option<(u32, u32, u32)>,
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
        backend: Box<dyn TabBackend>,
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
                .update(cx, |_, window, _| this.presenter.release(window))
                .ok();
        })
        .detach();

        Self {
            focus_handle: cx.focus_handle(),
            backend,
            presenter: Box::new(SoftwarePresenter::new()),
            url: initial_url,
            title: String::new(),
            is_loading: false,
            can_go_back: false,
            can_go_forward: false,
            engine_error: None,
            last_viewport: None,
            content_bounds: Bounds::default(),
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    pub fn open(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
        Self::open_with_backend(workspace, window, cx, || {
            Box::new(crate::tab::CefTab::new())
        });
    }

    fn open_with_backend(
        workspace: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Workspace>,
        backend: impl FnOnce() -> Box<dyn TabBackend>,
    ) {
        // One browser view per workspace (ADR-0004); reopening focuses it.
        let existing = workspace.items_of_type::<BrowserView>(cx).next();
        if let Some(existing) = existing {
            workspace.activate_item(&existing, true, true, window, cx);
            return;
        }
        let view = cx.new(|cx| BrowserView::new(backend(), default_url(), window, cx));
        workspace.add_item_to_active_pane(Box::new(view), None, true, window, cx);
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn is_loading(&self) -> bool {
        self.is_loading
    }

    pub fn can_go_back(&self) -> bool {
        self.can_go_back
    }

    pub fn can_go_forward(&self) -> bool {
        self.can_go_forward
    }

    /// Drain pending engine events into view state. Invoked after every
    /// message-pump iteration; tests call it directly after scripting stub
    /// events.
    fn drain_engine_events(&mut self, cx: &mut Context<Self>) {
        // The engine may have just become ready (first pumps after init);
        // kick a render so the browser gets created with real bounds.
        if !self.backend.is_started() && self.engine_error.is_none() && self.backend.engine_ready()
        {
            cx.notify();
        }

        let mut item_changed = false;
        let mut needs_notify = false;
        while let Some(event) = self.backend.try_recv_event() {
            match event {
                TabBackendEvent::Created => {}
                TabBackendEvent::AddressChanged(url) => {
                    self.url = url;
                    item_changed = true;
                }
                TabBackendEvent::TitleChanged(title) => {
                    self.title = title;
                    item_changed = true;
                }
                TabBackendEvent::LoadingStateChanged {
                    is_loading,
                    can_go_back,
                    can_go_forward,
                } => {
                    self.is_loading = is_loading;
                    self.can_go_back = can_go_back;
                    self.can_go_forward = can_go_forward;
                    needs_notify = true;
                }
                TabBackendEvent::LoadingProgress(_) => {}
                TabBackendEvent::FaviconUrlsChanged(_) => {
                    // Favicon pipeline is M2 (ticket #11).
                }
                TabBackendEvent::FrameReady => needs_notify = true,
                TabBackendEvent::LoadError { url, error_text } => {
                    log::warn!("[browser] load error for {url}: {error_text}");
                }
            }
        }

        if item_changed {
            cx.emit(ItemEvent::UpdateTab);
        }
        if item_changed || needs_notify {
            cx.notify();
        }
    }

    /// Push the content size and display scale factor into the engine,
    /// creating the engine browser the first time real bounds and a ready
    /// engine coincide. Called from the content canvas during every draw of
    /// this view, so it always sees the laid-out bounds — including the final
    /// frame of a resize.
    fn handle_content_bounds(&mut self, bounds: Bounds<Pixels>, scale_factor: f32) {
        self.content_bounds = bounds;
        let width = f32::from(bounds.size.width) as u32;
        let height = f32::from(bounds.size.height) as u32;
        if width == 0 || height == 0 {
            return;
        }

        let viewport_key = (width, height, (scale_factor * 1000.0) as u32);
        if !self.backend.is_started() {
            if self.engine_error.is_some() || !self.backend.engine_ready() {
                return;
            }
            self.backend.set_viewport(width, height, scale_factor);
            self.last_viewport = Some(viewport_key);
            match self.backend.start(&self.url) {
                Ok(()) => self.backend.set_focus(true),
                Err(error) => {
                    log::error!("[browser] failed to start engine for {}: {error:#}", self.url);
                    self.engine_error = Some(format!("{error:#}"));
                }
            }
        } else if self.last_viewport != Some(viewport_key) {
            self.last_viewport = Some(viewport_key);
            self.backend.set_viewport(width, height, scale_factor);
        }
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
        self.backend.set_focus(true);
        self.backend.send_mouse_down(
            event.position - self.content_bounds.origin,
            event.button,
            event.click_count,
            event.modifiers,
        );
    }

    fn handle_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.backend.send_mouse_up(
            event.position - self.content_bounds.origin,
            event.button,
            event.modifiers,
        );
    }

    fn handle_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.backend.send_mouse_move(
            event.position - self.content_bounds.origin,
            event.pressed_button,
            event.modifiers,
        );
    }

    fn handle_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.backend.send_scroll_wheel(
            event.position - self.content_bounds.origin,
            event.delta,
            event.modifiers,
        );
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
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if is_app_keystroke(&event.keystroke) {
            return;
        }
        self.backend.send_key_down(&event.keystroke, event.is_held);
        cx.stop_propagation();
    }

    fn handle_key_up(&mut self, event: &KeyUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if is_app_keystroke(&event.keystroke) {
            return;
        }
        self.backend.send_key_up(&event.keystroke);
        cx.stop_propagation();
    }

    fn render_placeholder(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let message: SharedString = match &self.engine_error {
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
}

impl Render for BrowserView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(output) = self.backend.take_paint_output() {
            self.presenter.present(output);
        }
        let frame = self.presenter.render_frame(window);
        let has_frame = frame.is_some();

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

        div()
            .id("browser-view")
            .key_context("BrowserView")
            .track_focus(&self.focus_handle)
            .size_full()
            .relative()
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
            .on_key_down(cx.listener(Self::handle_key_down))
            .on_key_up(cx.listener(Self::handle_key_up))
            .child(bounds_tracker)
            .when_some(frame, |this, frame| this.child(frame))
            .when(!has_frame, |this| this.child(self.render_placeholder(cx)))
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

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        let title = self.title.trim();
        if !title.is_empty() {
            title.to_string().into()
        } else if !self.url.is_empty() {
            self.url.clone().into()
        } else {
            "Browser".into()
        }
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        // Upstream's globe glyph; the icon set has no dedicated Globe variant.
        Some(Icon::new(IconName::ToolWeb))
    }

    fn tab_tooltip_content(&self, _cx: &App) -> Option<TabTooltipContent> {
        if self.url.is_empty() {
            None
        } else {
            Some(TabTooltipContent::Text(self.url.clone().into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tab_backend::{PaintOutput, SoftwareFrame};
    use anyhow::Result;
    use gpui::{Modifiers, Point, ScrollDelta, TestAppContext, TouchPhase, point, size};
    use parking_lot::Mutex;
    use project::Project;
    use std::collections::VecDeque;
    use std::sync::Arc;
    use workspace::AppState;

    #[derive(Debug, Clone, PartialEq)]
    enum RecordedInput {
        MouseDown {
            position: Point<Pixels>,
            button: MouseButton,
            click_count: usize,
        },
        MouseUp {
            position: Point<Pixels>,
            button: MouseButton,
        },
        MouseMove {
            position: Point<Pixels>,
        },
        ScrollWheel {
            position: Point<Pixels>,
            /// x, y, and whether the delta was line-based (`ScrollDelta` has
            /// no `PartialEq`).
            delta: (f32, f32, bool),
        },
        KeyDown {
            key: String,
            is_held: bool,
        },
        KeyUp {
            key: String,
        },
    }

    #[derive(Default)]
    struct StubState {
        engine_ready: bool,
        started_with: Option<String>,
        viewports: Vec<(u32, u32, u32)>,
        focus_calls: Vec<bool>,
        events: VecDeque<TabBackendEvent>,
        paint_output: Option<PaintOutput>,
        inputs: Vec<RecordedInput>,
    }

    #[derive(Clone)]
    struct StubBackend(Arc<Mutex<StubState>>);

    impl StubBackend {
        fn new(engine_ready: bool) -> (Self, Arc<Mutex<StubState>>) {
            let state = Arc::new(Mutex::new(StubState {
                engine_ready,
                ..Default::default()
            }));
            (Self(state.clone()), state)
        }
    }

    impl TabBackend for StubBackend {
        fn engine_ready(&self) -> bool {
            self.0.lock().engine_ready
        }

        fn start(&mut self, url: &str) -> Result<()> {
            self.0.lock().started_with = Some(url.to_string());
            Ok(())
        }

        fn is_started(&self) -> bool {
            self.0.lock().started_with.is_some()
        }

        fn navigate(&mut self, _url: &str) {}
        fn reload(&mut self) {}
        fn go_back(&mut self) {}
        fn go_forward(&mut self) {}

        fn set_viewport(&mut self, width: u32, height: u32, scale_factor: f32) {
            self.0
                .lock()
                .viewports
                .push((width, height, (scale_factor * 1000.0) as u32));
        }

        fn set_focus(&mut self, focused: bool) {
            self.0.lock().focus_calls.push(focused);
        }

        fn send_mouse_down(
            &mut self,
            position: Point<Pixels>,
            button: MouseButton,
            click_count: usize,
            _modifiers: Modifiers,
        ) {
            self.0.lock().inputs.push(RecordedInput::MouseDown {
                position,
                button,
                click_count,
            });
        }

        fn send_mouse_up(
            &mut self,
            position: Point<Pixels>,
            button: MouseButton,
            _modifiers: Modifiers,
        ) {
            self.0
                .lock()
                .inputs
                .push(RecordedInput::MouseUp { position, button });
        }

        fn send_mouse_move(
            &mut self,
            position: Point<Pixels>,
            _pressed_button: Option<MouseButton>,
            _modifiers: Modifiers,
        ) {
            self.0
                .lock()
                .inputs
                .push(RecordedInput::MouseMove { position });
        }

        fn send_scroll_wheel(
            &mut self,
            position: Point<Pixels>,
            delta: ScrollDelta,
            _modifiers: Modifiers,
        ) {
            let delta = match delta {
                ScrollDelta::Pixels(delta) => (f32::from(delta.x), f32::from(delta.y), false),
                ScrollDelta::Lines(delta) => (delta.x, delta.y, true),
            };
            self.0
                .lock()
                .inputs
                .push(RecordedInput::ScrollWheel { position, delta });
        }

        fn send_key_down(&mut self, keystroke: &Keystroke, is_held: bool) {
            self.0.lock().inputs.push(RecordedInput::KeyDown {
                key: keystroke.key.clone(),
                is_held,
            });
        }

        fn send_key_up(&mut self, keystroke: &Keystroke) {
            self.0.lock().inputs.push(RecordedInput::KeyUp {
                key: keystroke.key.clone(),
            });
        }

        fn close(&mut self) {}

        fn try_recv_event(&mut self) -> Option<TabBackendEvent> {
            self.0.lock().events.pop_front()
        }

        fn take_paint_output(&mut self) -> Option<PaintOutput> {
            self.0.lock().paint_output.take()
        }
    }

    fn init_test(cx: &mut TestAppContext) -> Arc<AppState> {
        cx.update(AppState::test)
    }

    #[gpui::test]
    async fn test_engine_events_update_item_state(cx: &mut TestAppContext) {
        init_test(cx);
        let (backend, state) = StubBackend::new(true);
        let (view, cx) = cx.add_window_view(|window, cx| {
            BrowserView::new(Box::new(backend), "https://example.com".into(), window, cx)
        });
        cx.run_until_parked();

        // The first render starts the engine tab with the initial URL and a
        // real viewport.
        let window_scale_key = (cx.update(|window, _| window.scale_factor()) * 1000.0) as u32;
        {
            let state = state.lock();
            assert_eq!(state.started_with.as_deref(), Some("https://example.com"));
            assert!(!state.viewports.is_empty());
            let (width, height, scale_key) = state.viewports[0];
            assert!(width > 0 && height > 0);
            assert_eq!(scale_key, window_scale_key);
            assert_eq!(state.focus_calls, vec![true]);
        }

        state.lock().events.extend([
            TabBackendEvent::AddressChanged("https://example.com/docs".into()),
            TabBackendEvent::TitleChanged("Example Docs".into()),
            TabBackendEvent::LoadingStateChanged {
                is_loading: false,
                can_go_back: true,
                can_go_forward: false,
            },
        ]);
        view.update(cx, |view, cx| view.drain_engine_events(cx));

        view.update(cx, |view, cx| {
            assert_eq!(view.url(), "https://example.com/docs");
            assert_eq!(view.title(), "Example Docs");
            assert!(view.can_go_back());
            assert!(!view.can_go_forward());
            assert!(!view.is_loading());
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
        let (backend, state) = StubBackend::new(true);
        let (view, cx) = cx.add_window_view(|window, cx| {
            BrowserView::new(Box::new(backend), DEFAULT_URL.into(), window, cx)
        });
        cx.run_until_parked();
        assert!(!view.update(cx, |view, _| view.presenter.has_frame()));

        let (width, height) = (4, 4);
        state.lock().paint_output = Some(PaintOutput::Software(SoftwareFrame {
            width,
            height,
            bgra: vec![0xff; (width * height * 4) as usize],
        }));
        state.lock().events.push_back(TabBackendEvent::FrameReady);
        view.update(cx, |view, cx| view.drain_engine_events(cx));
        cx.run_until_parked();

        assert!(
            view.update(cx, |view, _| view.presenter.has_frame()),
            "presenter should hold the frame after the FrameReady render"
        );
        assert!(
            state.lock().paint_output.is_none(),
            "render should have taken the paint output from the backend"
        );
    }

    #[gpui::test]
    async fn test_resize_propagates_scaled_viewport(cx: &mut TestAppContext) {
        init_test(cx);
        let (backend, state) = StubBackend::new(true);
        let (_view, cx) = cx.add_window_view(|window, cx| {
            BrowserView::new(Box::new(backend), DEFAULT_URL.into(), window, cx)
        });
        cx.run_until_parked();
        let viewports_before = state.lock().viewports.len();

        cx.simulate_resize(size(px(500.), px(400.)));
        cx.run_until_parked();

        let window_scale_key = (cx.update(|window, _| window.scale_factor()) * 1000.0) as u32;
        let state = state.lock();
        assert!(state.viewports.len() > viewports_before);
        let (width, height, scale_key) = *state.viewports.last().unwrap();
        assert_eq!(
            (width, height),
            (500, 400),
            "engine viewport tracks the resized logical bounds"
        );
        assert_eq!(scale_key, window_scale_key);
    }

    #[gpui::test]
    async fn test_engine_not_ready_defers_start_until_a_pump(cx: &mut TestAppContext) {
        init_test(cx);
        let (backend, state) = StubBackend::new(false);
        let (view, cx) = cx.add_window_view(|window, cx| {
            BrowserView::new(Box::new(backend), DEFAULT_URL.into(), window, cx)
        });
        cx.run_until_parked();
        assert_eq!(state.lock().started_with, None);

        // Engine comes up; the post-pump drain notices and triggers a render
        // that starts the tab.
        state.lock().engine_ready = true;
        view.update(cx, |view, cx| view.drain_engine_events(cx));
        cx.run_until_parked();
        assert_eq!(state.lock().started_with.as_deref(), Some(DEFAULT_URL));
    }

    #[gpui::test]
    async fn test_plain_keys_route_to_the_page_but_app_chords_do_not(cx: &mut TestAppContext) {
        init_test(cx);
        let (backend, state) = StubBackend::new(true);
        let (view, cx) = cx.add_window_view(|window, cx| {
            BrowserView::new(Box::new(backend), DEFAULT_URL.into(), window, cx)
        });
        cx.run_until_parked();
        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });

        cx.simulate_keystrokes("a");
        assert_eq!(
            state.lock().inputs,
            vec![RecordedInput::KeyDown {
                key: "a".into(),
                is_held: false,
            }],
            "an unmodified printable key is forwarded to the page"
        );

        state.lock().inputs.clear();
        cx.simulate_keystrokes("ctrl-t ctrl-shift-r");
        assert_eq!(
            state.lock().inputs,
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

        let (backend, state) = StubBackend::new(true);
        workspace.update_in(cx, |workspace, window, cx| {
            BrowserView::open_with_backend(workspace, window, cx, || Box::new(backend));
        });
        cx.run_until_parked();

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

        state.lock().inputs.clear();
        let click_offset = point(px(15.), px(25.));
        cx.simulate_click(content_origin + click_offset, Modifiers::default());
        assert_eq!(
            state.lock().inputs,
            vec![
                RecordedInput::MouseDown {
                    position: click_offset,
                    button: MouseButton::Left,
                    click_count: 1,
                },
                RecordedInput::MouseUp {
                    position: click_offset,
                    button: MouseButton::Left,
                },
            ],
            "click coordinates are translated by the pane offset"
        );

        state.lock().inputs.clear();
        cx.simulate_event(ScrollWheelEvent {
            position: content_origin + click_offset,
            delta: ScrollDelta::Lines(point(0.0, -2.0)),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
        });
        assert_eq!(
            state.lock().inputs,
            vec![RecordedInput::ScrollWheel {
                position: click_offset,
                delta: (0.0, -2.0, true),
            }],
            "scroll coordinates are translated by the pane offset"
        );
    }

    #[gpui::test]
    async fn test_open_is_a_workspace_singleton(cx: &mut TestAppContext) {
        let app_state = init_test(cx);
        let project = Project::test(app_state.fs.clone(), [], cx).await;
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            Workspace::test_new(project.clone(), window, cx)
        });

        workspace.update_in(cx, |workspace, window, cx| {
            BrowserView::open_with_backend(workspace, window, cx, || {
                Box::new(StubBackend::new(false).0)
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
            BrowserView::open_with_backend(workspace, window, cx, || {
                Box::new(StubBackend::new(false).0)
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
}
