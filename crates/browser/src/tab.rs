//! `CefTab`: the CEF implementation of the tab-backend seam, ported from
//! `Glass:crates/browser/src/tab.rs` (minus the GPUI entity wrapper — tab
//! state lives above the seam in `browser_view.rs`).
//!
//! CEF browser handles are stored in a centralized global registry
//! (`BROWSER_HANDLES`) rather than directly on `CefTab`. This allows CEF
//! shutdown to take all handles, close browsers, and release ref counts before
//! calling `cef::shutdown()` — regardless of whether GPUI has dropped the
//! owning entities yet.

use crate::cef_instance::CefInstance;
use crate::client::ClientBuilder;
use crate::render_handler::RenderState;
use crate::tab_backend::{
    EventReceiver, PaintOutput, TabBackend, TabBackendEvent, event_channel,
};
use anyhow::{Context as _, Result};
use cef::{ImplBrowser, ImplBrowserHost, ImplFrame};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

/// All live CEF browser handles, keyed by browser ID.
static BROWSER_HANDLES: Mutex<Option<HashMap<i32, cef::Browser>>> = Mutex::new(None);

/// Force-close all tracked browsers and release their CEF handles.
///
/// This takes every handle out of the global map, calls
/// `host.close_browser(force_close=1)` on each, then drops them all. After
/// this returns, no Rust code holds `cef::Browser` references, so CEF's
/// internal `BrowserContext` ref counts can reach zero.
pub(crate) fn close_all_browsers() -> usize {
    let handles = BROWSER_HANDLES.lock().take().unwrap_or_default();
    let count = handles.len();
    if count > 0 {
        log::info!("[browser::tab] close_all_browsers: closing {count} browser(s)");
    }
    for (id, browser) in handles {
        match browser.host() {
            Some(host) => {
                log::trace!("[browser::tab] close_all_browsers: id={id}, requesting close");
                host.close_browser(1);
            }
            None => {
                log::trace!("[browser::tab] close_all_browsers: id={id} has no host");
            }
        }
    }
    count
}

/// Whether any engine browser is alive. The message pump tightens its sleep
/// cap while this holds, to keep frame and input latency low.
pub(crate) fn has_live_browsers() -> bool {
    BROWSER_HANDLES
        .lock()
        .as_ref()
        .is_some_and(|handles| !handles.is_empty())
}

pub struct CefTab {
    browser_id: Option<i32>,
    client: cef::Client,
    render_state: Arc<Mutex<RenderState>>,
    event_receiver: EventReceiver,
}

impl CefTab {
    pub fn new() -> Self {
        let render_state = Arc::new(Mutex::new(RenderState::default()));
        let (sender, receiver) = event_channel();
        let client = ClientBuilder::build(render_state.clone(), sender);
        Self {
            browser_id: None,
            client,
            render_state,
            event_receiver: receiver,
        }
    }

    /// Access the CEF browser handle from the global registry. Returns `None`
    /// if the browser was never created or was already taken by shutdown or
    /// `close`.
    fn with_browser<R>(&self, callback: impl FnOnce(&cef::Browser) -> R) -> Option<R> {
        let browser_id = self.browser_id?;
        let handles = BROWSER_HANDLES.lock();
        handles.as_ref()?.get(&browser_id).map(callback)
    }

    fn with_host(&self, callback: impl FnOnce(&cef::BrowserHost)) {
        self.with_browser(|browser| {
            if let Some(host) = browser.host() {
                callback(&host);
            }
        });
    }
}

impl TabBackend for CefTab {
    fn engine_ready(&self) -> bool {
        CefInstance::is_context_ready()
    }

    fn start(&mut self, url: &str) -> Result<()> {
        if self.browser_id.is_some() {
            return Ok(());
        }

        // Software OSR: `shared_texture_enabled` stays unset so CEF paints
        // through `on_paint` (ADR-0002; the GPU shared-texture path is
        // macOS-only and returns behind the presenter seam in M3).
        let window_info = cef::WindowInfo {
            windowless_rendering_enabled: 1,
            ..Default::default()
        };

        let browser_settings = cef::BrowserSettings {
            windowless_frame_rate: 60,
            ..Default::default()
        };

        let url = cef::CefString::from(url);

        let browser = cef::browser_host_create_browser_sync(
            Some(&window_info),
            Some(&mut self.client.clone()),
            Some(&url),
            Some(&browser_settings),
            None,
            None,
        )
        .context("Failed to create CEF browser")?;

        let browser_id = browser.identifier();
        log::info!("[browser::tab] created browser id={browser_id}");

        BROWSER_HANDLES
            .lock()
            .get_or_insert_with(HashMap::new)
            .insert(browser_id, browser);
        self.browser_id = Some(browser_id);

        self.with_host(|host| {
            host.was_resized();
        });

        Ok(())
    }

    fn is_started(&self) -> bool {
        self.browser_id.is_some()
    }

    fn navigate(&mut self, url: &str) {
        self.with_browser(|browser| {
            if let Some(frame) = browser.main_frame() {
                let url_string = cef::CefString::from(url);
                frame.load_url(Some(&url_string));
            }
        });
    }

    fn reload(&mut self) {
        self.with_browser(|browser| browser.reload());
    }

    fn go_back(&mut self) {
        self.with_browser(|browser| browser.go_back());
    }

    fn go_forward(&mut self) {
        self.with_browser(|browser| browser.go_forward());
    }

    fn set_viewport(&mut self, width: u32, height: u32, scale_factor: f32) {
        {
            let mut state = self.render_state.lock();
            state.width = width.max(1);
            state.height = height.max(1);
            state.scale_factor = scale_factor;
        }
        self.with_host(|host| {
            host.was_resized();
        });
    }

    fn set_focus(&mut self, focused: bool) {
        self.with_host(|host| {
            host.set_focus(if focused { 1 } else { 0 });
        });
    }

    fn close(&mut self) {
        if let Some(browser_id) = self.browser_id.take() {
            // If shutdown already took the handle via close_all_browsers(),
            // the remove returns None and all CEF API calls are skipped.
            let browser = BROWSER_HANDLES
                .lock()
                .as_mut()
                .and_then(|handles| handles.remove(&browser_id));
            if let Some(browser) = browser
                && let Some(host) = browser.host()
            {
                log::trace!("[browser::tab] close: id={browser_id}, requesting close");
                host.close_browser(1);
            }
        }
        self.render_state.lock().frame = None;
    }

    fn try_recv_event(&mut self) -> Option<TabBackendEvent> {
        self.event_receiver.try_recv().ok()
    }

    fn take_paint_output(&mut self) -> Option<PaintOutput> {
        self.render_state
            .lock()
            .frame
            .take()
            .map(PaintOutput::Software)
    }
}

impl Drop for CefTab {
    fn drop(&mut self) {
        self.close();
    }
}
