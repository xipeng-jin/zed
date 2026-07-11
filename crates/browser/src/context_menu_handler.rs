//! CEF context menu handler, ported from
//! `Glass:crates/browser/src/context_menu_handler.rs`. In windowless (OSR)
//! mode CEF cannot display its own context menu, so the request is
//! intercepted here: the page context is extracted and forwarded to the
//! browser view, which renders a Zed-style menu (`context_menu.rs`).
//!
//! `on_before_context_menu` must NOT be overridden: CEF's default populates
//! the menu model with standard items, and CEF skips `run_context_menu`
//! entirely when the model is empty — overriding it with an empty model means
//! the app-rendered path never triggers (plan §3.6, the documented trap).

use crate::context_menu::ContextMenuContext;
use crate::tab_backend::{EventSender, TabBackendEvent, send_event};
use cef::{
    Browser, ContextMenuHandler, ContextMenuParams, Frame, ImplContextMenuHandler,
    ImplContextMenuParams, ImplRunContextMenuCallback, MenuModel, RunContextMenuCallback,
    WrapContextMenuHandler, rc::Rc as _, wrap_context_menu_handler,
};

#[derive(Clone)]
pub(crate) struct OsrContextMenuHandler {
    sender: EventSender,
}

impl OsrContextMenuHandler {
    pub fn new(sender: EventSender) -> Self {
        Self { sender }
    }

    fn extract_context(params: &ContextMenuParams) -> ContextMenuContext {
        let link_url = {
            let userfree = params.link_url();
            let url = cef::CefString::from(&userfree).to_string();
            if url.is_empty() { None } else { Some(url) }
        };

        let selection_text = {
            let userfree = params.selection_text();
            let text = cef::CefString::from(&userfree).to_string();
            if text.is_empty() { None } else { Some(text) }
        };

        type EditStateFlags = cef::sys::cef_context_menu_edit_state_flags_t;
        let edit_flags: EditStateFlags = params.edit_state_flags().into();
        let can = |flag: EditStateFlags| (edit_flags & flag).0 != 0;

        ContextMenuContext {
            link_url,
            selection_text,
            is_editable: params.is_editable() != 0,
            can_undo: can(EditStateFlags::CM_EDITFLAG_CAN_UNDO),
            can_redo: can(EditStateFlags::CM_EDITFLAG_CAN_REDO),
            can_cut: can(EditStateFlags::CM_EDITFLAG_CAN_CUT),
            can_copy: can(EditStateFlags::CM_EDITFLAG_CAN_COPY),
            can_paste: can(EditStateFlags::CM_EDITFLAG_CAN_PASTE),
            can_delete: can(EditStateFlags::CM_EDITFLAG_CAN_DELETE),
            can_select_all: can(EditStateFlags::CM_EDITFLAG_CAN_SELECT_ALL),
        }
    }
}

wrap_context_menu_handler! {
    pub(crate) struct ContextMenuHandlerBuilder {
        handler: OsrContextMenuHandler,
    }

    impl ContextMenuHandler {
        // NOTE: no on_before_context_menu override — see the module doc.

        fn run_context_menu(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            params: Option<&mut ContextMenuParams>,
            _model: Option<&mut MenuModel>,
            callback: Option<&mut RunContextMenuCallback>,
        ) -> ::std::os::raw::c_int {
            let context = params
                .map(|params| OsrContextMenuHandler::extract_context(params))
                .unwrap_or_default();

            // Cancel the CEF callback: menu entries dispatch through the
            // tab-backend seam, not through CEF's menu command ids.
            if let Some(callback) = callback {
                callback.cancel();
            }

            send_event(
                &self.handler.sender,
                TabBackendEvent::ContextMenuRequested(context),
            );

            // 1 = the app handles displaying the menu.
            1
        }
    }
}

impl ContextMenuHandlerBuilder {
    pub fn build(handler: OsrContextMenuHandler) -> cef::ContextMenuHandler {
        Self::new(handler)
    }
}
