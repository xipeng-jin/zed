//! CEF request handler, ported from
//! `Glass:crates/browser/src/request_handler.rs`.
//!
//! Mirrors the life-span handler's popup routing for link-opens targeting
//! tabs (middle-click, ctrl-click, `target=_blank` links resolved without
//! popup creation): tab-like dispositions are redirected into the browser
//! view's tab flow.

use crate::tab_backend::{EventSender, OpenDisposition, redirect_open_target_to_tab};
use cef::{
    Browser, ImplRequestHandler, RequestHandler, WindowOpenDisposition, WrapRequestHandler,
    rc::Rc as _, wrap_request_handler,
};

#[derive(Clone)]
pub(crate) struct OsrRequestHandler {
    sender: EventSender,
}

impl OsrRequestHandler {
    pub fn new(sender: EventSender) -> Self {
        Self { sender }
    }
}

wrap_request_handler! {
    pub(crate) struct RequestHandlerBuilder {
        handler: OsrRequestHandler,
    }

    impl RequestHandler {
        fn on_open_urlfrom_tab(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut cef::Frame>,
            target_url: Option<&cef::CefString>,
            target_disposition: WindowOpenDisposition,
            _user_gesture: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            let redirected = redirect_open_target_to_tab(
                &self.handler.sender,
                target_url.map(ToString::to_string),
                OpenDisposition::from(target_disposition),
            );
            if redirected { 1 } else { 0 }
        }
    }
}
