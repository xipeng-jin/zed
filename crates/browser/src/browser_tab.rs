//! Per-browser-tab state: one web page open inside the browser view's
//! internal tab strip (CONTEXT.md, "browser tab" — distinct from a pane tab).
//!
//! Each browser tab owns both halves of its engine communication: its tab
//! backend (commands in, events out) and its frame presenter (paint output to
//! GPUI element). Because presented surfaces live per tab, switching the
//! active tab swaps whole presenters and one tab's frames can never bleed
//! into another's.

use crate::frame_presenter::{FramePresenter, SoftwarePresenter};
use crate::tab_backend::{TabBackend, TabBackendEvent};
use gpui::{
    AnyElement, Keystroke, Modifiers, MouseButton, Pixels, Point, ScrollDelta, SharedString,
    SharedUri, Window,
};

/// A closed browser tab, remembered on the browser view's reopen stack.
pub(crate) struct ClosedTab {
    pub url: String,
    pub title: String,
    pub favicon_url: Option<SharedUri>,
    pub is_pinned: bool,
}

/// What a drain of pending engine events changed, so the view can decide
/// whether to update the pane tab and re-render.
#[derive(Default)]
pub(crate) struct DrainedChanges {
    /// Identity shown in chrome changed: URL, title, or favicon.
    pub identity_changed: bool,
    /// Anything else the UI reflects changed: loading state, a new frame.
    pub needs_notify: bool,
}

pub(crate) struct BrowserTab {
    /// Stable identity across reorders (pinning sorts the tab list) and used
    /// for tab strip element ids.
    pub id: usize,
    backend: Box<dyn TabBackend>,
    presenter: Box<dyn FramePresenter>,
    url: String,
    title: String,
    favicon_url: Option<SharedUri>,
    is_loading: bool,
    can_go_back: bool,
    can_go_forward: bool,
    is_pinned: bool,
    engine_error: Option<String>,
    /// Last viewport pushed to the engine: logical width and height, plus the
    /// scale factor in thousandths (to keep the key comparable).
    last_viewport: Option<(u32, u32, u32)>,
}

impl BrowserTab {
    pub fn new(id: usize, backend: Box<dyn TabBackend>, url: String) -> Self {
        Self {
            id,
            backend,
            presenter: Box::new(SoftwarePresenter::new()),
            url,
            title: String::new(),
            favicon_url: None,
            is_loading: false,
            can_go_back: false,
            can_go_forward: false,
            is_pinned: false,
            engine_error: None,
            last_viewport: None,
        }
    }

