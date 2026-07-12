//! The `browser` settings section (plan §7 M2 step 8): default search
//! engine, new-tab behavior, and download directory. The content schema
//! lives in `settings_content::browser`; defaults in
//! `assets/settings/default.json`.

use gpui::App;
use settings::{BrowserNewTabBehavior, BrowserSearchEngine, RegisterSetting, Settings};
use std::path::PathBuf;

/// Settings for the integrated browser.
#[derive(Clone, Debug, RegisterSetting)]
pub struct BrowserSettings {
    /// The search engine used when omnibox input is not a URL.
    pub search_engine: BrowserSearchEngine,
    /// What a new browser tab opens.
    pub new_tab_behavior: BrowserNewTabBehavior,
    /// The directory downloads are saved into; `None` means the home
    /// Downloads directory.
    pub download_directory: Option<PathBuf>,
}

impl Settings for BrowserSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let browser = content.browser.clone().unwrap();
        Self {
            search_engine: browser.search_engine.unwrap(),
            new_tab_behavior: browser.new_tab_behavior.unwrap(),
            download_directory: browser.download_directory,
        }
    }
}

/// Keep the download-directory override in sync with settings. The override
/// is a process global rather than a read at use time because the engine's
/// download callback runs outside any GPUI context
/// (`crate::download_handler`).
pub(crate) fn init(cx: &mut App) {
    sync_download_directory(cx);
    cx.observe_global::<settings::SettingsStore>(sync_download_directory)
        .detach();
}

fn sync_download_directory(cx: &mut App) {
    crate::downloads::set_download_directory_override(
        BrowserSettings::get_global(cx).download_directory.clone(),
    );
}

/// The engine name shown in the omnibox's search suggestion row. A custom
/// engine reads as its host, so the row still says where the search goes.
pub(crate) fn search_engine_label(engine: &BrowserSearchEngine) -> String {
    match engine {
        BrowserSearchEngine::Google => "Google".to_string(),
        BrowserSearchEngine::DuckDuckGo => "DuckDuckGo".to_string(),
        BrowserSearchEngine::Bing => "Bing".to_string(),
        BrowserSearchEngine::Custom(template) => url::Url::parse(template)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .unwrap_or_else(|| "the web".to_string()),
    }
}

/// The search URL loading `query`'s results on `engine`.
pub(crate) fn search_url(engine: &BrowserSearchEngine, query: &str) -> String {
    let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
    match engine {
        BrowserSearchEngine::Google => format!("https://www.google.com/search?q={encoded}"),
        BrowserSearchEngine::DuckDuckGo => format!("https://duckduckgo.com/?q={encoded}"),
        BrowserSearchEngine::Bing => format!("https://www.bing.com/search?q={encoded}"),
        BrowserSearchEngine::Custom(template) => {
            if template.contains("{query}") {
                template.replace("{query}", &encoded)
            } else {
                // A template with no placeholder still has to produce a
                // usable search; treat it as a URL prefix.
                format!("{template}{encoded}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn test_download_directory_setting_syncs_the_engine_override(cx: &mut gpui::TestAppContext) {
        let _guard = crate::downloads::OVERRIDE_TEST_LOCK.lock();
        let temp = tempfile::tempdir().unwrap();
        let configured = temp.path().join("configured-downloads");
        cx.update(|cx| {
            let store = settings::SettingsStore::test(cx);
            cx.set_global(store);
            init(cx);
        });

        cx.update_global(|store: &mut settings::SettingsStore, cx| {
            store.update_user_settings(cx, |settings| {
                settings.browser.get_or_insert_default().download_directory =
                    Some(configured.clone());
            });
        });

        assert_eq!(
            crate::downloads::download_directory(),
            configured,
            "the settings change reaches the engine-side download directory"
        );
        crate::downloads::set_download_directory_override(None);
    }

    #[test]
    fn named_engines_build_their_search_urls() {
        assert_eq!(
            search_url(&BrowserSearchEngine::Google, "rust ownership"),
            "https://www.google.com/search?q=rust+ownership"
        );
        assert_eq!(
            search_url(&BrowserSearchEngine::DuckDuckGo, "rust ownership"),
            "https://duckduckgo.com/?q=rust+ownership"
        );
        assert_eq!(
            search_url(&BrowserSearchEngine::Bing, "rust ownership"),
            "https://www.bing.com/search?q=rust+ownership"
        );
    }

    #[test]
    fn custom_engine_replaces_the_query_placeholder() {
        let engine = BrowserSearchEngine::Custom("https://kagi.com/search?q={query}".to_string());
        assert_eq!(
            search_url(&engine, "a&b"),
            "https://kagi.com/search?q=a%26b"
        );
    }

    #[test]
    fn custom_engine_without_placeholder_appends_the_query() {
        let engine = BrowserSearchEngine::Custom("https://example.com/find?q=".to_string());
        assert_eq!(search_url(&engine, "zed"), "https://example.com/find?q=zed");
    }
}
