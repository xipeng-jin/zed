//! The tab-backend seam: the trait through which all engine communication for a
//! single browser tab flows (PRD issue #2, "Engine seam").
//!
//! Commands go in through [`TabBackend`] methods; engine state comes back out
//! through the [`TabBackendEvent`] stream and the paint-output frame source.
//! Two implementations exist: the real CEF backend (`CefTab` in `tab.rs`,
//! behind the `cef` feature) and the scripted stub for deterministic tests
//! (`stub_tab_backend.rs`).

use crate::context_menu::ContextMenuContext;
use crate::downloads::DownloadUpdate;
use crate::page_chrome::PageChrome;
use crate::text_input::BrowserTextInputState;
use anyhow::Result;
use gpui::{Keystroke, Modifiers, MouseButton, Pixels, Point, ScrollDelta};
use std::ops::Range;
#[cfg(feature = "cef")]
use std::sync::mpsc;

/// Where an engine-initiated open lands in the browser view's tab strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserTabOpenTarget {
    /// The new browser tab is activated immediately.
    Foreground,
    /// The new browser tab joins the strip without taking over.
    Background,
}

/// How a page asked for a navigation target to be opened, mapped from the
/// engine's window-open disposition
/// (`Glass:crates/browser/src/events.rs:41-108`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenDisposition {
    Unknown,
    CurrentTab,
    SingletonTab,
    NewForegroundTab,
    NewBackgroundTab,
    NewPopup,
    NewWindow,
    SaveToDisk,
    OffTheRecord,
    IgnoreAction,
    SwitchToTab,
    NewPictureInPicture,
}

impl OpenDisposition {
    /// Dispositions the app redirects into its own browser tab flow instead of
    /// letting the engine create a window. The browser view has no separate
    /// windows, so window-like dispositions land as foreground tabs.
    pub fn app_tab_target(self) -> Option<BrowserTabOpenTarget> {
        match self {
            Self::NewForegroundTab | Self::NewWindow | Self::OffTheRecord | Self::SwitchToTab => {
                Some(BrowserTabOpenTarget::Foreground)
            }
            Self::NewBackgroundTab => Some(BrowserTabOpenTarget::Background),
            Self::Unknown
            | Self::CurrentTab
            | Self::SingletonTab
            | Self::NewPopup
            | Self::SaveToDisk
            | Self::IgnoreAction
            | Self::NewPictureInPicture => None,
        }
    }

    /// Whether the engine may host this open as a real native window. Only
    /// genuine popups (OAuth/login windows) qualify; they need native opener
    /// semantics (`window.opener`, postMessage) to complete their flows.
    pub fn allow_native_popup(self) -> bool {
        matches!(self, Self::NewPopup)
    }
}

/// A page-initiated request to open `url` somewhere other than the current
/// browser tab, surfaced to the browser view for routing. Glass additionally
/// carried the engine's user-gesture and popup-origin flags; they were never
/// consulted, so they are not ported (a future popup-blocking policy can
/// reintroduce them from the engine callbacks).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenTargetRequest {
    pub url: String,
    pub disposition: OpenDisposition,
}

