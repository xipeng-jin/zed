//! CEF lifecycle management, ported from `Glass:crates/browser/src/cef_instance.rs`.
//!
//! CEF initialization is split into two phases:
//! 1. `handle_subprocess()` — must be called very early in `main()`, before any
//!    GUI initialization. If this process is a CEF subprocess it never returns.
//! 2. `initialize()` — called later (from `browser::init`) to complete CEF setup
//!    for the browser process.

use anyhow::{Result, anyhow};
use cef::{
    App, BrowserProcessHandler, ImplApp, ImplBrowserProcessHandler, ImplCommandLine, WrapApp,
    WrapBrowserProcessHandler, api_hash, rc::Rc as _, sys, wrap_app, wrap_browser_process_handler,
};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

static CEF_SUBPROCESS_HANDLED: AtomicBool = AtomicBool::new(false);
static CEF_INITIALIZED: AtomicBool = AtomicBool::new(false);
static CEF_CONTEXT_READY: AtomicBool = AtomicBool::new(false);
static CEF_INSTANCE: Mutex<Option<Arc<CefInstance>>> = Mutex::new(None);
static CEF_APP: Mutex<Option<cef::App>> = Mutex::new(None);

#[cfg(target_os = "macos")]
static CEF_LIBRARY_LOADER: Mutex<Option<cef::library_loader::LibraryLoader>> = Mutex::new(None);

static PUMP_SCHEDULE: PumpSchedule = PumpSchedule::new();
static PUMP_EPOCH: OnceLock<Instant> = OnceLock::new();

fn elapsed_us() -> u64 {
    PUMP_EPOCH.get_or_init(Instant::now).elapsed().as_micros() as u64
}

/// When the next `do_message_loop_work` call is due, as an absolute time in
/// microseconds on the caller-supplied clock. CEF's `on_schedule_message_pump_work`
/// callbacks (which may arrive on any thread) fold into a single earliest deadline.
struct PumpSchedule {
    next_due_us: AtomicU64,
}

impl PumpSchedule {
    const IDLE: u64 = u64::MAX;
    /// Re-check cadence when CEF has not scheduled any work (~30 Hz).
    const IDLE_FALLBACK_US: u64 = 33_000;

    const fn new() -> Self {
        Self {
            next_due_us: AtomicU64::new(Self::IDLE),
        }
    }

    /// Record a CEF request to pump `delay_ms` from now. The earliest
    /// outstanding request wins.
    fn request(&self, delay_ms: i64, now_us: u64) {
        let delay_us = (delay_ms.max(0) as u64).saturating_mul(1_000);
        let due_us = now_us.saturating_add(delay_us);
        self.next_due_us.fetch_min(due_us, Ordering::Release);
    }

    fn is_due(&self, now_us: u64) -> bool {
        now_us >= self.next_due_us.load(Ordering::Acquire)
    }

    fn time_until_due_us(&self, now_us: u64) -> u64 {
        self.next_due_us
            .load(Ordering::Acquire)
            .saturating_sub(now_us)
    }

    /// Clear the deadline before pumping; CEF re-schedules during
    /// `do_message_loop_work` if more work is pending.
    fn clear(&self) {
        self.next_due_us.store(Self::IDLE, Ordering::Release);
    }

    /// After a pump, if CEF scheduled nothing, fall back to a periodic re-check
    /// so missed schedule callbacks cannot stall the pump forever.
    fn schedule_idle_fallback(&self, now_us: u64) {
        // A failed exchange means CEF scheduled work during the pump; keep
        // that earlier deadline.
        self.next_due_us
            .compare_exchange(
                Self::IDLE,
                now_us.saturating_add(Self::IDLE_FALLBACK_US),
                Ordering::AcqRel,
                Ordering::Relaxed,
            )
            .ok();
    }
}

#[derive(Clone)]
struct ZedBrowserProcessHandler {}

wrap_browser_process_handler! {
    struct ZedBrowserProcessHandlerBuilder {
        handler: ZedBrowserProcessHandler,
    }

    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            CEF_CONTEXT_READY.store(true, Ordering::SeqCst);
            log::info!("[browser] CEF context initialized");
        }

        fn on_before_child_process_launch(&self, command_line: Option<&mut cef::CommandLine>) {
            let Some(command_line) = command_line else {
                return;
            };
            command_line.append_switch(Some(&"disable-session-crashed-bubble".into()));
        }

        fn on_schedule_message_pump_work(&self, delay_ms: i64) {
            PUMP_SCHEDULE.request(delay_ms, elapsed_us());
        }
    }
}

