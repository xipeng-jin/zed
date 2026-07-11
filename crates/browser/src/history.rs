//! Browsing history: visited pages recorded in memory (capped with LRU
//! eviction), persisted as one JSON blob in the KV store, and fuzzy-searched
//! for omnibox suggestions ranked by recency, frequency, and URL prefix
//! (ported from `Glass:crates/browser/src/history.rs`).
//!
//! One history exists per app — visits from every browser view (except
//! incognito ones, which never record) land in the same shared entity, so it
//! is also the single writer of the persisted blob.

use crate::session;
use db::kvp::KeyValueStore;
use fuzzy::StringMatchCandidate;
use gpui::{App, AppContext as _, BackgroundExecutor, Context, Entity, Global, Subscription, Task};
use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use util::ResultExt as _;

/// Entry cap; inserting beyond it evicts the least recently visited entry
/// (`Glass:crates/browser/src/history.rs:7`).
const MAX_ENTRIES: usize = 2000;

/// How long visit recordings batch before the history is written.
pub(crate) const HISTORY_SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct HistoryEntry {
    pub url: String,
    pub title: String,
    pub visit_count: u32,
    pub last_visited_ms: u64,
}

/// A history entry matched against an omnibox query, with its final ranking
/// score (fuzzy match plus recency, frequency, and prefix bonuses).
#[derive(Clone, Debug)]
pub(crate) struct HistoryMatch {
    pub url: String,
    pub title: String,
    pub score: f64,
}

pub(crate) struct BrowserHistory {
    entries: Vec<HistoryEntry>,
    /// Debounced history write; replacing it pushes the deadline out. Also
    /// the dirty flag: `Some` means recorded visits have not been written.
    pending_save: Option<Task<()>>,
    _quit_flush: Subscription,
}

struct GlobalBrowserHistory(Entity<BrowserHistory>);

impl Global for GlobalBrowserHistory {}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

