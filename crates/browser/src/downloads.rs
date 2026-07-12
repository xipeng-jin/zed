//! Downloads: the record model and the app-global download store (ported
//! from `Glass:crates/browser/src/download_handler.rs` and the download half
//! of `Glass:crates/browser/src/browser_view.rs`).
//!
//! The engine auto-saves every download into the Downloads directory with
//! collision-safe naming — no save dialog — and streams progress through the
//! tab-backend seam as [`DownloadUpdate`] events. One download list exists
//! per app: every browser view records updates into the same shared entity
//! (tagged incognito when they came from an incognito window), and that
//! entity is the single writer of the persisted blob. Downloads from
//! incognito windows appear in the download center but are excluded from
//! persistence (CONTEXT.md "incognito window").

use crate::session;
use db::kvp::KeyValueStore;
use gpui::{App, AppContext as _, Context, Entity, Global, Subscription, Task};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use util::ResultExt as _;

/// How long download progress updates batch before the blob is written.
pub(crate) const DOWNLOADS_SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

/// One engine report about a download's state. Sent through the tab-backend
/// seam on every progress change; the latest update for an id is the
/// download's current state (and what gets persisted).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DownloadUpdate {
    pub id: u32,
    pub url: String,
    pub original_url: String,
    pub suggested_file_name: String,
    pub full_path: Option<String>,
    pub current_speed: i64,
    pub percent_complete: i32,
    pub total_bytes: i64,
    pub received_bytes: i64,
    pub is_in_progress: bool,
    pub is_complete: bool,
    pub is_canceled: bool,
    pub is_interrupted: bool,
}

/// A download in the download center: the latest engine update plus how the
/// record entered the list.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DownloadRecord {
    pub update: DownloadUpdate,
    /// From an incognito window: shown, never persisted.
    pub is_incognito: bool,
    /// Restored from the persisted blob rather than reported this session.
    /// Engine download ids restart every session, so updates never match
    /// restored records — a new download reusing an old id must not
    /// overwrite last session's history.
    pub is_restored: bool,
}

impl DownloadRecord {
    /// Name shown in the download center: the suggested file name, else the
    /// saved file's name, else a generic label.
    pub fn display_name(&self) -> String {
        if !self.update.suggested_file_name.is_empty() {
            return self.update.suggested_file_name.clone();
        }
        self.update
            .full_path
            .as_deref()
            .and_then(|path| Path::new(path).file_name())
            .and_then(|name| name.to_str())
            .map(ToString::to_string)
            .unwrap_or_else(|| String::from("download"))
    }

    /// One-line status: completion, cancellation, interruption, or live
    /// progress with received/total sizes.
    pub fn status_text(&self) -> String {
        if self.update.is_complete {
            return String::from("Complete");
        }
        if self.update.is_canceled {
            return String::from("Canceled");
        }
        if self.update.is_interrupted {
            return String::from("Interrupted");
        }
        if self.update.is_in_progress {
            let received = format_size(self.update.received_bytes);
            let total = if self.update.total_bytes > 0 {
                format_size(self.update.total_bytes)
            } else {
                String::from("--")
            };
            let percent = self.update.percent_complete.max(0);
            return format!("{percent}% ({received}/{total})");
        }
        String::from("Queued")
    }
}

fn format_size(bytes: i64) -> String {
    let safe_bytes = bytes.max(0) as f64;
    if safe_bytes < 1024.0 {
        return format!("{} B", safe_bytes as i64);
    }
    if safe_bytes < 1024.0 * 1024.0 {
        return format!("{:.1} KB", safe_bytes / 1024.0);
    }
    if safe_bytes < 1024.0 * 1024.0 * 1024.0 {
        return format!("{:.1} MB", safe_bytes / (1024.0 * 1024.0));
    }
    format!("{:.1} GB", safe_bytes / (1024.0 * 1024.0 * 1024.0))
}

/// The settings-configured download directory, mirrored into a process
/// global because the engine's download callback consults it outside any
/// GPUI context (`crate::download_handler`). Kept in sync by
/// `crate::browser_settings::init`.
static DOWNLOAD_DIRECTORY_OVERRIDE: parking_lot::Mutex<Option<PathBuf>> =
    parking_lot::Mutex::new(None);

pub(crate) fn set_download_directory_override(directory: Option<PathBuf>) {
    *DOWNLOAD_DIRECTORY_OVERRIDE.lock() = directory;
}