#[derive(Clone)]
struct ZedCefApp {
    browser_process_handler: cef::BrowserProcessHandler,
    render_process_handler: cef::RenderProcessHandler,
}

impl ZedCefApp {
    fn new() -> Self {
        Self {
            browser_process_handler: ZedBrowserProcessHandlerBuilder::new(
                ZedBrowserProcessHandler {},
            ),
            render_process_handler:
                crate::page_chrome::PageChromeRenderProcessHandlerBuilder::build(),
        }
    }
}

wrap_app! {
    struct ZedCefAppBuilder {
        app: ZedCefApp,
    }

    impl App {
        fn on_before_command_line_processing(
            &self,
            _process_type: Option<&cef::CefStringUtf16>,
            command_line: Option<&mut cef::CommandLine>,
        ) {
            let Some(command_line) = command_line else {
                return;
            };

            command_line.append_switch(Some(&"no-startup-window".into()));
            command_line.append_switch(Some(&"noerrdialogs".into()));
            command_line.append_switch(Some(&"hide-crash-restore-bubble".into()));
            // Keep Chromium on the real keychain path in signed production
            // builds. The mock keychain path is only for local ad-hoc bundles.
            #[cfg(debug_assertions)]
            command_line.append_switch(Some(&"use-mock-keychain".into()));
            command_line.append_switch(Some(&"disable-gpu-sandbox".into()));
            command_line.append_switch_with_value(
                Some(&"autoplay-policy".into()),
                Some(&"no-user-gesture-required".into()),
            );
            command_line.append_switch_with_value(
                Some(&"component-updater".into()),
                Some(&"fast-update".into()),
            );
            #[cfg(target_os = "macos")]
            {
                command_line.append_switch_with_value(
                    Some(&"use-angle".into()),
                    Some(&"metal".into()),
                );
            }
            #[cfg(target_os = "linux")]
            {
                // Pin Chromium's windowing to X11 (risk R1: "accept
                // XWayland"). Left to auto-detection, ozone picks Wayland
                // from the session environment even when GPUI runs on X11,
                // and native popup windows (ticket #15) then open on the
                // Wayland session half-broken: GPU compositing fails
                // ("--ozone-platform=wayland is not compatible with Vulkan")
                // and the window cannot be closed. Under a Wayland session
                // the X11 path runs via XWayland, which popups handle fine.
                command_line.append_switch_with_value(
                    Some(&"ozone-platform".into()),
                    Some(&"x11".into()),
                );
            }
            command_line.append_switch(Some(&"ignore-gpu-blocklist".into()));
            command_line.append_switch(Some(&"enable-gpu-rasterization".into()));
            command_line.append_switch(Some(&"enable-zero-copy".into()));
            command_line.append_switch(Some(&"enable-accelerated-video-decode".into()));
            command_line.append_switch(Some(&"enable-accelerated-mjpeg-decode".into()));
            command_line.append_switch_with_value(
                Some(&"enable-features".into()),
                Some(&"PlatformHEVCDecoderSupport,PlatformEncryptedDolbyVision".into()),
            );
            #[cfg(target_os = "windows")]
            command_line.append_switch_with_value(
                Some(&"disable-features".into()),
                Some(&"WebUsbDeviceDetection".into()),
            );
            #[cfg(debug_assertions)]
            {
                if std::env::var_os("ZED_CEF_DEBUG").is_some() {
                    command_line.append_switch(Some(&"enable-logging=stderr".into()));
                    command_line.append_switch_with_value(
                        Some(&"remote-debugging-port".into()),
                        Some(&"9222".into()),
                    );
                }
            }
        }

        fn browser_process_handler(&self) -> Option<cef::BrowserProcessHandler> {
            Some(self.app.browser_process_handler.clone())
        }

        // Runs in the render subprocess (the same App is passed to
        // `execute_process`): reports focused-node editability for keystroke
        // routing (ticket #16).
        fn render_process_handler(&self) -> Option<cef::RenderProcessHandler> {
            Some(self.app.render_process_handler.clone())
        }
    }
}

fn build_cef_app() -> cef::App {
    ZedCefAppBuilder::new(ZedCefApp::new())
}

