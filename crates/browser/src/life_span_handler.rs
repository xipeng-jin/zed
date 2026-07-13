//! CEF life-span handler, ported from
//! `Glass:crates/browser/src/life_span_handler.rs`.
//!
//! Popup routing: tab-like dispositions are cancelled and redirected into the
//! browser view's tab flow; genuine popups (OAuth/login windows) are allowed
//! as real native engine windows so their opener semantics stay intact
//! (plan §7 M2 step 6, risk R3).

use crate::client::ClientBuilder;
use crate::render_handler::RenderState;
use crate::tab_backend::{
    EventSender, OpenDisposition, TabBackendEvent, redirect_open_target_to_tab, send_event,
};
use cef::{
    Browser, ImplLifeSpanHandler, LifeSpanHandler, WrapLifeSpanHandler, rc::Rc as _,
    wrap_life_span_handler,
};
use parking_lot::Mutex;
use std::sync::Arc;

/// A dedicated client for a native popup window. Popups must never share the
/// opener tab's client: its render handler would corrupt the opener's
/// off-screen render state. The popup's event receiver is dropped immediately
/// — a native window manages itself; the app tracks nothing beyond the handle
/// registry.
fn popup_client() -> cef::Client {
    let render_state = Arc::new(Mutex::new(RenderState::default()));
    let (popup_sender, _popup_receiver) = crate::tab_backend::event_channel();
    ClientBuilder::build_for_popup(render_state, popup_sender)
}

/// Shared `on_before_popup` routing for tab and popup clients: redirect
/// tab-like dispositions into the app's tab flow (the popup client's sender is
/// dropped, so its redirects are no-ops — matching Glass, where opens from a
/// popup window go nowhere), host genuine popups as native windows, and cancel
/// the rest.
fn route_popup(
    sender: &EventSender,
    target_url: Option<&cef::CefString>,
    target_disposition: cef::WindowOpenDisposition,
    window_info: Option<&mut cef::WindowInfo>,
    client: Option<&mut Option<cef::Client>>,
) -> ::std::os::raw::c_int {
    let disposition = OpenDisposition::from(target_disposition);
    let target_url = target_url.map(ToString::to_string);

    if redirect_open_target_to_tab(sender, target_url.clone(), disposition) {
        log::info!(
            "[browser::life_span] redirecting popup disposition {disposition:?} into browser tab flow"
        );
        return 1;
    }

    if !disposition.allow_native_popup() {
        // Deviation from Glass (which lets these through with the opener's
        // client): a disposition that is neither tab-like nor a real popup
        // would be created windowless on the opener's client and corrupt its
        // render state, so cancel it.
        log::info!("[browser::life_span] cancelling popup with disposition {disposition:?}");
        return 1;
    }

    // Ensure the popup is hosted as a real native window and not as an
    // off-screen child tied to the opener tab's event/render pipeline.
    if let Some(window_info) = window_info {
        window_info.windowless_rendering_enabled = 0;
        window_info.shared_texture_enabled = 0;
    }

    if let Some(client) = client {
        *client = Some(popup_client());
    }

    if let Some(url) = target_url.filter(|url| !url.is_empty()) {
        log::info!("[browser::life_span] allowing native popup navigation: {url}");
    }

    0 // Allow popup creation.
}

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
            target_disposition: cef::WindowOpenDisposition,
            _user_gesture: ::std::os::raw::c_int,
            _popup_features: Option<&cef::PopupFeatures>,
            window_info: Option<&mut cef::WindowInfo>,
            client: Option<&mut Option<cef::Client>>,
            _settings: Option<&mut cef::BrowserSettings>,
            _extra_info: Option<&mut Option<cef::DictionaryValue>>,
            _no_javascript_access: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            route_popup(
                &self.handler.sender,
                target_url,
                target_disposition,
                window_info,
                client,
            )
        }

        fn on_after_created(&self, _browser: Option<&mut Browser>) {
            send_event(&self.handler.sender, TabBackendEvent::Created);
        }

        fn do_close(&self, _browser: Option<&mut Browser>) -> ::std::os::raw::c_int {
            0 // Allow close.
        }
    }
}

/// Life-span handler for native popup windows. Unlike Glass, popup browsers
/// are tracked in the global handle registry: shutdown must force-close them
/// (CEF asserts all browsers are gone before `cef::shutdown()`), and the
/// message pump keeps its tight sleep cap while any popup lives.
#[derive(Clone)]
pub(crate) struct PopupLifeSpanHandler {
    sender: EventSender,
}

impl PopupLifeSpanHandler {
    pub fn new(sender: EventSender) -> Self {
        Self { sender }
    }
}

wrap_life_span_handler! {
    pub(crate) struct PopupLifeSpanHandlerBuilder {
        handler: PopupLifeSpanHandler,
    }

    impl LifeSpanHandler {
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut cef::Frame>,
            _popup_id: ::std::os::raw::c_int,
            target_url: Option<&cef::CefString>,
            _target_frame_name: Option<&cef::CefString>,
            target_disposition: cef::WindowOpenDisposition,
            _user_gesture: ::std::os::raw::c_int,
            _popup_features: Option<&cef::PopupFeatures>,
            window_info: Option<&mut cef::WindowInfo>,
            client: Option<&mut Option<cef::Client>>,
            _settings: Option<&mut cef::BrowserSettings>,
            _extra_info: Option<&mut Option<cef::DictionaryValue>>,
            _no_javascript_access: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            route_popup(
                &self.handler.sender,
                target_url,
                target_disposition,
                window_info,
                client,
            )
        }

        fn on_after_created(&self, browser: Option<&mut Browser>) {
            if let Some(browser) = browser {
                crate::tab::register_browser(browser);
            }
        }

        fn on_before_close(&self, browser: Option<&mut Browser>) {
            if let Some(browser) = browser {
                crate::tab::unregister_browser(browser);
            }
        }

        fn do_close(&self, _browser: Option<&mut Browser>) -> ::std::os::raw::c_int {
            0 // Allow close.
        }
    }
}
