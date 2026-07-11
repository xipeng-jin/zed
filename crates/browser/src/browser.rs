//! Integrated CEF browser (Glass rebuild, `docs/glass/migration-plan.md`).
//!
//! M1: CEF boots inside Zed, runs windowless with an external message pump
//! driven by a GPUI foreground task, and renders pages into a workspace pane
//! item via software OSR. All engine communication flows through the
//! tab-backend seam (`tab_backend.rs`) and all frame presentation through the
//! frame-presenter seam (`frame_presenter.rs`).

mod browser_view;
mod frame_presenter;
mod omnibox;
mod tab_backend;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod cef_instance;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod client;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod display_handler;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod input;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod keycodes;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod life_span_handler;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod load_handler;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod render_handler;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod tab;

pub use browser_view::{BrowserView, OpenBrowser};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub use cef_instance::CefInstance;
pub use frame_presenter::{FramePresenter, SoftwarePresenter};
pub use tab_backend::{PaintOutput, SoftwareFrame, TabBackend, TabBackendEvent};

use gpui::App;

/// Handle CEF subprocess execution. This MUST be the first thing `main()` does,
/// before any other initialization: on Linux and Windows CEF subprocesses are
/// this executable re-invoked with `--type=...` arguments, and this call never
/// returns for them (it calls `std::process::exit`).
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub fn handle_cef_subprocess() -> anyhow::Result<()> {
    CefInstance::handle_subprocess()
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub fn handle_cef_subprocess() -> anyhow::Result<()> {
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
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
            browser_view::init(cx);
        }
        Err(error) => {
            log::error!(
                "[browser] failed to initialize CEF: {error:#}. Browser features will be unavailable."
            );
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub fn init(_cx: &mut App) {}

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

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
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
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn start_message_pump(cx: &mut App) {
    use std::time::Duration;

    cx.spawn(async move |cx| {
        while CefInstance::is_initialized() {
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