/// How one in-page find request behaves. `find_next` distinguishes stepping
/// through the current query's matches from starting a fresh search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FindOptions {
    pub forward: bool,
    pub match_case: bool,
    pub find_next: bool,
}

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
    /// A download this tab initiated started or progressed.
    DownloadUpdated(DownloadUpdate),
    /// An in-page find reported its matches: how many, and which one is
    /// selected (1-based; 0 while unknown).
    FindResult {
        count: i32,
        active_match_ordinal: i32,
    },
    /// The user right-clicked the page; the app renders the menu (OSR
    /// suppresses the engine's own).
    ContextMenuRequested(ContextMenuContext),
    /// The page asked to open a URL outside the current browser tab (a
    /// tab-like popup or link-open the engine handlers redirected here).
    OpenTargetRequested(OpenTargetRequest),
    /// The render process reported whether the page's focused node is
    /// editable, which drives keystroke routing (ticket #16).
    TextInputStateChanged(BrowserTextInputState),
    /// The page's sampled theme color changed, or resolved to nothing
    /// (`None`); it tints the page's tab in the tab strip (ticket #22).
    PageChromeChanged(Option<PageChrome>),
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
/// [`engine_ready`](Self::engine_ready) reports true.
///
/// Input positions are logical pixels relative to the tab's content origin;
/// the engine converts to device pixels via the viewport scale factor.
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

    /// Stop loading the current page.
    fn stop(&mut self);

    fn go_back(&mut self);

    fn go_forward(&mut self);

    /// Set the tab's viewport size in logical pixels plus the display scale
    /// factor. The engine paints at `size * scale_factor` device pixels.
    fn set_viewport(&mut self, width: u32, height: u32, scale_factor: f32);

    fn set_focus(&mut self, focused: bool);

    /// Tell the engine whether this tab is currently presented. Hidden tabs
    /// stop painting until shown again; their last frame stays with their
    /// presenter.
    fn set_hidden(&mut self, hidden: bool);

    fn send_mouse_down(
        &mut self,
        position: Point<Pixels>,
        button: MouseButton,
        click_count: usize,
        modifiers: Modifiers,
    );

    fn send_mouse_up(&mut self, position: Point<Pixels>, button: MouseButton, modifiers: Modifiers);

    fn send_mouse_move(
        &mut self,
        position: Point<Pixels>,
        pressed_button: Option<MouseButton>,
        modifiers: Modifiers,
    );

    fn send_scroll_wheel(
        &mut self,
        position: Point<Pixels>,
        delta: ScrollDelta,
        modifiers: Modifiers,
    );

    /// Send a key-down to the page, including the character event for
    /// printable keys. Only invoked for keystrokes classified as
    /// browser-routed; the engine marks them app-sent so its native key
    /// suppression lets them through.
    fn send_key_down(&mut self, keystroke: &Keystroke, is_held: bool);

    fn send_key_up(&mut self, keystroke: &Keystroke);

    /// Show `text` as the in-progress IME composition (preedit) in the page's
    /// focused editable field, with `selected_range` as the UTF-16 selection
    /// within it. Repeated calls replace the composition.
    fn ime_set_composition(&mut self, text: &str, selected_range: Option<Range<usize>>);

    /// Insert `text` into the focused editable field as committed input.
    fn ime_commit_text(&mut self, text: &str);

    /// Abandon the in-progress composition, removing the preedit text.
    fn ime_cancel_composition(&mut self);

    /// Start or continue an in-page find; results come back as
    /// [`TabBackendEvent::FindResult`]s.
    fn find(&mut self, query: &str, options: FindOptions);

    /// End the in-page find, optionally clearing the match selection.
    fn stop_finding(&mut self, clear_selection: bool);

    /// Edit commands targeting the page's focused frame, used by the
    /// app-rendered context menu on editable fields.
    fn undo(&mut self);

    fn redo(&mut self);

    fn cut(&mut self);

    fn copy(&mut self);

    fn paste(&mut self);

    fn delete(&mut self);

    fn select_all(&mut self);

    /// Download `url` through the engine's download pipeline, as if the page
    /// had triggered it.
    fn start_download(&mut self, url: &str);

    /// Open the engine's developer tools attached to this tab, in a native
    /// engine-managed window.
    fn open_devtools(&mut self);

    /// Close the underlying engine browser. Also invoked on drop.
    fn close(&mut self);

    /// Drain one pending engine event, if any.
    fn try_recv_event(&mut self) -> Option<TabBackendEvent>;

    /// Take the most recent unpresented frame, if a new one arrived since the
    /// last call. The engine keeps only the latest frame; intermediate frames
    /// are dropped.
    fn take_paint_output(&mut self) -> Option<PaintOutput>;
}

#[cfg(feature = "cef")]
pub(crate) type EventSender = mpsc::Sender<TabBackendEvent>;
#[cfg(feature = "cef")]
pub(crate) type EventReceiver = mpsc::Receiver<TabBackendEvent>;

#[cfg(feature = "cef")]
pub(crate) fn event_channel() -> (EventSender, EventReceiver) {
    mpsc::channel()
}

