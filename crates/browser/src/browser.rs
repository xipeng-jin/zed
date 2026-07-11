//! Integrated CEF browser (Glass rebuild, `docs/glass/migration-plan.md`).
//!
//! M1 engine lifecycle: CEF boots inside Zed, runs windowless with an external
//! message pump driven by a GPUI foreground task, and shuts down cleanly on app
//! quit. Browser UI and tabs land in later milestones.

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod cef_instance;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub use cef_instance::CefInstance;

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

/// Drive CEF's external message pump from a GPUI foreground task.
///
/// CEF reports when it next wants `do_message_loop_work` called via
/// `on_schedule_message_pump_work`; `CefInstance` folds that into a deadline the
/// loop sleeps toward. With no browsers alive the deadline decays to a ~30 Hz
/// idle fallback, so the task never busy-loops. The 33ms sleep cap bounds how
/// long a pump request issued from another thread can wait mid-sleep; once
/// browser tabs exist, frame-latency requirements may force this cap down
/// (Glass used 1ms — `Glass:crates/browser/src/browser_view/tabs.rs:387`).
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn start_message_pump(cx: &mut App) {
    use std::time::Duration;

    cx.spawn(async move |cx| {
        while CefInstance::is_initialized() {
            if CefInstance::should_pump() {
                CefInstance::pump_messages();
            }

            let wait_us = CefInstance::time_until_next_pump_us();
            let sleep_us = wait_us.clamp(500, 33_000);
            cx.background_executor()
                .timer(Duration::from_micros(sleep_us))
                .await;
        }
    })
    .detach();
}
