//! Bookmarks: pages the user saved, persisted as one JSON blob in the KV
//! store and shown as chips in the bookmark bar (ported from
//! `Glass:crates/browser/src/bookmarks.rs`; Glass's bookmark folders are not
//! ported — their UI was inert even there).
//!
//! One bookmark set exists per app — every browser view reads and mutates the
//! same shared entity, so it is also the single writer of the persisted blob.

use crate::session;
use db::kvp::KeyValueStore;
use gpui::{App, AppContext as _, Context, Entity, Global, Subscription, Task};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use util::ResultExt as _;

/// How long bookmark mutations batch before the blob is written.
pub(crate) const BOOKMARKS_SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Bookmark {
    pub url: String,
    pub title: String,
    #[serde(default)]
    pub favicon_url: Option<String>,
}

pub(crate) struct BrowserBookmarks {
    bookmarks: Vec<Bookmark>,
    /// Debounced bookmark write; replacing it pushes the deadline out. Also
    /// the dirty flag: `Some` means mutations have not been written.
    pending_save: Option<Task<()>>,
    _quit_flush: Subscription,
}

struct GlobalBrowserBookmarks(Entity<BrowserBookmarks>);

impl Global for GlobalBrowserBookmarks {}

impl BrowserBookmarks {
    /// The app-wide shared bookmarks, created (restoring the persisted blob)
    /// on first access.
    pub fn global(cx: &mut App) -> Entity<Self> {
        if let Some(global) = cx.try_global::<GlobalBrowserBookmarks>() {
            return global.0.clone();
        }
        let bookmarks = cx.new(Self::new);
        cx.set_global(GlobalBrowserBookmarks(bookmarks.clone()));
        bookmarks
    }

    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            bookmarks: session::restore_bookmarks(cx).unwrap_or_default(),
            pending_save: None,
            _quit_flush: cx.on_app_quit(Self::flush_on_quit),
        }
    }

    pub fn bookmarks(&self) -> &[Bookmark] {
        &self.bookmarks
    }

    pub fn is_bookmarked(&self, url: &str) -> bool {
        self.bookmarks.iter().any(|bookmark| bookmark.url == url)
    }

    pub fn add(
        &mut self,
        url: String,
        title: String,
        favicon_url: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.insert(url, title, favicon_url) {
            self.schedule_save(cx);
            cx.notify();
        }
    }

    pub fn remove(&mut self, url: &str, cx: &mut Context<Self>) {
        if self.remove_by_url(url) {
            self.schedule_save(cx);
            cx.notify();
        }
    }

    /// Append a bookmark unless the URL is already bookmarked; returns whether
    /// the set changed.
    fn insert(&mut self, url: String, title: String, favicon_url: Option<String>) -> bool {
        if self.is_bookmarked(&url) {
            return false;
        }
        self.bookmarks.push(Bookmark {
            url,
            title,
            favicon_url,
        });
        true
    }

    /// Remove the bookmark for `url`; returns whether the set changed.
    fn remove_by_url(&mut self, url: &str) -> bool {
        let count_before = self.bookmarks.len();
        self.bookmarks.retain(|bookmark| bookmark.url != url);
        self.bookmarks.len() != count_before
    }

    fn serialize(&self) -> Option<String> {
        serde_json::to_string(&self.bookmarks).log_err()
    }

    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        self.pending_save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(BOOKMARKS_SAVE_DEBOUNCE)
                .await;
            let Ok((bookmarks, store)) = this.update(cx, |this, cx| {
                this.pending_save = None;
                (this.serialize(), KeyValueStore::global(cx))
            }) else {
                return;
            };
            if let Some(bookmarks) = bookmarks {
                session::save_bookmarks(store, bookmarks).await.log_err();
            }
        }));
    }

    fn flush_on_quit(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if self.pending_save.take().is_none() {
            return Task::ready(());
        }
        let Some(bookmarks) = self.serialize() else {
            return Task::ready(());
        };
        let store = KeyValueStore::global(cx);
        cx.background_spawn(async move {
            session::save_bookmarks(store, bookmarks).await.log_err();
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bookmarks() -> BrowserBookmarks {
        BrowserBookmarks {
            bookmarks: Vec::new(),
            pending_save: None,
            _quit_flush: Subscription::new(|| {}),
        }
    }

    #[test]
    fn test_adding_dedupes_by_url_and_keeps_the_first_entry() {
        let mut bookmarks = bookmarks();

        assert!(bookmarks.insert(
            "https://example.com".to_string(),
            "Example".to_string(),
            Some("https://example.com/favicon.ico".to_string()),
        ));
        assert!(
            !bookmarks.insert(
                "https://example.com".to_string(),
                "Renamed".to_string(),
                None,
            ),
            "bookmarking an already-bookmarked URL changes nothing"
        );
        assert!(bookmarks.insert("https://other.example".to_string(), String::new(), None));

        assert_eq!(bookmarks.bookmarks().len(), 2);
        assert_eq!(bookmarks.bookmarks()[0].title, "Example");
        assert_eq!(
            bookmarks.bookmarks()[0].favicon_url.as_deref(),
            Some("https://example.com/favicon.ico")
        );
    }

    #[test]
    fn test_removing_by_url() {
        let mut bookmarks = bookmarks();
        bookmarks.insert("https://one.example".to_string(), "One".to_string(), None);
        bookmarks.insert("https://two.example".to_string(), "Two".to_string(), None);

        assert!(bookmarks.remove_by_url("https://one.example"));
        assert!(!bookmarks.is_bookmarked("https://one.example"));
        assert!(bookmarks.is_bookmarked("https://two.example"));

        assert!(
            !bookmarks.remove_by_url("https://one.example"),
            "removing an absent URL changes nothing"
        );
        assert_eq!(bookmarks.bookmarks().len(), 1);
    }

    #[test]
    fn test_bookmarks_round_trip_through_json() {
        let mut bookmarks = bookmarks();
        bookmarks.insert(
            "https://example.com".to_string(),
            "Example".to_string(),
            Some("https://example.com/favicon.ico".to_string()),
        );
        bookmarks.insert(
            "https://other.example".to_string(),
            "Other".to_string(),
            None,
        );

        let json = bookmarks.serialize().unwrap();
        let restored: Vec<Bookmark> = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, bookmarks.bookmarks());
    }
}
