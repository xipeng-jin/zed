//! Browsing-context persistence: the browser session (tabs, pinned state,
//! active index) and the browsing history, each as a JSON blob in Zed's
//! key-value store, under the keys established by Glass
//! (`Glass:crates/browser/src/session.rs:7-11`). Web session state (cookies,
//! logins) is not persisted here — it lives in the engine's on-disk profile
//! (`cef_instance.rs`, `browser_cache` + `persist_session_cookies`).

use crate::bookmarks::Bookmark;
use crate::downloads::DownloadUpdate;
use crate::history::HistoryEntry;
use db::kvp::KeyValueStore;
use gpui::App;
use serde::{Deserialize, Serialize};
use util::ResultExt as _;

const BROWSER_TABS_KEY: &str = "browser_tabs";
const BROWSER_HISTORY_KEY: &str = "browser_history";
const BROWSER_BOOKMARKS_KEY: &str = "browser_bookmarks";
const BROWSER_DOWNLOADS_KEY: &str = "browser_downloads";

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct SerializedBrowserTabs {
    pub tabs: Vec<SerializedTab>,
    pub active_index: usize,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct SerializedTab {
    pub url: String,
    pub title: String,
    #[serde(default)]
    pub is_new_tab_page: bool,
    #[serde(default)]
    pub is_pinned: bool,
    #[serde(default)]
    pub favicon_url: Option<String>,
}

pub(crate) fn restore(cx: &App) -> Option<SerializedBrowserTabs> {
    let json = KeyValueStore::global(cx)
        .read_kvp(BROWSER_TABS_KEY)
        .log_err()??;
    serde_json::from_str(&json).log_err()
}

pub(crate) async fn save(store: KeyValueStore, session: String) -> anyhow::Result<()> {
    store.write_kvp(BROWSER_TABS_KEY.to_string(), session).await
}

pub(crate) fn restore_history(cx: &App) -> Option<Vec<HistoryEntry>> {
    let json = KeyValueStore::global(cx)
        .read_kvp(BROWSER_HISTORY_KEY)
        .log_err()??;
    serde_json::from_str(&json).log_err()
}

pub(crate) async fn save_history(store: KeyValueStore, history: String) -> anyhow::Result<()> {
    store
        .write_kvp(BROWSER_HISTORY_KEY.to_string(), history)
        .await
}

pub(crate) fn restore_bookmarks(cx: &App) -> Option<Vec<Bookmark>> {
    let json = KeyValueStore::global(cx)
        .read_kvp(BROWSER_BOOKMARKS_KEY)
        .log_err()??;
    serde_json::from_str(&json).log_err()
}

pub(crate) async fn save_bookmarks(store: KeyValueStore, bookmarks: String) -> anyhow::Result<()> {
    store
        .write_kvp(BROWSER_BOOKMARKS_KEY.to_string(), bookmarks)
        .await
}

pub(crate) fn restore_downloads(cx: &App) -> Option<Vec<DownloadUpdate>> {
    let json = KeyValueStore::global(cx)
        .read_kvp(BROWSER_DOWNLOADS_KEY)
        .log_err()??;
    serde_json::from_str(&json).log_err()
}

pub(crate) async fn save_downloads(store: KeyValueStore, downloads: String) -> anyhow::Result<()> {
    store
        .write_kvp(BROWSER_DOWNLOADS_KEY.to_string(), downloads)
        .await
}
