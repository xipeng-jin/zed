//! CEF display handler, ported from
//! `Glass:crates/browser/src/display_handler.rs`. Handles URL changes, title
//! changes, loading progress, and favicon updates.

use crate::tab_backend::{EventSender, TabBackendEvent, send_event};
use cef::{
    Browser, CefStringList, DisplayHandler, ImplDisplayHandler, ImplFrame, WrapDisplayHandler,
    rc::Rc as _, wrap_display_handler,
};

#[derive(Clone)]
pub(crate) struct OsrDisplayHandler {
    sender: EventSender,
}

impl OsrDisplayHandler {
    pub fn new(sender: EventSender) -> Self {
        Self { sender }
    }
}

wrap_display_handler! {
    pub(crate) struct DisplayHandlerBuilder {
        handler: OsrDisplayHandler,
    }

    impl DisplayHandler {
        fn on_address_change(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut cef::Frame>,
            url: Option<&cef::CefString>,
        ) {
            if frame.is_some_and(|frame| frame.is_main() == 0) {
                return;
            }
            if let Some(url) = url {
                let url_string = url.to_string();
                if !url_string.is_empty() {
                    send_event(
                        &self.handler.sender,
                        TabBackendEvent::AddressChanged(url_string),
                    );
                }
            }
        }

        fn on_title_change(
            &self,
            _browser: Option<&mut Browser>,
            title: Option<&cef::CefString>,
        ) {
            if let Some(title) = title {
                send_event(
                    &self.handler.sender,
                    TabBackendEvent::TitleChanged(title.to_string()),
                );
            }
        }

        fn on_loading_progress_change(
            &self,
            _browser: Option<&mut Browser>,
            progress: f64,
        ) {
            send_event(
                &self.handler.sender,
                TabBackendEvent::LoadingProgress(progress),
            );
        }

        fn on_fullscreen_mode_change(
            &self,
            _browser: Option<&mut Browser>,
            fullscreen: ::std::os::raw::c_int,
        ) {
            // Implementing this callback prevents CEF from triggering native
            // window fullscreen when a website calls requestFullscreen(). The
            // web content still renders fullscreen within the browser view.
            log::trace!(
                "[browser::display] on_fullscreen_mode_change: fullscreen={}",
                fullscreen != 0,
            );
        }

        fn on_favicon_urlchange(
            &self,
            _browser: Option<&mut Browser>,
            icon_urls: Option<&mut CefStringList>,
        ) {
            if let Some(icon_urls) = icon_urls {
                // CefStringList::clone() is broken for BorrowedMut variants
                // (opaque pointer types): the clone converts to a Borrowed
                // variant which loses the pointer. Use std::mem::take to get
                // the original BorrowedMut, which preserves the pointer and
                // iterates correctly. The original slot gets a new empty list.
                let taken = std::mem::take(icon_urls);
                let urls: Vec<String> = taken.into_iter().collect();
                send_event(
                    &self.handler.sender,
                    TabBackendEvent::FaviconUrlsChanged(urls),
                );
            }
        }
    }
}
