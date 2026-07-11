//! CEF life-span handler, ported (minimal M1 subset) from
//! `Glass:crates/browser/src/life_span_handler.rs`. The full popup/OAuth flow
//! — redirecting tab-like dispositions into the browser tab strip and hosting
//! `NEW_POPUP` login windows as real native windows — is ticket #15 (M2).

use crate::tab_backend::{EventSender, TabBackendEvent, send_event};
use cef::{
    Browser, ImplLifeSpanHandler, LifeSpanHandler, WrapLifeSpanHandler, rc::Rc as _,
    wrap_life_span_handler,
};

#[derive(Clone)]
pub(crate) struct OsrLifeSpanHandler {
    sender: EventSender,
}

impl OsrLifeSpanHandler {
    pub fn new(sender: EventSender) -> Self {
        Self { sender }
    }
}

wrap_life_span_handler! {
    pub(crate) struct LifeSpanHandlerBuilder {
        handler: OsrLifeSpanHandler,
    }

    impl LifeSpanHandler {
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut cef::Frame>,
            _popup_id: ::std::os::raw::c_int,
            target_url: Option<&cef::CefString>,
            _target_frame_name: Option<&cef::CefString>,
            _target_disposition: cef::WindowOpenDisposition,
            _user_gesture: ::std::os::raw::c_int,
            _popup_features: Option<&cef::PopupFeatures>,
            _window_info: Option<&mut cef::WindowInfo>,
            _client: Option<&mut Option<cef::Client>>,
            _settings: Option<&mut cef::BrowserSettings>,
            _extra_info: Option<&mut Option<cef::DictionaryValue>>,
            _no_javascript_access: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            // Cancel all popups for now: by default CEF would create the popup
            // with this tab's client, so the popup's off-screen paints would
            // corrupt this tab's render state. Ticket #15 ports the real
            // routing (tab redirect + native windows with a separate client).
            if let Some(url) = target_url {
                log::info!("[browser::life_span] suppressing popup until ticket #15: {url}");
            }
            1
        }

        fn on_after_created(&self, _browser: Option<&mut Browser>) {
            send_event(&self.handler.sender, TabBackendEvent::Created);
        }

        fn do_close(&self, _browser: Option<&mut Browser>) -> ::std::os::raw::c_int {
            0 // Allow close.
        }
    }
}

impl LifeSpanHandlerBuilder {
    pub fn build(handler: OsrLifeSpanHandler) -> cef::LifeSpanHandler {
        Self::new(handler)
    }
}