/// Serializes tests that touch the process-wide override, which would
/// otherwise race across test threads.
#[cfg(test)]
pub(crate) static OVERRIDE_TEST_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// Where downloads are saved: the settings-configured directory when one is
/// set and creatable, else `~/Downloads`, else a directory inside Zed's data
/// dir when the home Downloads directory cannot be created.
pub(crate) fn download_directory() -> PathBuf {
    if let Some(configured) = DOWNLOAD_DIRECTORY_OVERRIDE.lock().clone() {
        let configured = expand_home(&configured);
        match std::fs::create_dir_all(&configured) {
            Ok(()) => return configured,
            Err(error) => log::warn!(
                "[browser] failed to create configured download directory {}: {}",
                configured.display(),
                error
            ),
        }
    }
    let preferred = paths::home_dir().join("Downloads");
    if std::fs::create_dir_all(&preferred).is_ok() {
        return preferred;
    }
    let fallback = paths::data_dir().join("browser_downloads");
    if let Err(error) = std::fs::create_dir_all(&fallback) {
        log::warn!(
            "[browser] failed to create fallback download directory {}: {}",
            fallback.display(),
            error
        );
    }
    fallback
}

fn expand_home(path: &Path) -> PathBuf {
    match path.strip_prefix("~") {
        Ok(stripped) => paths::home_dir().join(stripped),
        Err(_) => path.to_owned(),
    }
}

/// File name for a download: the engine's suggestion if any, else the last
/// path segment of the URL, else a generic name.
pub(crate) fn file_name_for_download(suggested_name: &str, url: &str) -> String {
    if !suggested_name.is_empty() {
        return suggested_name.to_string();
    }
    if let Ok(parsed) = url::Url::parse(url)
        && let Some(segment) = parsed
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .filter(|segment| !segment.is_empty())
    {
        return segment.to_string();
    }
    String::from("download")
}

/// A path in `directory` that does not collide with an existing file:
/// `name.ext`, else `name (1).ext`, `name (2).ext`, … Any directory
/// components in `file_name` are dropped so a hostile suggested name cannot
/// escape the download directory.
pub(crate) fn unique_download_path(directory: &Path, file_name: &str) -> PathBuf {
    let file_name = Path::new(file_name)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("download");

    let original_path = directory.join(file_name);
    if !original_path.exists() {
        return original_path;
    }

    let file_path = Path::new(file_name);
    let stem = file_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("download");
    let extension = file_path
        .extension()
        .and_then(|extension| extension.to_str());

    let mut attempt = 1u32;
    loop {
        let candidate_file_name = if let Some(extension) = extension {
            format!("{stem} ({attempt}).{extension}")
        } else {
            format!("{stem} ({attempt})")
        };
        let candidate_path = directory.join(candidate_file_name);
        if !candidate_path.exists() {
            return candidate_path;
        }
        attempt += 1;
    }
}

pub(crate) struct BrowserDownloads {
    /// All download records, newest first.
    downloads: Vec<DownloadRecord>,
    /// Debounced blob write; replacing it pushes the deadline out. Also the
    /// dirty flag: `Some` means mutations have not been written.
    pending_save: Option<Task<()>>,
    _quit_flush: Subscription,
}

struct GlobalBrowserDownloads(Entity<BrowserDownloads>);

impl Global for GlobalBrowserDownloads {}

impl BrowserDownloads {
    /// The app-wide shared download list, created (restoring the persisted
    /// blob) on first access.
    pub fn global(cx: &mut App) -> Entity<Self> {
        if let Some(global) = cx.try_global::<GlobalBrowserDownloads>() {
            return global.0.clone();
        }
        let downloads = cx.new(Self::new);
        cx.set_global(GlobalBrowserDownloads(downloads.clone()));
        downloads
    }

    pub fn new(cx: &mut Context<Self>) -> Self {
        let downloads = session::restore_downloads(cx)
            .unwrap_or_default()
            .into_iter()
            .map(|update| DownloadRecord {
                update,
                is_incognito: false,
                is_restored: true,
            })
            .collect();
        Self {
            downloads,
            pending_save: None,
            _quit_flush: cx.on_app_quit(Self::flush_on_quit),
        }
    }

    pub fn downloads(&self) -> &[DownloadRecord] {
        &self.downloads
    }

    /// Fold an engine update into the list: refresh the record it belongs to,
    /// or prepend a record for a download seen for the first time. Incognito
    /// updates never touch the persisted blob (CONTEXT.md "incognito
    /// window").
    pub fn record_update(
        &mut self,
        update: DownloadUpdate,
        is_incognito: bool,
        cx: &mut Context<Self>,
    ) {
        self.apply_update(update, is_incognito);
        if !is_incognito {
            self.schedule_save(cx);
        }
        cx.notify();
    }

