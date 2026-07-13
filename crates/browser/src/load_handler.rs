//! CEF load handler, ported from `Glass:crates/browser/src/load_handler.rs`.
//! Tracks loading state changes and load errors; URL and title tracking is
//! handled by the display handler instead.

use crate::tab_backend::{EventSender, TabBackendEvent, send_event};
use cef::{Browser, ImplLoadHandler, LoadHandler, WrapLoadHandler, rc::Rc as _, wrap_load_handler};

#[derive(Clone)]
pub(crate) struct OsrLoadHandler {
    sender: EventSender,
}

impl OsrLoadHandler {
    pub fn new(sender: EventSender) -> Self {
        Self { sender }
    }
}

wrap_load_handler! {
    pub(crate) struct LoadHandlerBuilder {
        handler: OsrLoadHandler,
    }

    impl LoadHandler {
        fn on_loading_state_change(
            &self,
            _browser: Option<&mut Browser>,
            is_loading: ::std::os::raw::c_int,
            can_go_back: ::std::os::raw::c_int,
            can_go_forward: ::std::os::raw::c_int,
        ) {
            send_event(&self.handler.sender, TabBackendEvent::LoadingStateChanged {
                is_loading: is_loading != 0,
                can_go_back: can_go_back != 0,
                can_go_forward: can_go_forward != 0,
            });
        }

        fn on_load_error(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut cef::Frame>,
            error_code: cef::Errorcode,
            error_text: Option<&cef::CefString>,
            failed_url: Option<&cef::CefString>,
        ) {
            let url = failed_url.map(|url| url.to_string()).unwrap_or_default();
            let text = error_text.map(|text| text.to_string()).unwrap_or_default();

            let code: &cef::sys::cef_errorcode_t = error_code.as_ref();
            log::warn!(
                "[browser::load_handler] on_load_error(url={}, text={}, code={:?})",
                url,
                text,
                code
            );

            send_event(&self.handler.sender, TabBackendEvent::LoadError {
                url,
                error_text: text,
            });
        }
    }
}
