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
use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use util::ResultExt as _;

const BROWSER_TABS_KEY: &str = "browser_tabs";
pub(crate) const BROWSER_HISTORY_KEY: &str = "browser_history";
pub(crate) const BROWSER_BOOKMARKS_KEY: &str = "browser_bookmarks";
pub(crate) const BROWSER_DOWNLOADS_KEY: &str = "browser_downloads";

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
    restore_blob(BROWSER_TABS_KEY, cx)
}

pub(crate) async fn save(store: KeyValueStore, session: String) -> anyhow::Result<()> {
    save_blob(store, BROWSER_TABS_KEY, session).await
}

pub(crate) fn restore_history(cx: &App) -> Option<Vec<HistoryEntry>> {
    restore_blob(BROWSER_HISTORY_KEY, cx)
}

pub(crate) fn restore_bookmarks(cx: &App) -> Option<Vec<Bookmark>> {
    restore_blob(BROWSER_BOOKMARKS_KEY, cx)
}

pub(crate) fn restore_downloads(cx: &App) -> Option<Vec<DownloadUpdate>> {
    restore_blob(BROWSER_DOWNLOADS_KEY, cx)
}

fn restore_blob<T: DeserializeOwned>(key: &str, cx: &App) -> Option<T> {
    let json = KeyValueStore::global(cx).read_kvp(key).log_err()??;
    serde_json::from_str(&json).log_err()
}

pub(crate) async fn save_blob(store: KeyValueStore, key: &str, blob: String) -> anyhow::Result<()> {
    store.write_kvp(key.to_string(), blob).await
}

/// Holds the app-global instance of one shared browser entity (history,
/// bookmarks, downloads).
struct GlobalEntity<T>(Entity<T>);

impl<T: 'static> Global for GlobalEntity<T> {}

/// The app-wide shared instance of `T`, created on first access.
pub(crate) fn global_entity<T: 'static>(
    cx: &mut App,
    new: impl FnOnce(&mut Context<T>) -> T,
) -> Entity<T> {
    if let Some(global) = cx.try_global::<GlobalEntity<T>>() {
        return global.0.clone();
    }
    let entity = cx.new(new);
    cx.set_global(GlobalEntity(entity.clone()));
    entity
}

/// An app-global entity persisted as one JSON blob under [`Self::KEY`]:
/// mutations batch for [`Self::SAVE_DEBOUNCE`] before the blob is written, and
/// quit flushes a pending write immediately. Implemented by the shared
/// history, bookmarks, and downloads entities so they share one copy of the
/// debounce-and-flush machinery.
pub(crate) trait PersistedBlob: Sized + 'static {
    const KEY: &'static str;
    const SAVE_DEBOUNCE: Duration;

    /// The blob as saved; `None` when serialization fails (already logged).
    fn serialize_blob(&self) -> Option<String>;

    /// The debounced write task slot. `Some` doubles as the dirty flag:
    /// recorded mutations have not been written yet.
    fn pending_save_mut(&mut self) -> &mut Option<Task<()>>;

    /// Debounced blob write: mutations within the window batch into one KV
    /// write. Replacing the pending task pushes the deadline out.
    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        *self.pending_save_mut() = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Self::SAVE_DEBOUNCE).await;
            let Ok((blob, store)) = this.update(cx, |this, cx| {
                *this.pending_save_mut() = None;
                (this.serialize_blob(), KeyValueStore::global(cx))
            }) else {
                return;
            };
            if let Some(blob) = blob {
                save_blob(store, Self::KEY, blob).await.log_err();
            }
        }));
    }

    /// Write a pending (dirty) blob immediately, for `cx.on_app_quit`.
    fn flush_on_quit(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if self.pending_save_mut().take().is_none() {
            return Task::ready(());
        }
        let Some(blob) = self.serialize_blob() else {
            return Task::ready(());
        };
        let store = KeyValueStore::global(cx);
        cx.background_spawn(async move {
            save_blob(store, Self::KEY, blob).await.log_err();
        })
    }
}