/// Send an event from an engine handler thread, tolerating a receiver that was
/// dropped mid-teardown (the tab is closing; late events are expected).
#[cfg(feature = "cef")]
pub(crate) fn send_event(sender: &EventSender, event: TabBackendEvent) {
    if sender.send(event).is_err() {
        log::trace!("[browser] dropped engine event: tab backend receiver closed");
    }
}

/// Shared gate for the life-span and request handlers: if `disposition` is
/// tab-like and the target URL is real, emit an
/// [`TabBackendEvent::OpenTargetRequested`] and report true so the caller
/// takes over the open from the engine.
#[cfg(feature = "cef")]
pub(crate) fn redirect_open_target_to_tab(
    sender: &EventSender,
    target_url: Option<String>,
    disposition: OpenDisposition,
) -> bool {
    if disposition.app_tab_target().is_none() {
        return false;
    }
    let Some(url) = target_url.filter(|url| !url.is_empty()) else {
        return false;
    };
    send_event(
        sender,
        TabBackendEvent::OpenTargetRequested(OpenTargetRequest { url, disposition }),
    );
    true
}

#[cfg(feature = "cef")]
impl From<cef::WindowOpenDisposition> for OpenDisposition {
    fn from(value: cef::WindowOpenDisposition) -> Self {
        use cef::WindowOpenDisposition;
        if value == WindowOpenDisposition::CURRENT_TAB {
            Self::CurrentTab
        } else if value == WindowOpenDisposition::SINGLETON_TAB {
            Self::SingletonTab
        } else if value == WindowOpenDisposition::NEW_FOREGROUND_TAB {
            Self::NewForegroundTab
        } else if value == WindowOpenDisposition::NEW_BACKGROUND_TAB {
            Self::NewBackgroundTab
        } else if value == WindowOpenDisposition::NEW_POPUP {
            Self::NewPopup
        } else if value == WindowOpenDisposition::NEW_WINDOW {
            Self::NewWindow
        } else if value == WindowOpenDisposition::SAVE_TO_DISK {
            Self::SaveToDisk
        } else if value == WindowOpenDisposition::OFF_THE_RECORD {
            Self::OffTheRecord
        } else if value == WindowOpenDisposition::IGNORE_ACTION {
            Self::IgnoreAction
        } else if value == WindowOpenDisposition::SWITCH_TO_TAB {
            Self::SwitchToTab
        } else if value == WindowOpenDisposition::NEW_PICTURE_IN_PICTURE {
            Self::NewPictureInPicture
        } else {
            Self::Unknown
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BrowserTabOpenTarget, OpenDisposition};

    #[test]
    fn tab_like_dispositions_are_app_managed() {
        assert_eq!(
            OpenDisposition::NewForegroundTab.app_tab_target(),
            Some(BrowserTabOpenTarget::Foreground),
        );
        assert_eq!(
            OpenDisposition::NewBackgroundTab.app_tab_target(),
            Some(BrowserTabOpenTarget::Background),
        );
        assert_eq!(
            OpenDisposition::NewWindow.app_tab_target(),
            Some(BrowserTabOpenTarget::Foreground),
        );
        assert_eq!(
            OpenDisposition::SwitchToTab.app_tab_target(),
            Some(BrowserTabOpenTarget::Foreground),
        );
    }

    #[test]
    fn popup_disposition_is_left_to_native_popup_handling() {
        assert_eq!(OpenDisposition::NewPopup.app_tab_target(), None);
        assert!(OpenDisposition::NewPopup.allow_native_popup());
    }

    #[test]
    fn non_navigation_dispositions_are_neither_tab_nor_popup() {
        for disposition in [
            OpenDisposition::Unknown,
            OpenDisposition::CurrentTab,
            OpenDisposition::SingletonTab,
            OpenDisposition::SaveToDisk,
            OpenDisposition::IgnoreAction,
            OpenDisposition::NewPictureInPicture,
        ] {
            assert_eq!(disposition.app_tab_target(), None);
            assert!(!disposition.allow_native_popup());
        }
    }
}
