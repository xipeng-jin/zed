//! Integrated CEF browser (Glass rebuild, `docs/glass/migration-plan.md`).
//!
//! M1: CEF boots inside Zed, runs windowless with an external message pump
//! driven by a GPUI foreground task, and renders pages into a workspace pane
//! item via software OSR. All engine communication flows through the
//! tab-backend seam (`tab_backend.rs`) and all frame presentation through the
//! frame-presenter seam (`frame_presenter.rs`).
//!
//! The real engine sits behind the `cef` cargo feature (enabled by
//! `crates/zed` on desktop targets). Without it the crate contains everything
//! above the tab-backend seam, so its deterministic tests build and run with
//! no CEF distribution present (ticket #8).

mod bookmarks;
mod browser_settings;
mod browser_tab;
mod browser_view;
mod context_menu;
mod downloads;
mod frame_presenter;
mod history;
mod omnibox;
mod page_chrome;
mod session;
#[cfg(any(test, feature = "test-support"))]
mod stub_tab_backend;
mod tab_backend;
mod text_input;

#[cfg(feature = "cef")]
mod cef_instance;
#[cfg(feature = "cef")]
mod client;
#[cfg(feature = "cef")]
mod context_menu_handler;
#[cfg(feature = "cef")]
mod display_handler;
#[cfg(feature = "cef")]
mod download_handler;
#[cfg(feature = "cef")]
mod find_handler;
#[cfg(feature = "cef")]
mod input;
#[cfg(feature = "cef")]
mod keycodes;
#[cfg(feature = "cef")]
mod life_span_handler;
#[cfg(feature = "cef")]
mod load_handler;
#[cfg(feature = "cef")]
mod permission_handler;
#[cfg(feature = "cef")]
mod render_handler;
#[cfg(feature = "cef")]
mod request_handler;
#[cfg(feature = "cef")]
mod tab;

#[cfg(all(
    feature = "cef",
    not(any(target_os = "linux", target_os = "macos", target_os = "windows"))
))]
compile_error!("the `cef` feature is only supported on Linux, macOS, and Windows");

pub use browser_settings::BrowserSettings;
pub use browser_view::{BrowserView, NewIncognitoWindow, OpenBrowser, TabBackendFactory};
#[cfg(feature = "cef")]
pub use cef_instance::CefInstance;
pub use context_menu::ContextMenuContext;
pub use downloads::DownloadUpdate;
pub use frame_presenter::{FramePresenter, SoftwarePresenter};
pub use page_chrome::{PageChrome, PageChromeSource};
#[cfg(any(test, feature = "test-support"))]
pub use stub_tab_backend::{
    RecordedCommand, StubBackendFactory, StubTabBackend, StubTabController,
};
pub use tab_backend::{
    BrowserTabOpenTarget, OpenDisposition, OpenTargetRequest, PaintOutput, SoftwareFrame,
    TabBackend, TabBackendEvent,
};
pub use text_input::BrowserTextInputState;

use gpui::App;

/// Handle CEF subprocess execution. This MUST be the first thing `main()` does,
/// before any other initialization: on Linux and Windows CEF subprocesses are
/// this executable re-invoked with `--type=...` arguments, and this call never
/// returns for them (it calls `std::process::exit`).
#[cfg(feature = "cef")]
pub fn handle_cef_subprocess() -> anyhow::Result<()> {
    CefInstance::handle_subprocess()
}

#[cfg(not(feature = "cef"))]
pub fn handle_cef_subprocess() -> anyhow::Result<()> {
    Ok(())
}

#[cfg(feature = "cef")]
pub fn init(cx: &mut App) {
    match CefInstance::initialize(cx) {
        Ok(_) => {
            // Ensure CEF is shut down before the process exits. Without this,
            // exit() triggers CEF's static CefShutdownChecker destructor, which
            // asserts that CefShutdown() was already called.
            cx.on_app_quit(|_| async {
                CefInstance::shutdown();
            })
            .detach();

            start_message_pump(cx);
            browser_settings::init(cx);
            browser_view::init(cx);
        }
        Err(error) => {
            log::error!(
                "[browser] failed to initialize CEF: {error:#}. Browser features will be unavailable."
            );
        }
    }
}

#[cfg(not(feature = "cef"))]
pub fn init(_cx: &mut App) {}