    /// Rebuild a tab from the reopen stack. The engine browser is created
    /// lazily, the next time this tab is active with real bounds.
    pub fn restore(id: usize, backend: Box<dyn TabBackend>, closed: ClosedTab) -> Self {
        let mut tab = Self::new(id, backend, closed.url);
        tab.title = closed.title;
        tab.favicon_url = closed.favicon_url;
        tab.is_pinned = closed.is_pinned;
        tab
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn favicon_url(&self) -> Option<SharedUri> {
        self.favicon_url.clone()
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

    pub fn is_pinned(&self) -> bool {
        self.is_pinned
    }

    pub fn set_pinned(&mut self, pinned: bool) {
        self.is_pinned = pinned;
    }

    pub fn engine_error(&self) -> Option<&str> {
        self.engine_error.as_deref()
    }

    /// Display title for tab strips and pane tabs: the page title, else the
    /// URL, else `fallback`.
    pub fn display_title(&self, fallback: &str) -> SharedString {
        let title = self.title.trim();
        if !title.is_empty() {
            title.to_string().into()
        } else if !self.url.is_empty() {
            self.url.clone().into()
        } else {
            fallback.to_string().into()
        }
    }

    pub fn to_closed_tab(&self) -> ClosedTab {
        ClosedTab {
            url: self.url.clone(),
            title: self.title.clone(),
            favicon_url: self.favicon_url.clone(),
            is_pinned: self.is_pinned,
        }
    }

    /// Whether the tab is waiting to create its engine browser: the engine is
    /// ready but the tab has not started (typically because real bounds have
    /// not been seen yet).
    pub fn wants_start(&self) -> bool {
        !self.backend.is_started() && self.engine_error.is_none() && self.backend.engine_ready()
    }

    /// Push the content size and display scale factor into the engine,
    /// creating the engine browser the first time real bounds and a ready
    /// engine coincide. Called for the active tab during every draw of the
    /// browser view, so it always sees the laid-out bounds — including the
    /// final frame of a resize and the first draw after a tab switch.
    pub fn sync_viewport(&mut self, width: u32, height: u32, scale_factor: f32) {
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

    /// Drain pending engine events into tab state.
    pub fn drain_events(&mut self) -> DrainedChanges {
        let mut changes = DrainedChanges::default();
        while let Some(event) = self.backend.try_recv_event() {
            match event {
                TabBackendEvent::Created => {}
                TabBackendEvent::AddressChanged(url) => {
                    if self.url != url {
                        // The engine does not always re-announce favicons when
                        // returning to a page (e.g. history traversal); drop
                        // the old page's icon rather than show it for the new
                        // one.
                        self.favicon_url = None;
                    }
                    self.url = url;
                    changes.identity_changed = true;
                }
                TabBackendEvent::TitleChanged(title) => {
                    self.title = title;
                    changes.identity_changed = true;
                }
                TabBackendEvent::LoadingStateChanged {
                    is_loading,
                    can_go_back,
                    can_go_forward,
                } => {
                    self.is_loading = is_loading;
                    self.can_go_back = can_go_back;
                    self.can_go_forward = can_go_forward;
                    changes.needs_notify = true;
                }
                TabBackendEvent::LoadingProgress(_) => {}
                TabBackendEvent::FaviconUrlsChanged(urls) => {
                    // Minimal favicon display: hand the first candidate URL to
                    // gpui's image loader. The cached pipeline with sizing
                    // preferences is M2 (ticket #11).
                    self.favicon_url = urls.first().map(|url| SharedUri::from(url.clone()));
                    changes.identity_changed = true;
                }
                TabBackendEvent::FrameReady => changes.needs_notify = true,
                TabBackendEvent::LoadError { url, error_text } => {
                    log::warn!("[browser] load error for {url}: {error_text}");
                }
            }
        }
        changes
    }

    /// Navigate the engine to `url` (already heuristic-resolved). The address
    /// is reflected optimistically; the engine's `AddressChanged` confirms or
    /// corrects it.
    pub fn navigate(&mut self, url: String) {
        self.url = url;
        self.favicon_url = None;
        self.backend.navigate(&self.url);
    }

    pub fn reload(&mut self) {
        self.backend.reload();
    }

    pub fn stop(&mut self) {
        self.backend.stop();
    }

    pub fn go_back(&mut self) {
        self.backend.go_back();
    }

    pub fn go_forward(&mut self) {
        self.backend.go_forward();
    }

    pub fn set_focus(&mut self, focused: bool) {
        self.backend.set_focus(focused);
    }

    pub fn set_hidden(&mut self, hidden: bool) {
        self.backend.set_hidden(hidden);
    }

    pub fn send_mouse_down(
        &mut self,
        position: Point<Pixels>,
        button: MouseButton,
        click_count: usize,
        modifiers: Modifiers,
    ) {
        self.backend
            .send_mouse_down(position, button, click_count, modifiers);
    }

    pub fn send_mouse_up(
        &mut self,
        position: Point<Pixels>,
        button: MouseButton,
        modifiers: Modifiers,
    ) {
        self.backend.send_mouse_up(position, button, modifiers);
    }

    pub fn send_mouse_move(
        &mut self,
        position: Point<Pixels>,
        pressed_button: Option<MouseButton>,
        modifiers: Modifiers,
    ) {
        self.backend
            .send_mouse_move(position, pressed_button, modifiers);
    }

    pub fn send_scroll_wheel(
        &mut self,
        position: Point<Pixels>,
        delta: ScrollDelta,
        modifiers: Modifiers,
    ) {
        self.backend.send_scroll_wheel(position, delta, modifiers);
    }

    pub fn send_key_down(&mut self, keystroke: &Keystroke, is_held: bool) {
        self.backend.send_key_down(keystroke, is_held);
    }

    pub fn send_key_up(&mut self, keystroke: &Keystroke) {
        self.backend.send_key_up(keystroke);
    }

    /// Move the latest engine frame, if any, into this tab's presenter.
    pub fn present_pending_frame(&mut self) {
        if let Some(output) = self.backend.take_paint_output() {
            self.presenter.present(output);
        }
    }

    pub fn render_frame(&mut self, window: &mut Window) -> Option<AnyElement> {
        self.presenter.render_frame(window)
    }

    /// Whether this tab's presenter holds a frame (test observability).
    #[cfg(test)]
    pub fn has_frame(&self) -> bool {
        self.presenter.has_frame()
    }

    /// Release the presenter's retained GPU resources. Requires the window;
    /// engine shutdown is separate ([`close`](Self::close)) so it can run even
    /// when the window is already gone.
    pub fn release_presenter(&mut self, window: &mut Window) {
        self.presenter.release(window);
    }

    /// Close the underlying engine browser.
    pub fn close(&mut self) {
        self.backend.close();
    }
}
