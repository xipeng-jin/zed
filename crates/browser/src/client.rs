//! CEF client, ported (M2 subset) from `Glass:crates/browser/src/client.rs`.
//! Ties together the render, load, display, life-span, keyboard, download,
//! find, context-menu, request, and permission handlers, and receives the
//! render process's text-input state and page-chrome messages.

use crate::context_menu_handler::{ContextMenuHandlerBuilder, OsrContextMenuHandler};
use crate::display_handler::{DisplayHandlerBuilder, OsrDisplayHandler};
use crate::download_handler::{DownloadHandlerBuilder, OsrDownloadHandler};
use crate::find_handler::{FindHandlerBuilder, OsrFindHandler};
use crate::life_span_handler::{
    LifeSpanHandlerBuilder, OsrLifeSpanHandler, PopupLifeSpanHandler, PopupLifeSpanHandlerBuilder,
};
use crate::load_handler::{LoadHandlerBuilder, OsrLoadHandler};
use crate::page_chrome::extract_page_chrome_from_message;
use crate::permission_handler::{OsrPermissionHandler, PermissionHandlerBuilder};
use crate::render_handler::{OsrRenderHandler, RenderHandlerBuilder, RenderState};
use crate::request_handler::{OsrRequestHandler, RequestHandlerBuilder};
use crate::tab_backend::{EventSender, TabBackendEvent, send_event};
use crate::text_input::extract_text_input_state_from_message;
#[cfg(target_os = "windows")]
use cef::sys::tagMSG;
use cef::{
    Browser, Client, ContextMenuHandler, DisplayHandler, DownloadHandler, FindHandler, ImplClient,
    ImplKeyboardHandler, KeyEvent, KeyboardHandler, LifeSpanHandler, LoadHandler,
    PermissionHandler, RenderHandler, RequestHandler, WrapClient, WrapKeyboardHandler, rc::Rc as _,
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

// Popup windows are real native windows, so their key events come through the
// OS natively and must NOT be suppressed.
#[derive(Clone)]
struct PopupKeyboardHandler;

wrap_keyboard_handler! {
    struct PopupKeyboardHandlerBuilder {
        handler: PopupKeyboardHandler,
    }

    impl KeyboardHandler {
        fn on_pre_key_event(
            &self,
            _browser: Option<&mut Browser>,
            _event: Option<&KeyEvent>,
            _os_event: KeyboardOsEvent<'_>,
            _is_keyboard_shortcut: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            0 // Allow all native key events through.
        }
    }
}

impl PopupKeyboardHandlerBuilder {
    fn build() -> cef::KeyboardHandler {
        Self::new(PopupKeyboardHandler)
    }
}

wrap_client! {
    pub(crate) struct ClientBuilder {
        render_handler: RenderHandler,
        load_handler: LoadHandler,
        display_handler: DisplayHandler,
        life_span_handler: LifeSpanHandler,
        keyboard_handler: KeyboardHandler,
        download_handler: DownloadHandler,
        find_handler: FindHandler,
        context_menu_handler: ContextMenuHandler,
        request_handler: RequestHandler,
        permission_handler: PermissionHandler,
        event_sender: EventSender,
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

        fn download_handler(&self) -> Option<cef::DownloadHandler> {
            Some(self.download_handler.clone())
        }

        fn find_handler(&self) -> Option<cef::FindHandler> {
            Some(self.find_handler.clone())
        }

        fn context_menu_handler(&self) -> Option<cef::ContextMenuHandler> {
            Some(self.context_menu_handler.clone())
        }

        fn request_handler(&self) -> Option<cef::RequestHandler> {
            Some(self.request_handler.clone())
        }

        fn permission_handler(&self) -> Option<cef::PermissionHandler> {
            Some(self.permission_handler.clone())
        }

        fn on_process_message_received(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut cef::Frame>,
            _source_process: cef::ProcessId,
            message: Option<&mut cef::ProcessMessage>,
        ) -> ::std::os::raw::c_int {
            let Some(message) = message else {
                return 0;
            };

            if let Some(text_input_state) = extract_text_input_state_from_message(message) {
                send_event(
                    &self.event_sender,
                    TabBackendEvent::TextInputStateChanged(text_input_state),
                );
                return 1;
            }

            if let Some(page_chrome) = extract_page_chrome_from_message(message) {
                send_event(
                    &self.event_sender,
                    TabBackendEvent::PageChromeChanged(page_chrome),
                );
                return 1;
            }

            0
        }
    }
}

impl ClientBuilder {
    pub fn build(render_state: Arc<Mutex<RenderState>>, event_sender: EventSender) -> cef::Client {
        let life_span_handler =
            LifeSpanHandlerBuilder::build(OsrLifeSpanHandler::new(event_sender.clone()));
        Self::build_inner(
            render_state,
            event_sender,
            life_span_handler,
            KeyboardHandlerBuilder::build(),
        )
    }

    /// Client for a native popup window (OAuth/login). Differs from the tab
    /// client in its life-span handler (tracks the popup in the handle
    /// registry) and its keyboard handler (native key events pass through).
    pub fn build_for_popup(
        render_state: Arc<Mutex<RenderState>>,
        event_sender: EventSender,
    ) -> cef::Client {
        let life_span_handler =
            PopupLifeSpanHandlerBuilder::build(PopupLifeSpanHandler::new(event_sender.clone()));
        Self::build_inner(
            render_state,
            event_sender,
            life_span_handler,
            PopupKeyboardHandlerBuilder::build(),
        )
    }

    fn build_inner(
        render_state: Arc<Mutex<RenderState>>,
        event_sender: EventSender,
        life_span_handler: cef::LifeSpanHandler,
        keyboard_handler: cef::KeyboardHandler,
    ) -> cef::Client {
        let render_handler = OsrRenderHandler::new(render_state, event_sender.clone());
        let load_handler = OsrLoadHandler::new(event_sender.clone());
        let display_handler = OsrDisplayHandler::new(event_sender.clone());
        let download_handler = OsrDownloadHandler::new(event_sender.clone());
        let find_handler = OsrFindHandler::new(event_sender.clone());
        let request_handler = OsrRequestHandler::new(event_sender.clone());
        let context_menu_handler = OsrContextMenuHandler::new(event_sender.clone());
        let permission_handler = OsrPermissionHandler::new();
        Self::new(
            RenderHandlerBuilder::build(render_handler),
            LoadHandlerBuilder::build(load_handler),
            DisplayHandlerBuilder::build(display_handler),
            life_span_handler,
            keyboard_handler,
            DownloadHandlerBuilder::build(download_handler),
            FindHandlerBuilder::build(find_handler),
            ContextMenuHandlerBuilder::build(context_menu_handler),
            RequestHandlerBuilder::build(request_handler),
            PermissionHandlerBuilder::build(permission_handler),
            event_sender,
        )
    }
}
