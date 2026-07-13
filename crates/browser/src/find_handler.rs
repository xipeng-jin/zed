//! CEF find handler, ported from `Glass:crates/browser/src/find_handler.rs`.
//! Forwards in-page find results to the tab's event stream for the find
//! overlay.

use crate::tab_backend::{EventSender, TabBackendEvent, send_event};
use cef::{Browser, FindHandler, ImplFindHandler, WrapFindHandler, rc::Rc as _, wrap_find_handler};

#[derive(Clone)]
pub(crate) struct OsrFindHandler {
    sender: EventSender,
}

impl OsrFindHandler {
    pub fn new(sender: EventSender) -> Self {
        Self { sender }
    }
}

wrap_find_handler! {
    pub(crate) struct FindHandlerBuilder {
        handler: OsrFindHandler,
    }

    impl FindHandler {
        fn on_find_result(
            &self,
            _browser: Option<&mut Browser>,
            _identifier: ::std::os::raw::c_int,
            count: ::std::os::raw::c_int,
            _selection_rect: Option<&cef::Rect>,
            active_match_ordinal: ::std::os::raw::c_int,
            _final_update: ::std::os::raw::c_int,
        ) {
            send_event(
                &self.handler.sender,
                TabBackendEvent::FindResult {
                    count,
                    active_match_ordinal,
                },
            );
        }
    }
}