// Google's sign-in endpoints reject unrecognized browser versions with 400
// errors on the browserinfo fingerprint check, so present the matching stable
// Chrome release's UA for the host platform instead of CEF's default. The
// Chrome version must track the pinned CEF respin's Chromium version
// (migration plan §6.4); it appears once here so a bump edits one literal.
macro_rules! spoofed_user_agent {
    ($platform:literal) => {
        concat!(
            "Mozilla/5.0 (",
            $platform,
            ") AppleWebKit/537.36 (KHTML, like Gecko) Chrome/150.0.7871.101 Safari/537.36"
        )
    };
}
#[cfg(target_os = "macos")]
const SPOOFED_USER_AGENT: &str = spoofed_user_agent!("Macintosh; Intel Mac OS X 10_15_7");
#[cfg(target_os = "linux")]
const SPOOFED_USER_AGENT: &str = spoofed_user_agent!("X11; Linux x86_64");
#[cfg(target_os = "windows")]
const SPOOFED_USER_AGENT: &str = spoofed_user_agent!("Windows NT 10.0; Win64; x64");

/// Resolve the CEF directory from `CEF_PATH` env var, falling back to `~/.local/share/cef`.
#[cfg(target_os = "macos")]
fn resolve_cef_dir_from_env() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;

    let cef_dir = match std::env::var("CEF_PATH") {
        Ok(path) => PathBuf::from(path),
        Err(_) => {
            let home = std::env::var("HOME").ok()?;
            PathBuf::from(home).join(".local/share/cef")
        }
    };
    if cef_dir
        .join("Chromium Embedded Framework.framework/Chromium Embedded Framework")
        .exists()
    {
        Some(cef_dir)
    } else {
        None
    }
}

/// Load the CEF framework directly from a directory path (bypasses LibraryLoader
/// which only supports bundle-relative paths).
#[cfg(target_os = "macos")]
fn load_cef_framework_from_dir(cef_dir: &std::path::Path) -> bool {
    let framework_path =
        cef_dir.join("Chromium Embedded Framework.framework/Chromium Embedded Framework");
    use std::os::unix::ffi::OsStrExt;
    let Ok(path_cstr) = std::ffi::CString::new(framework_path.as_os_str().as_bytes()) else {
        return false;
    };
    unsafe { cef::load_library(Some(&*path_cstr.as_ptr().cast())) == 1 }
}

pub struct CefInstance {}

impl CefInstance {
    pub fn global() -> Option<Arc<CefInstance>> {
        CEF_INSTANCE.lock().clone()
    }

    /// True between successful `initialize()` and `shutdown()`.
    pub fn is_initialized() -> bool {
        CEF_INITIALIZED.load(Ordering::SeqCst)
    }

    /// True once CEF's context is initialized and ready for browser creation.
    pub fn is_context_ready() -> bool {
        CEF_CONTEXT_READY.load(Ordering::SeqCst)
    }