    fn apply_update(&mut self, update: DownloadUpdate, is_incognito: bool) {
        if let Some(existing) = self
            .downloads
            .iter_mut()
            .find(|record| !record.is_restored && record.update.id == update.id)
        {
            existing.update = update;
        } else {
            self.downloads.insert(
                0,
                DownloadRecord {
                    update,
                    is_incognito,
                    is_restored: false,
                },
            );
        }
    }

    fn serialize(&self) -> Option<String> {
        let updates = self
            .downloads
            .iter()
            .filter(|record| !record.is_incognito)
            .map(|record| &record.update)
            .collect::<Vec<_>>();
        serde_json::to_string(&updates).log_err()
    }

    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        self.pending_save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(DOWNLOADS_SAVE_DEBOUNCE)
                .await;
            let Ok((downloads, store)) = this.update(cx, |this, cx| {
                this.pending_save = None;
                (this.serialize(), KeyValueStore::global(cx))
            }) else {
                return;
            };
            if let Some(downloads) = downloads {
                session::save_downloads(store, downloads).await.log_err();
            }
        }));
    }

    fn flush_on_quit(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if self.pending_save.take().is_none() {
            return Task::ready(());
        }
        let Some(downloads) = self.serialize() else {
            return Task::ready(());
        };
        let store = KeyValueStore::global(cx);
        cx.background_spawn(async move {
            session::save_downloads(store, downloads).await.log_err();
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(id: u32) -> DownloadUpdate {
        DownloadUpdate {
            id,
            url: format!("https://example.com/files/file-{id}.zip"),
            original_url: format!("https://example.com/files/file-{id}.zip"),
            suggested_file_name: format!("file-{id}.zip"),
            full_path: None,
            current_speed: 0,
            percent_complete: 0,
            total_bytes: 0,
            received_bytes: 0,
            is_in_progress: true,
            is_complete: false,
            is_canceled: false,
            is_interrupted: false,
        }
    }

    fn store() -> BrowserDownloads {
        BrowserDownloads {
            downloads: Vec::new(),
            pending_save: None,
            _quit_flush: Subscription::new(|| {}),
        }
    }

    #[test]
    fn test_download_directory_prefers_the_configured_override() {
        let _guard = OVERRIDE_TEST_LOCK.lock();
        let temp = tempfile::tempdir().unwrap();
        let configured = temp.path().join("browser-downloads");
        set_download_directory_override(Some(configured.clone()));
        assert_eq!(download_directory(), configured);
        assert!(configured.is_dir(), "the configured directory is created");
        set_download_directory_override(None);
    }

    #[test]
    fn test_expand_home_maps_tilde_to_the_home_directory() {
        assert_eq!(
            expand_home(Path::new("~/browser-downloads")),
            paths::home_dir().join("browser-downloads")
        );
        assert_eq!(
            expand_home(Path::new("/absolute/downloads")),
            PathBuf::from("/absolute/downloads")
        );
    }

    #[test]
    fn test_file_name_prefers_suggestion_then_url_segment() {
        assert_eq!(
            file_name_for_download("report.pdf", "https://example.com/x/y.bin"),
            "report.pdf"
        );
        assert_eq!(
            file_name_for_download("", "https://example.com/x/archive.tar.gz"),
            "archive.tar.gz"
        );
        assert_eq!(
            file_name_for_download("", "https://example.com/x/archive.tar.gz?token=abc"),
            "archive.tar.gz"
        );
        assert_eq!(file_name_for_download("", "https://example.com/"), "download");
        assert_eq!(file_name_for_download("", "not a url"), "download");
    }

    #[test]
    fn test_unique_download_path_numbers_collisions() {
        let directory = tempfile::tempdir().unwrap();
        let directory = directory.path();

        assert_eq!(
            unique_download_path(directory, "data.zip"),
            directory.join("data.zip")
        );

        std::fs::write(directory.join("data.zip"), b"first").unwrap();
        assert_eq!(
            unique_download_path(directory, "data.zip"),
            directory.join("data (1).zip")
        );

        std::fs::write(directory.join("data (1).zip"), b"second").unwrap();
        assert_eq!(
            unique_download_path(directory, "data.zip"),
            directory.join("data (2).zip")
        );

        std::fs::write(directory.join("notes"), b"no extension").unwrap();
        assert_eq!(
            unique_download_path(directory, "notes"),
            directory.join("notes (1)")
        );
    }

    #[test]
    fn test_unique_download_path_strips_directory_components() {
        let directory = tempfile::tempdir().unwrap();
        let directory = directory.path();

        assert_eq!(
            unique_download_path(directory, "../../escape.sh"),
            directory.join("escape.sh")
        );
        assert_eq!(
            unique_download_path(directory, ""),
            directory.join("download")
        );
    }

    #[test]
    fn test_updates_fold_into_records_by_id_newest_first() {
        let mut store = store();

        store.apply_update(update(1), false);
        store.apply_update(update(2), false);
        let mut progress = update(1);
        progress.percent_complete = 50;
        progress.received_bytes = 512;
        store.apply_update(progress, false);

        let ids: Vec<u32> = store
            .downloads()
            .iter()
            .map(|record| record.update.id)
            .collect();
        assert_eq!(ids, vec![2, 1], "newest download first; updates fold in");
        assert_eq!(store.downloads()[1].update.percent_complete, 50);
    }

    #[test]
    fn test_updates_never_match_restored_records() {
        let mut store = store();
        store.downloads.push(DownloadRecord {
            update: update(1),
            is_incognito: false,
            is_restored: true,
        });

        store.apply_update(update(1), false);

        assert_eq!(
            store.downloads().len(),
            2,
            "a new download reusing a restored record's id gets its own record"
        );
        assert!(!store.downloads()[0].is_restored);
        assert!(store.downloads()[1].is_restored);
    }

    #[test]
    fn test_incognito_records_are_shown_but_not_serialized() {
        let mut store = store();
        store.apply_update(update(1), false);
        store.apply_update(update(2), true);

        assert_eq!(store.downloads().len(), 2);
        let json = store.serialize().unwrap();
        let persisted: Vec<DownloadUpdate> = serde_json::from_str(&json).unwrap();
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].id, 1);
    }

    #[test]
    fn test_records_round_trip_through_json() {
        let mut store = store();
        let mut complete = update(7);
        complete.is_in_progress = false;
        complete.is_complete = true;
        complete.full_path = Some("/home/user/Downloads/file-7.zip".to_string());
        store.apply_update(complete.clone(), false);

        let json = store.serialize().unwrap();
        let restored: Vec<DownloadUpdate> = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, vec![complete]);
    }

    #[test]
    fn test_display_name_falls_back_to_saved_file_name() {
        let record = |suggested: &str, path: Option<&str>| DownloadRecord {
            update: DownloadUpdate {
                suggested_file_name: suggested.to_string(),
                full_path: path.map(ToString::to_string),
                ..update(1)
            },
            is_incognito: false,
            is_restored: false,
        };

        assert_eq!(record("cat.png", None).display_name(), "cat.png");
        assert_eq!(
            record("", Some("/downloads/dog (1).png")).display_name(),
            "dog (1).png"
        );
        assert_eq!(record("", None).display_name(), "download");
    }

    #[test]
    fn test_status_text_reflects_download_state() {
        let record = |mutate: fn(&mut DownloadUpdate)| {
            let mut update = update(1);
            mutate(&mut update);
            DownloadRecord {
                update,
                is_incognito: false,
                is_restored: false,
            }
        };

        assert_eq!(
            record(|update| {
                update.is_in_progress = false;
                update.is_complete = true;
            })
            .status_text(),
            "Complete"
        );
        assert_eq!(
            record(|update| {
                update.is_in_progress = false;
                update.is_canceled = true;
            })
            .status_text(),
            "Canceled"
        );
        assert_eq!(
            record(|update| {
                update.is_in_progress = false;
                update.is_interrupted = true;
            })
            .status_text(),
            "Interrupted"
        );
        assert_eq!(
            record(|update| {
                update.percent_complete = 40;
                update.received_bytes = 2 * 1024 * 1024;
                update.total_bytes = 5 * 1024 * 1024;
            })
            .status_text(),
            "40% (2.0 MB/5.0 MB)"
        );
        assert_eq!(
            record(|update| {
                update.percent_complete = -1;
                update.received_bytes = 512;
                update.total_bytes = 0;
            })
            .status_text(),
            "0% (512 B/--)",
            "unknown totals show as -- and negative percents clamp to 0"
        );
        assert_eq!(
            record(|update| update.is_in_progress = false).status_text(),
            "Queued"
        );
    }
}
