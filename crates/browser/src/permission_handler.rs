//! CEF permission handler, ported from
//! `Glass:crates/browser/src/permission_handler.rs`.
//!
//! Grants media-access requests (camera/microphone/screen share) and accepts
//! protected-media-identifier prompts required for Widevine DRM playback on
//! streaming platforms. All other permission prompts fall through to the
//! engine default (deny). Deliberate for a daily-driver browser without
//! permission UI (plan §3.6).

use cef::{
    Browser, Frame, ImplMediaAccessCallback, ImplPermissionHandler, ImplPermissionPromptCallback,
    MediaAccessCallback, PermissionHandler, PermissionPromptCallback, PermissionRequestResult,
    WrapPermissionHandler, rc::Rc as _, wrap_permission_handler,
};

/// `CEF_PERMISSION_TYPE_PROTECTED_MEDIA_IDENTIFIER` in the permission-type
/// bitmask (`Glass:crates/browser/src/permission_handler.rs:14`).
const PROTECTED_MEDIA_IDENTIFIER: u32 = 1 << 18;

#[derive(Clone)]
pub(crate) struct OsrPermissionHandler;

impl OsrPermissionHandler {
    pub fn new() -> Self {
        Self
    }
}

wrap_permission_handler! {
    pub(crate) struct PermissionHandlerBuilder {
        handler: OsrPermissionHandler,
    }

    impl PermissionHandler {
        fn on_request_media_access_permission(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            requesting_origin: Option<&cef::CefString>,
            requested_permissions: u32,
            callback: Option<&mut MediaAccessCallback>,
        ) -> ::std::os::raw::c_int {
            let origin = requesting_origin
                .map(|origin| origin.to_string())
                .unwrap_or_default();
            log::info!(
                "[browser::permission] granting media access for '{origin}' \
                 (permissions=0x{requested_permissions:x})"
            );
            if let Some(callback) = callback {
                callback.cont(requested_permissions);
            }
            1
        }

        fn on_show_permission_prompt(
            &self,
            _browser: Option<&mut Browser>,
            prompt_id: u64,
            requesting_origin: Option<&cef::CefString>,
            requested_permissions: u32,
            callback: Option<&mut PermissionPromptCallback>,
        ) -> ::std::os::raw::c_int {
            let origin = requesting_origin
                .map(|origin| origin.to_string())
                .unwrap_or_default();

            if requested_permissions & PROTECTED_MEDIA_IDENTIFIER != 0 {
                log::info!(
                    "[browser::permission] granting protected media identifier for '{origin}' \
                     (prompt_id={prompt_id})"
                );
                if let Some(callback) = callback {
                    callback.cont(PermissionRequestResult::ACCEPT);
                }
                return 1;
            }

            log::info!(
                "[browser::permission] leaving permission prompt to engine default for \
                 '{origin}' (permissions=0x{requested_permissions:x}, prompt_id={prompt_id})"
            );
            0
        }

        fn on_dismiss_permission_prompt(
            &self,
            _browser: Option<&mut Browser>,
            prompt_id: u64,
            result: PermissionRequestResult,
        ) {
            log::debug!(
                "[browser::permission] permission prompt dismissed \
                 (prompt_id={prompt_id}, result={result:?})"
            );
        }
    }
}

impl PermissionHandlerBuilder {
    pub fn build(handler: OsrPermissionHandler) -> cef::PermissionHandler {
        Self::new(handler)
    }
}
