//! CEF client, ported (M1 subset) from `Glass:crates/browser/src/client.rs`.
//! Ties together the render, load, display, life-span, and keyboard handlers.
//! Download, find, request, context-menu, and permission handlers join in M2.

use crate::display_handler::{DisplayHandlerBuilder, OsrDisplayHandler};
use crate::life_span_handler::{LifeSpanHandlerBuilder, OsrLifeSpanHandler};
use crate::load_handler::{LoadHandlerBuilder, OsrLoadHandler};
use crate::render_handler::{OsrRenderHandler, RenderHandlerBuilder, RenderState};
use crate::tab_backend::EventSender;
#[cfg(target_os = "windows")]
use cef::sys::tagMSG;
use cef::{
    Browser, Client, DisplayHandler, ImplClient, ImplKeyboardHandler, KeyEvent, KeyboardHandler,
    LifeSpanHandler, LoadHandler, RenderHandler, WrapClient, WrapKeyboardHandler, rc::Rc as _,
    wrap_client, wrap_keyboard_handler,
};
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

// Off-screen browser views receive input through the app's routing only: raw
// key events are forwarded explicitly for browser-routed keys (and, with
// ticket #16, text input committed through the IME APIs). OS-delivered key
// events would duplicate that, so they are suppressed unless flagged as
// app-sent (`CefTab::send_key_event` sets the flag around each send).
pub(crate) static MANUAL_KEY_EVENT: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "windows")]
type KeyboardOsEvent<'a> = Option<&'a mut tagMSG>;
#[cfg(target_os = "linux")]
type KeyboardOsEvent<'a> = Option<&'a mut cef::sys::_XEvent>;
#[cfg(target_os = "macos")]
type KeyboardOsEvent<'a> = *mut u8;

#[derive(Clone)]
struct OsrKeyboardHandler;

wrap_keyboard_handler! {
    struct KeyboardHandlerBuilder {
        handler: OsrKeyboardHandler,
    }

    impl KeyboardHandler {
        fn on_pre_key_event(
            &self,
            _browser: Option<&mut Browser>,
            _event: Option<&KeyEvent>,
            _os_event: KeyboardOsEvent<'_>,
            _is_keyboard_shortcut: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            let is_manual = MANUAL_KEY_EVENT.load(Ordering::Relaxed);
            if is_manual { 0 } else { 1 }
        }
    }
}

impl KeyboardHandlerBuilder {
    fn build() -> cef::KeyboardHandler {
        Self::new(OsrKeyboardHandler)
    }
}

wrap_client! {
    pub(crate) struct ClientBuilder {
        render_handler: RenderHandler,
        load_handler: LoadHandler,
        display_handler: DisplayHandler,
        life_span_handler: LifeSpanHandler,
        keyboard_handler: KeyboardHandler,
    }

    impl Client {
        fn render_handler(&self) -> Option<cef::RenderHandler> {
            Some(self.render_handler.clone())
        }

        fn load_handler(&self) -> Option<cef::LoadHandler> {
            Some(self.load_handler.clone())
        }

        fn display_handler(&self) -> Option<cef::DisplayHandler> {
            Some(self.display_handler.clone())
        }

        fn life_span_handler(&self) -> Option<cef::LifeSpanHandler> {
            Some(self.life_span_handler.clone())
        }

        fn keyboard_handler(&self) -> Option<cef::KeyboardHandler> {
            Some(self.keyboard_handler.clone())
        }
    }
}

impl ClientBuilder {
    pub fn build(render_state: Arc<Mutex<RenderState>>, event_sender: EventSender) -> cef::Client {
        let render_handler = OsrRenderHandler::new(render_state, event_sender.clone());
        let load_handler = OsrLoadHandler::new(event_sender.clone());
        let display_handler = OsrDisplayHandler::new(event_sender.clone());
        let life_span_handler = OsrLifeSpanHandler::new(event_sender);
        Self::new(
            RenderHandlerBuilder::build(render_handler),
            LoadHandlerBuilder::build(load_handler),
            DisplayHandlerBuilder::build(display_handler),
            LifeSpanHandlerBuilder::build(life_span_handler),
            KeyboardHandlerBuilder::build(),
        )
    }
}
