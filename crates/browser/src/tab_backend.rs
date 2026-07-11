//! The tab-backend seam: the trait through which all engine communication for a
//! single browser tab flows (PRD issue #2, "Engine seam").
//!
//! Commands go in through [`TabBackend`] methods; engine state comes back out
//! through the [`TabBackendEvent`] stream and the paint-output frame source.
//! Two implementations exist: the real CEF backend (`CefTab` in `tab.rs`) and a
//! scripted stub for deterministic tests (see `browser_view.rs` tests; the full
//! harness is ticket #8).

use anyhow::Result;
use std::sync::mpsc;

/// Events flowing from the engine to the app, drained on the foreground thread
/// after each message-pump iteration.
#[derive(Debug, Clone, PartialEq)]
pub enum TabBackendEvent {
    /// The engine browser for this tab finished creating.
    Created,
    /// The main frame's URL changed.
    AddressChanged(String),
    /// The page title changed.
    TitleChanged(String),
    /// Loading started or stopped, with current history-traversal ability.
    LoadingStateChanged {
        is_loading: bool,
        can_go_back: bool,
        can_go_forward: bool,
    },
    /// Estimated load progress in `0.0..=1.0`.
    LoadingProgress(f64),
    /// The page's favicon candidate URLs changed.
    FaviconUrlsChanged(Vec<String>),
    /// A new frame is available from the paint-output source.
    FrameReady,
    /// The main frame failed to load.
    LoadError { url: String, error_text: String },
}

/// One software-OSR frame: a tightly-packed premultiplied BGRA buffer at
/// physical (device) pixel resolution.
pub struct SoftwareFrame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// Engine paint output, consumed by a frame presenter. Software OSR is the
/// cross-platform baseline; an IOSurface variant joins in M3 (ADR-0002).
pub enum PaintOutput {
    Software(SoftwareFrame),
}

/// Commands into the engine for one browser tab. All engine communication in
/// the browser view flows through this trait; nothing above it may touch CEF.
///
/// Lifecycle: construct, then [`set_viewport`](Self::set_viewport) with real
/// dimensions, then [`start`](Self::start) once
/// [`engine_ready`](Self::engine_ready) reports true. Input injection methods
/// join with tickets #6 (pointer/keys) and #16 (IME).
pub trait TabBackend: 'static {
    /// Whether the engine is initialized enough to create browsers.
    fn engine_ready(&self) -> bool;

    /// Create the underlying engine browser, loading `url`. The viewport must
    /// have been set to its real size first: the engine reads it synchronously
    /// during creation.
    fn start(&mut self, url: &str) -> Result<()>;

    /// True once `start` has succeeded.
    fn is_started(&self) -> bool;

    fn navigate(&mut self, url: &str);

    fn reload(&mut self);

    fn go_back(&mut self);

    fn go_forward(&mut self);

    /// Set the tab's viewport size in logical pixels plus the display scale
    /// factor. The engine paints at `size * scale_factor` device pixels.
    fn set_viewport(&mut self, width: u32, height: u32, scale_factor: f32);

    fn set_focus(&mut self, focused: bool);

    /// Close the underlying engine browser. Also invoked on drop.
    fn close(&mut self);

    /// Drain one pending engine event, if any.
    fn try_recv_event(&mut self) -> Option<TabBackendEvent>;

    /// Take the most recent unpresented frame, if a new one arrived since the
    /// last call. The engine keeps only the latest frame; intermediate frames
    /// are dropped.
    fn take_paint_output(&mut self) -> Option<PaintOutput>;
}

pub(crate) type EventSender = mpsc::Sender<TabBackendEvent>;
pub(crate) type EventReceiver = mpsc::Receiver<TabBackendEvent>;

pub(crate) fn event_channel() -> (EventSender, EventReceiver) {
    mpsc::channel()
}

/// Send an event from an engine handler thread, tolerating a receiver that was
/// dropped mid-teardown (the tab is closing; late events are expected).
pub(crate) fn send_event(sender: &EventSender, event: TabBackendEvent) {
    if sender.send(event).is_err() {
        log::trace!("[browser] dropped engine event: tab backend receiver closed");
    }
}
