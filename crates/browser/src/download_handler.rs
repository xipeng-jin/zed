//! CEF download handler, ported from
//! `Glass:crates/browser/src/download_handler.rs`. Downloads are auto-saved
//! into the Downloads directory with collision-safe naming — no save dialog —
//! and every progress change streams through the tab-backend seam
//! (`downloads.rs` holds the shared naming logic and the record model).

use crate::downloads::{self, DownloadUpdate};
use crate::tab_backend::{EventSender, TabBackendEvent, send_event};
use cef::{
    Browser, DownloadHandler, DownloadItem, ImplBeforeDownloadCallback, ImplDownloadHandler,
    ImplDownloadItem, WrapDownloadHandler, rc::Rc as _, wrap_download_handler,
};

#[derive(Clone)]
pub(crate) struct OsrDownloadHandler {
    sender: EventSender,
}

impl OsrDownloadHandler {
    pub fn new(sender: EventSender) -> Self {
        Self { sender }
    }

    fn file_name_for_download(
        suggested_name: Option<&cef::CefString>,
        download_item: Option<&mut DownloadItem>,
    ) -> String {
        let suggested = suggested_name
            .map(ToString::to_string)
            .filter(|name| !name.is_empty())
            .or_else(|| {
                let download_item = download_item.as_ref()?;
                let suggested_userfree = download_item.suggested_file_name();
                let suggested = cef::CefString::from(&suggested_userfree).to_string();
                (!suggested.is_empty()).then_some(suggested)
            })
            .unwrap_or_default();
        let url = download_item
            .map(|download_item| {
                let url_userfree = download_item.url();
                cef::CefString::from(&url_userfree).to_string()
            })
            .unwrap_or_default();
        downloads::file_name_for_download(&suggested, &url)
    }
}

wrap_download_handler! {
    pub(crate) struct DownloadHandlerBuilder {
        handler: OsrDownloadHandler,
    }

    impl DownloadHandler {
        fn can_download(
            &self,
            _browser: Option<&mut Browser>,
            _url: Option<&cef::CefString>,
            _request_method: Option<&cef::CefString>,
        ) -> ::std::os::raw::c_int {
            1
        }

        fn on_before_download(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            suggested_name: Option<&cef::CefString>,
            callback: Option<&mut cef::BeforeDownloadCallback>,
        ) -> ::std::os::raw::c_int {
            let Some(callback) = callback else {
                return 0;
            };

            let directory = downloads::download_directory();
            let file_name =
                OsrDownloadHandler::file_name_for_download(suggested_name, download_item);
            let target_path = downloads::unique_download_path(&directory, &file_name);
            let target_path_text = target_path.to_string_lossy().to_string();
            let cef_target_path = cef::CefString::from(target_path_text.as_str());

            // show_dialog=0: auto-save to the computed path without prompting.
            callback.cont(Some(&cef_target_path), 0);
            1
        }

        fn on_download_updated(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            _callback: Option<&mut cef::DownloadItemCallback>,
        ) {
            let Some(download_item) = download_item else {
                return;
            };

            let full_path_userfree = download_item.full_path();
            let full_path_text = cef::CefString::from(&full_path_userfree).to_string();
            let full_path = (!full_path_text.is_empty()).then_some(full_path_text);

            let url_userfree = download_item.url();
            let original_url_userfree = download_item.original_url();
            let suggested_file_name_userfree = download_item.suggested_file_name();

            let update = DownloadUpdate {
                id: download_item.id(),
                url: cef::CefString::from(&url_userfree).to_string(),
                original_url: cef::CefString::from(&original_url_userfree).to_string(),
                suggested_file_name: cef::CefString::from(&suggested_file_name_userfree)
                    .to_string(),
                full_path,
                current_speed: download_item.current_speed(),
                percent_complete: download_item.percent_complete(),
                total_bytes: download_item.total_bytes(),
                received_bytes: download_item.received_bytes(),
                is_in_progress: download_item.is_in_progress() != 0,
                is_complete: download_item.is_complete() != 0,
                is_canceled: download_item.is_canceled() != 0,
                is_interrupted: download_item.is_interrupted() != 0,
            };

            send_event(&self.handler.sender, TabBackendEvent::DownloadUpdated(update));
        }
    }
}

impl DownloadHandlerBuilder {
    pub fn build(handler: OsrDownloadHandler) -> cef::DownloadHandler {
        Self::new(handler)
    }
}