    /// Handle CEF subprocess execution. This MUST be called very early in main(),
    /// before any GUI initialization.
    ///
    /// If this process is a CEF subprocess (renderer, GPU, etc.), this function
    /// will NOT return — it calls `std::process::exit()`. On Linux and Windows
    /// subprocesses are this executable re-invoked with `--type=...` arguments;
    /// on macOS a dedicated helper binary takes this role in packaged builds (M3).
    ///
    /// If this is the main browser process, it returns Ok(()) and normal
    /// initialization should continue.
    pub fn handle_subprocess() -> Result<()> {
        if CEF_SUBPROCESS_HANDLED.load(Ordering::SeqCst) {
            return Ok(());
        }

        // On macOS libcef is a framework loaded at runtime; on Linux and Windows
        // the binary links the CEF library directly and no explicit load happens.
        #[cfg(target_os = "macos")]
        {
            let exe_path = std::env::current_exe()
                .map_err(|error| anyhow!("Failed to get current executable path: {}", error))?;

            let framework_path = exe_path.parent().map(|parent| {
                parent.join(
                    "../Frameworks/Chromium Embedded Framework.framework/Chromium Embedded Framework",
                )
            });

            match framework_path {
                Some(path) if path.exists() => {
                    let loader = cef::library_loader::LibraryLoader::new(&exe_path, false);
                    if !loader.load() {
                        log::warn!("[browser] CEF LibraryLoader::load() failed");
                        return Ok(());
                    }
                    *CEF_LIBRARY_LOADER.lock() = Some(loader);
                }
                _ => {
                    // Not running from a bundle - try CEF_PATH env var or ~/.local/share/cef
                    match resolve_cef_dir_from_env() {
                        Some(cef_dir) => {
                            if !load_cef_framework_from_dir(&cef_dir) {
                                log::warn!(
                                    "[browser] failed to load CEF from {}",
                                    cef_dir.display()
                                );
                                return Ok(());
                            }
                        }
                        None => {
                            return Ok(());
                        }
                    }
                }
            }
        }

        // Must precede any other CEF call: pins the process to the CEF API
        // version the bindings were generated for. The returned hash string
        // itself is unused.
        api_hash(sys::CEF_API_VERSION_LAST, 0);

        let args = cef::args::Args::new();
        let mut app = build_cef_app();

        let exit_code = cef::execute_process(
            Some(args.as_main_args()),
            Some(&mut app),
            std::ptr::null_mut(),
        );

        if exit_code >= 0 {
            std::process::exit(exit_code);
        }

        *CEF_APP.lock() = Some(app);
        CEF_SUBPROCESS_HANDLED.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Initialize CEF for the browser process. Call this after `handle_subprocess()`
    /// has returned successfully and after GPUI is set up.
    pub fn initialize(_cx: &mut gpui::App) -> Result<Arc<CefInstance>> {
        if CEF_INITIALIZED.load(Ordering::SeqCst)
            && let Some(instance) = Self::global()
        {
            return Ok(instance);
        }

        if !CEF_SUBPROCESS_HANDLED.load(Ordering::SeqCst) {
            return Err(anyhow!(
                "CEF subprocess handling was not done. Call CefInstance::handle_subprocess() early in main()."
            ));
        }

        Self::initialize_cef()?;

        CEF_INITIALIZED.store(true, Ordering::SeqCst);
        let instance = Arc::new(CefInstance {});
        *CEF_INSTANCE.lock() = Some(instance.clone());

        Ok(instance)
    }

    fn initialize_cef() -> Result<()> {
        let args = cef::args::Args::new();

        let mut app_guard = CEF_APP.lock();
        let app = app_guard.as_mut().ok_or_else(|| {
            anyhow!("CEF App not found. handle_subprocess() must be called first.")
        })?;

        let mut settings = cef::Settings::default();

        // Browser creation (a later M1 step) must additionally request software
        // OSR on Linux by leaving `shared_texture_enabled` unset in its
        // `WindowInfo` — the GPU shared-texture path is macOS-only (ADR-0002,
        // migration plan §7 M1 step 3).
        settings.windowless_rendering_enabled = 1;
        settings.external_message_pump = 1;
        settings.no_sandbox = 1;
        settings.log_severity = cef::sys::cef_log_severity_t::LOGSEVERITY_WARNING.into();
        settings.user_agent = cef::CefString::from(SPOOFED_USER_AGENT);

        // Set framework_dir_path and main_bundle_path only when not running from a
        // bundle (e.g. cargo run with CEF_PATH). When running from a .app bundle, CEF
        // discovers the bundle automatically via NSBundle; setting main_bundle_path to
        // the wrong directory (Contents/MacOS/) would cause CEF to fail reading the
        // CFBundleIdentifier.
        #[cfg(target_os = "macos")]
        {
            let running_from_bundle = CEF_LIBRARY_LOADER.lock().is_some();
            if !running_from_bundle && let Some(cef_dir) = resolve_cef_dir_from_env() {
                let framework_path = cef_dir.join("Chromium Embedded Framework.framework");
                if framework_path.exists()
                    && let Some(framework_path_str) = framework_path.to_str()
                {
                    settings.framework_dir_path = cef::CefString::from(framework_path_str);
                }
                if let Ok(exe_path) = std::env::current_exe()
                    && let Some(exe_dir) = exe_path.parent()
                    && let Some(dir_str) = exe_dir.to_str()
                {
                    settings.main_bundle_path = cef::CefString::from(dir_str);
                }
            }
        }

        let cache_dir = paths::data_dir().join("browser_cache");
        if let Err(error) = std::fs::create_dir_all(&cache_dir) {
            log::warn!(
                "[browser] failed to create browser cache directory: {}",
                error
            );
        }
        if let Some(cache_path_str) = cache_dir.to_str() {
            settings.cache_path = cef::CefString::from(cache_path_str);
            settings.root_cache_path = cef::CefString::from(cache_path_str);
        }
        settings.persist_session_cookies = 1;

        #[cfg(debug_assertions)]
        {
            if std::env::var_os("ZED_CEF_DEBUG").is_some() {
                settings.remote_debugging_port = 9222;
            }
        }

        let result = cef::initialize(
            Some(args.as_main_args()),
            Some(&settings),
            Some(app),
            std::ptr::null_mut(),
        );

        if result != 1 {
            return Err(anyhow!("Failed to initialize CEF (error code: {})", result));
        }

        Ok(())
    }

    /// Returns true when CEF has requested a pump and the delay has elapsed.
    pub fn should_pump() -> bool {
        if !CEF_CONTEXT_READY.load(Ordering::SeqCst) {
            return false;
        }
        PUMP_SCHEDULE.is_due(elapsed_us())
    }

    /// Microseconds until the next scheduled pump, or 0 if overdue.
    pub fn time_until_next_pump_us() -> u64 {
        PUMP_SCHEDULE.time_until_due_us(elapsed_us())
    }

    /// Pump CEF message loop. Only call when `should_pump()` returns true.
    pub fn pump_messages() {
        if !CEF_CONTEXT_READY.load(Ordering::SeqCst) {
            return;
        }
        PUMP_SCHEDULE.clear();
        cef::do_message_loop_work();
        PUMP_SCHEDULE.schedule_idle_fallback(elapsed_us());
    }

    pub fn shutdown() {
        if !CEF_INITIALIZED.load(Ordering::SeqCst) {
            return;
        }

        log::info!("[browser] CEF shutdown: starting");

        // All CEF browser handles must be force-closed and dropped before
        // cef::shutdown(): CEF asserts that no BrowserContext instances
        // remain, and GPUI's entity lifecycle doesn't guarantee entities drop
        // before quit futures run. Pump a few times afterwards so CEF
        // processes the closes (Glass:crates/browser/src/cef_instance.rs:492).
        let closed = crate::tab::close_all_browsers();
        if closed > 0 {
            for _ in 0..10 {
                cef::do_message_loop_work();
            }
        }

        // Prevent regular pump scheduling from interfering.
        CEF_CONTEXT_READY.store(false, Ordering::SeqCst);

        // CEF_INITIALIZED must clear before CEF_INSTANCE: dropping the final
        // Arc<CefInstance> re-enters shutdown() via Drop while the CEF_INSTANCE
        // lock is held, and the flag check is what makes that re-entry a no-op.
        CEF_INITIALIZED.store(false, Ordering::SeqCst);
        *CEF_INSTANCE.lock() = None;

        cef::shutdown();
        log::info!("[browser] CEF shutdown: complete");

        *CEF_APP.lock() = None;
    }
}

impl Drop for CefInstance {
    fn drop(&mut self) {
        Self::shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::PumpSchedule;

    #[test]
    fn starts_idle() {
        let schedule = PumpSchedule::new();
        assert!(!schedule.is_due(0));
        assert_eq!(schedule.time_until_due_us(0), PumpSchedule::IDLE);
    }

    #[test]
    fn request_becomes_due_after_delay() {
        let schedule = PumpSchedule::new();
        schedule.request(5, 1_000);
        assert!(!schedule.is_due(1_000));
        assert!(!schedule.is_due(5_999));
        assert!(schedule.is_due(6_000));
        assert_eq!(schedule.time_until_due_us(2_000), 4_000);
    }

    #[test]
    fn earliest_request_wins() {
        let schedule = PumpSchedule::new();
        schedule.request(100, 0);
        schedule.request(5, 0);
        assert_eq!(schedule.time_until_due_us(0), 5_000);
        schedule.request(50, 0);
        assert_eq!(schedule.time_until_due_us(0), 5_000);
    }

    #[test]
    fn negative_delay_means_due_now() {
        let schedule = PumpSchedule::new();
        schedule.request(-3, 7_000);
        assert!(schedule.is_due(7_000));
    }

    #[test]
    fn huge_delay_does_not_overflow() {
        let schedule = PumpSchedule::new();
        schedule.request(i64::MAX, u64::MAX - 1);
        assert!(!schedule.is_due(u64::MAX - 1));
    }

    #[test]
    fn idle_fallback_applies_only_when_no_work_was_scheduled() {
        let schedule = PumpSchedule::new();
        schedule.clear();
        schedule.schedule_idle_fallback(1_000);
        assert_eq!(
            schedule.time_until_due_us(1_000),
            PumpSchedule::IDLE_FALLBACK_US
        );

        // A request that arrives during the pump survives the fallback.
        schedule.clear();
        schedule.request(0, 2_000);
        schedule.schedule_idle_fallback(2_000);
        assert!(schedule.is_due(2_000));
    }
}