impl BrowserHistory {
    /// The app-wide shared history, created (restoring the persisted blob) on
    /// first access.
    pub fn global(cx: &mut App) -> Entity<Self> {
        if let Some(global) = cx.try_global::<GlobalBrowserHistory>() {
            return global.0.clone();
        }
        let history = cx.new(Self::new);
        cx.set_global(GlobalBrowserHistory(history.clone()));
        history
    }

    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            entries: session::restore_history(cx).unwrap_or_default(),
            pending_save: None,
            _quit_flush: cx.on_app_quit(Self::flush_on_quit),
        }
    }

    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    pub fn record_visit(&mut self, url: &str, title: &str, cx: &mut Context<Self>) {
        if url.is_empty() || url == "about:blank" {
            return;
        }
        self.record_visit_at(url, title, now_ms());
        self.schedule_save(cx);
    }

    fn record_visit_at(&mut self, url: &str, title: &str, now_ms: u64) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.url == url) {
            entry.visit_count += 1;
            entry.last_visited_ms = now_ms;
            if !title.is_empty() {
                entry.title = title.to_string();
            }
        } else {
            self.entries.push(HistoryEntry {
                url: url.to_string(),
                title: title.to_string(),
                visit_count: 1,
                last_visited_ms: now_ms,
            });

            if self.entries.len() > MAX_ENTRIES {
                if let Some(oldest_index) = self
                    .entries
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, entry)| entry.last_visited_ms)
                    .map(|(index, _)| index)
                {
                    self.entries.swap_remove(oldest_index);
                }
            }
        }
    }

    fn serialize(&self) -> Option<String> {
        serde_json::to_string(&self.entries).log_err()
    }

    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        self.pending_save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HISTORY_SAVE_DEBOUNCE).await;
            let Ok((history, store)) = this.update(cx, |this, cx| {
                this.pending_save = None;
                (this.serialize(), KeyValueStore::global(cx))
            }) else {
                return;
            };
            if let Some(history) = history {
                session::save_history(store, history).await.log_err();
            }
        }));
    }

    fn flush_on_quit(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if self.pending_save.take().is_none() {
            return Task::ready(());
        }
        let Some(history) = self.serialize() else {
            return Task::ready(());
        };
        let store = KeyValueStore::global(cx);
        cx.background_spawn(async move {
            session::save_history(store, history).await.log_err();
        })
    }

    /// Fuzzy-match `query` against `entries` (title and URL), re-ranking the
    /// fuzzy scores with recency, frequency, and URL-prefix bonuses. Static
    /// and by-value so the search can run without holding the entity.
    pub async fn search(
        entries: Vec<HistoryEntry>,
        query: String,
        max_results: usize,
        executor: BackgroundExecutor,
    ) -> Vec<HistoryMatch> {
        if query.is_empty() {
            return Vec::new();
        }

        let now_ms = now_ms();

        let candidates: Vec<StringMatchCandidate> = entries
            .iter()
            .enumerate()
            .map(|(id, entry)| {
                StringMatchCandidate::new(id, &format!("{} {}", entry.title, entry.url))
            })
            .collect();

        let cancel_flag = AtomicBool::new(false);
        let matches = fuzzy::match_strings(
            &candidates,
            &query,
            false,
            true,
            // Over-fetch so the bonus re-ranking below can promote entries the
            // raw fuzzy score alone would have cut.
            max_results * 3,
            &cancel_flag,
            executor,
        )
        .await;

        let query_lower = query.to_lowercase();

        let mut results: Vec<HistoryMatch> = matches
            .into_iter()
            .filter_map(|fuzzy_match| {
                let entry = entries.get(fuzzy_match.candidate_id)?;

                // Recency bonus: up to 0.3, halving every 24 hours of age.
                let age_ms = now_ms.saturating_sub(entry.last_visited_ms);
                let age_hours = age_ms as f64 / 3_600_000.0;
                let recency_bonus = 0.3 * (1.0 / (1.0 + age_hours / 24.0));

                // Frequency bonus: log-scaled visit count.
                let frequency_bonus = 0.2 * (entry.visit_count as f64).ln_1p() / 10.0_f64.ln_1p();

                // Prefix bonus: 0.5 when the URL (with or without its scheme)
                // starts with the query — typing an address should surface the
                // exact site above incidental fuzzy matches.
                let url_lower = entry.url.to_lowercase();
                let prefix_bonus = if url_lower.starts_with(&query_lower)
                    || url_lower
                        .strip_prefix("https://")
                        .is_some_and(|url| url.starts_with(&query_lower))
                    || url_lower
                        .strip_prefix("http://")
                        .is_some_and(|url| url.starts_with(&query_lower))
                {
                    0.5
                } else {
                    0.0
                };

                Some(HistoryMatch {
                    url: entry.url.clone(),
                    title: entry.title.clone(),
                    score: fuzzy_match.score + recency_bonus + frequency_bonus + prefix_bonus,
                })
            })
            .collect();

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(max_results);
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn entry(url: &str, title: &str, visit_count: u32, last_visited_ms: u64) -> HistoryEntry {
        HistoryEntry {
            url: url.to_string(),
            title: title.to_string(),
            visit_count,
            last_visited_ms,
        }
    }

    #[test]
    fn test_recording_dedupes_by_url_and_updates_the_entry() {
        let mut history = BrowserHistory {
            entries: Vec::new(),
            pending_save: None,
            _quit_flush: Subscription::new(|| {}),
        };

        history.record_visit_at("https://example.com", "", 1_000);
        history.record_visit_at("https://example.com", "Example", 2_000);
        history.record_visit_at("https://other.example", "Other", 3_000);

        assert_eq!(history.entries.len(), 2);
        let example = &history.entries[0];
        assert_eq!(example.url, "https://example.com");
        assert_eq!(example.visit_count, 2);
        assert_eq!(
            example.title, "Example",
            "a later titled visit fills in the missing title"
        );
        assert_eq!(example.last_visited_ms, 2_000);

        // An untitled revisit keeps the known title.
        history.record_visit_at("https://example.com", "", 4_000);
        assert_eq!(history.entries[0].title, "Example");
        assert_eq!(history.entries[0].visit_count, 3);
    }

    #[test]
    fn test_the_cap_evicts_the_least_recently_visited_entry() {
        let mut history = BrowserHistory {
            entries: Vec::new(),
            pending_save: None,
            _quit_flush: Subscription::new(|| {}),
        };

        // Visit order and recency deliberately disagree: site0 is the oldest
        // *visit time* even though it is not the first insertion evicted by
        // insertion order.
        history.record_visit_at("https://site1.example", "", 1);
        history.record_visit_at("https://site0.example", "", 0);
        for index in 2..MAX_ENTRIES {
            history.record_visit_at(&format!("https://site{index}.example"), "", index as u64);
        }
        assert_eq!(history.entries.len(), MAX_ENTRIES);

        history.record_visit_at("https://one-too-many.example", "", MAX_ENTRIES as u64);
        assert_eq!(history.entries.len(), MAX_ENTRIES);
        assert!(
            !history
                .entries
                .iter()
                .any(|entry| entry.url == "https://site0.example"),
            "the least recently visited entry is evicted"
        );
        assert!(
            history
                .entries
                .iter()
                .any(|entry| entry.url == "https://one-too-many.example")
        );
    }

    #[gpui::test]
    async fn test_search_ranks_prefix_matches_first(cx: &mut TestAppContext) {
        let now = now_ms();
        let entries = vec![
            entry("https://blog.example/github-tips", "GitHub Tips", 1, now),
            entry("https://github.com", "GitHub", 1, now),
        ];

        let results = BrowserHistory::search(
            entries,
            "github".to_string(),
            8,
            cx.background_executor.clone(),
        )
        .await;

        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0].url, "https://github.com",
            "the URL whose host starts with the query outranks an incidental match"
        );
    }

    #[gpui::test]
    async fn test_search_prefers_recent_and_frequent_entries(cx: &mut TestAppContext) {
        let now = now_ms();
        let week_ms = 7 * 24 * 3_600_000;

        let results = BrowserHistory::search(
            vec![
                entry("https://docs.example/old", "Rust Docs", 1, now - week_ms),
                entry("https://docs.example/new", "Rust Docs", 1, now),
            ],
            "rust docs".to_string(),
            8,
            cx.background_executor.clone(),
        )
        .await;
        assert_eq!(
            results[0].url, "https://docs.example/new",
            "with equal relevance the recent visit wins"
        );

        let results = BrowserHistory::search(
            vec![
                entry("https://forum.example/rare", "Rust Forum", 1, now),
                entry("https://forum.example/daily", "Rust Forum", 200, now),
            ],
            "rust forum".to_string(),
            8,
            cx.background_executor.clone(),
        )
        .await;
        assert_eq!(
            results[0].url, "https://forum.example/daily",
            "with equal relevance and recency the frequent visit wins"
        );
    }

    #[gpui::test]
    async fn test_search_respects_max_results_and_empty_query(cx: &mut TestAppContext) {
        let now = now_ms();
        let entries: Vec<_> = (0..10)
            .map(|index| entry(&format!("https://site{index}.example"), "Site", 1, now))
            .collect();

        let results = BrowserHistory::search(
            entries.clone(),
            "site".to_string(),
            3,
            cx.background_executor.clone(),
        )
        .await;
        assert_eq!(results.len(), 3);

        let results =
            BrowserHistory::search(entries, String::new(), 3, cx.background_executor.clone()).await;
        assert!(results.is_empty(), "an empty query suggests nothing");
    }
}