/// Opens each URL as a browser tab in the workspace's browser view, creating
/// the view if needed and activating the last tab. The app's open-url path
/// calls this with web links handed over by the OS, e.g. when the app is
/// registered as the default browser (ticket #21).
#[cfg(feature = "cef")]
pub fn open_urls(
    workspace: &mut workspace::Workspace,
    urls: Vec<String>,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<workspace::Workspace>,
) {
    BrowserView::open_urls(workspace, urls, window, cx);
}

#[cfg(not(feature = "cef"))]
pub fn open_urls(
    _workspace: &mut workspace::Workspace,
    urls: Vec<String>,
    _window: &mut gpui::Window,
    _cx: &mut gpui::Context<workspace::Workspace>,
) {
    log::error!("[browser] built without an engine; cannot open {urls:?}");
}

/// Callbacks run on the foreground thread after every message-pump iteration,
/// used by browser views to drain their tab backends' event streams. A
/// callback returning `false` is dropped.
#[derive(Default)]
struct PumpObservers(Vec<Box<dyn FnMut(&mut App) -> bool>>);

impl gpui::Global for PumpObservers {}

pub(crate) fn observe_pumps(cx: &mut App, callback: impl FnMut(&mut App) -> bool + 'static) {
    cx.default_global::<PumpObservers>()
        .0
        .push(Box::new(callback));
}

#[cfg(any(feature = "cef", test, feature = "test-support"))]
fn run_pump_observers(cx: &mut App) {
    // Take the observers out while running them: an observer may register new
    // observers (e.g. a drain callback opening another view), which land in
    // the global and are appended back afterwards.
    let mut observers = std::mem::take(&mut cx.default_global::<PumpObservers>().0);
    observers.retain_mut(|observer| observer(cx));
    let global = cx.default_global::<PumpObservers>();
    let newly_registered = std::mem::take(&mut global.0);
    global.0 = observers;
    global.0.extend(newly_registered);
}

/// Test stand-in for one message-pump iteration: runs the pump observers
/// exactly as the real pump does after `do_message_loop_work`, so scripted
/// stub events drain into browser views through the same path CEF events do.
#[cfg(any(test, feature = "test-support"))]
pub fn simulate_message_pump(cx: &mut App) {
    run_pump_observers(cx);
}

/// Drive CEF's external message pump from a GPUI foreground task.
///
/// CEF reports when it next wants `do_message_loop_work` called via
/// `on_schedule_message_pump_work`; `CefInstance` folds that into a deadline
/// the loop sleeps toward. With no browsers alive the deadline decays to a
/// ~30 Hz idle fallback and the sleep cap stays loose. Once engine browsers
/// exist the cap tightens to 1ms so pump requests issued from other threads
/// mid-sleep (frame paints, input) wait at most ~1ms — keeping frame delivery
/// latency under half a frame at 60fps
/// (`Glass:crates/browser/src/browser_view/tabs.rs:387`).
#[cfg(feature = "cef")]
fn start_message_pump(cx: &mut App) {
    use std::time::Duration;

    cx.spawn(async move |cx| {
        while CefInstance::is_initialized() {
            // Chromium's X11 event source attaches to the thread's default
            // GMainContext, which nothing else iterates — CEF's external pump
            // expects the embedder to run a GLib loop on Linux (cefclient
            // does, via GTK). Without this, native popup windows (ticket #15)
            // render but never receive input or close events.
            #[cfg(target_os = "linux")]
            pump_glib_main_context();

            if CefInstance::should_pump() {
                CefInstance::pump_messages();
                cx.update(run_pump_observers);
            }

            let wait_us = CefInstance::time_until_next_pump_us();
            let cap_us = if tab::has_live_browsers() {
                1_000
            } else {
                33_000
            };
            let sleep_us = wait_us.clamp(500, cap_us);
            cx.background_executor()
                .timer(Duration::from_micros(sleep_us))
                .await;
        }
    })
    .detach();
}

/// Dispatch any ready GLib sources on the default main context without
/// blocking, bounded in case a dispatched source keeps re-arming.
#[cfg(all(feature = "cef", target_os = "linux"))]
fn pump_glib_main_context() {
    for _ in 0..16 {
        // SAFETY: null means the default context; FALSE means do not block.
        // Called only from the thread that initialized CEF (the foreground
        // thread), matching GLib's ownership expectations for the default
        // context.
        let dispatched =
            unsafe { glib_sys::g_main_context_iteration(std::ptr::null_mut(), glib_sys::GFALSE) };
        if dispatched == glib_sys::GFALSE {
            break;
        }
    }
}
