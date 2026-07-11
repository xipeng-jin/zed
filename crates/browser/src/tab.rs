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
use crate::client::{ClientBuilder, MANUAL_KEY_EVENT};
use crate::input;
use crate::render_handler::RenderState;
use crate::tab_backend::{EventReceiver, PaintOutput, TabBackend, TabBackendEvent, event_channel};
use anyhow::{Context as _, Result};
use cef::{ImplBrowser, ImplBrowserHost, ImplFrame};
use gpui::{Keystroke, Modifiers, MouseButton, Pixels, Point, ScrollDelta};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

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

/// Track a browser created outside `CefTab::start` — a native popup window —
/// so shutdown force-closes it and the message pump keeps its tight sleep cap
/// while it lives.
pub(crate) fn register_browser(browser: &cef::Browser) {
    let browser_id = browser.identifier();
    log::info!("[browser::tab] registering popup browser id={browser_id}");
    BROWSER_HANDLES
        .lock()
        .get_or_insert_with(HashMap::new)
        .insert(browser_id, browser.clone());
}

/// Release a tracked browser when its native window closes on its own.
pub(crate) fn unregister_browser(browser: &cef::Browser) {
    let browser_id = browser.identifier();
    if let Some(handles) = BROWSER_HANDLES.lock().as_mut()
        && handles.remove(&browser_id).is_some()
    {
        log::info!("[browser::tab] unregistered popup browser id={browser_id}");
    }
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

    fn with_focused_frame(&self, callback: impl FnOnce(&cef::Frame)) {
        self.with_browser(|browser| {
            if let Some(frame) = browser.focused_frame() {
                callback(&frame);
            }
        });
    }

    /// Send a key event flagged as app-sent, so the keyboard handler's native
    /// suppression (`client.rs`) lets it through to the page. The flag works
    /// because `send_key_event` delivers to `on_pre_key_event` synchronously
    /// on this same thread.
    fn send_key_event(&self, event: &cef::KeyEvent) {
        self.with_host(|host| {
            MANUAL_KEY_EVENT.store(true, Ordering::Relaxed);
            host.send_key_event(Some(event));
            MANUAL_KEY_EVENT.store(false, Ordering::Relaxed);
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

    fn stop(&mut self) {
        self.with_browser(|browser| browser.stop_load());
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

    fn set_hidden(&mut self, hidden: bool) {
        self.with_host(|host| {
            host.was_hidden(if hidden { 1 } else { 0 });
        });
    }

    fn send_mouse_down(
        &mut self,
        position: Point<Pixels>,
        button: MouseButton,
        click_count: usize,
        modifiers: Modifiers,
    ) {
        let event = input::mouse_event(position, input::convert_modifiers(&modifiers));
        let button = input::convert_mouse_button(button);
        let mouse_up = 0;
        self.with_host(|host| {
            host.send_mouse_click_event(Some(&event), button, mouse_up, click_count as i32);
        });
    }

    fn send_mouse_up(
        &mut self,
        position: Point<Pixels>,
        button: MouseButton,
        modifiers: Modifiers,
    ) {
        let event = input::mouse_event(position, input::convert_modifiers(&modifiers));
        let button = input::convert_mouse_button(button);
        let (mouse_up, click_count) = (1, 1);
        self.with_host(|host| {
            host.send_mouse_click_event(Some(&event), button, mouse_up, click_count);
        });
    }

    fn send_mouse_move(
        &mut self,
        position: Point<Pixels>,
        pressed_button: Option<MouseButton>,
        modifiers: Modifiers,
    ) {
        let modifiers =
            input::convert_modifiers(&modifiers) | input::pressed_button_flags(pressed_button);
        let event = input::mouse_event(position, modifiers);
        self.with_host(|host| {
            host.send_mouse_move_event(Some(&event), 0);
        });
    }

    fn send_scroll_wheel(
        &mut self,
        position: Point<Pixels>,
        delta: ScrollDelta,
        modifiers: Modifiers,
    ) {
        let event = input::mouse_event(position, input::convert_modifiers(&modifiers));
        let (delta_x, delta_y) = input::scroll_delta_to_pixels(delta);
        self.with_host(|host| {
            host.send_mouse_wheel_event(Some(&event), delta_x, delta_y);
        });
    }

    fn send_key_down(&mut self, keystroke: &Keystroke, is_held: bool) {
        self.send_key_event(&input::convert_key_event(keystroke, true));

        if input::should_send_char_event(keystroke, is_held)
            && let Some(char_event) = input::create_char_event(keystroke)
        {
            self.send_key_event(&char_event);
        }
    }

    fn send_key_up(&mut self, keystroke: &Keystroke) {
        self.send_key_event(&input::convert_key_event(keystroke, false));
    }

    fn ime_set_composition(&mut self, text: &str, selected_range: Option<std::ops::Range<usize>>) {
        let utf16_len = text.encode_utf16().count() as u32;
        self.with_host(|host| {
            let text = cef::CefString::from(text);
            // CEF's C-API shim rejects null range pointers, so "no
            // replacement" is Chromium's invalid range and a missing
            // selection defaults to a caret after the composition.
            let replacement_range = cef::Range {
                from: u32::MAX,
                to: u32::MAX,
            };
            let selected_range = selected_range
                .map(|range| cef::Range {
                    from: range.start as u32,
                    to: range.end as u32,
                })
                .unwrap_or(cef::Range {
                    from: utf16_len,
                    to: utf16_len,
                });
            host.ime_set_composition(
                Some(&text),
                None,
                Some(&replacement_range),
                Some(&selected_range),
            );
        });
    }

    // Committed text is delivered as CHAR key events rather than
    // `ime_commit_text`: commits often arrive with no composition in flight
    // (e.g. plain insertText), and CEF drops commit calls outside one
    // (`Glass:crates/browser/src/tab.rs:598`).
    fn ime_commit_text(&mut self, text: &str) {
        for character in text.encode_utf16() {
            self.send_key_event(&cef::KeyEvent {
                type_: cef::KeyEventType::CHAR,
                modifiers: 0,
                windows_key_code: character as i32,
                character,
                unmodified_character: character,
                focus_on_editable_field: 1,
                ..Default::default()
            });
        }
    }

    fn ime_cancel_composition(&mut self) {
        self.with_host(|host| {
            host.ime_cancel_composition();
        });
    }

    fn find(&mut self, query: &str, forward: bool, match_case: bool, find_next: bool) {
        self.with_host(|host| {
            let query = cef::CefString::from(query);
            host.find(
                Some(&query),
                if forward { 1 } else { 0 },
                if match_case { 1 } else { 0 },
                if find_next { 1 } else { 0 },
            );
        });
    }

    fn stop_finding(&mut self, clear_selection: bool) {
        self.with_host(|host| {
            host.stop_finding(if clear_selection { 1 } else { 0 });
        });
    }

    fn undo(&mut self) {
        self.with_focused_frame(|frame| frame.undo());
    }

    fn redo(&mut self) {
        self.with_focused_frame(|frame| frame.redo());
    }

    fn cut(&mut self) {
        self.with_focused_frame(|frame| frame.cut());
    }

    fn copy(&mut self) {
        self.with_focused_frame(|frame| frame.copy());
    }

    fn paste(&mut self) {
        self.with_focused_frame(|frame| frame.paste());
    }

    fn delete(&mut self) {
        self.with_focused_frame(|frame| frame.del());
    }

    fn select_all(&mut self) {
        self.with_focused_frame(|frame| frame.select_all());
    }

    fn start_download(&mut self, url: &str) {
        self.with_host(|host| {
            let url = cef::CefString::from(url);
            host.start_download(Some(&url));
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
