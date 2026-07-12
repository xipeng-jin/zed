use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settings_macros::{MergeFrom, with_fallible_options};

/// Configuration of the integrated browser.
#[with_fallible_options]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct BrowserSettingsContent {
    /// The search engine used when omnibox input is not a URL.
    ///
    /// Default: google
    pub search_engine: Option<BrowserSearchEngine>,
    /// What a new browser tab opens.
    ///
    /// Default: new_tab_page
    pub new_tab_behavior: Option<BrowserNewTabBehavior>,
    /// The directory downloads are saved into. When not set, the home
    /// Downloads directory is used.
    ///
    /// Default: null
    pub download_directory: Option<PathBuf>,
}

/// The search engine used when omnibox input is not a URL.
///
/// Default: google
#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    strum::EnumDiscriminants,
)]
#[strum_discriminants(derive(strum::VariantArray, strum::VariantNames, strum::FromRepr))]
#[serde(rename_all = "snake_case")]
pub enum BrowserSearchEngine {
    /// Search with Google.
    #[default]
    Google,
    /// Search with DuckDuckGo.
    DuckDuckGo,
    /// Search with Bing.
    Bing,
    /// Search with a custom URL template, in which `{query}` is replaced
    /// with the percent-encoded search terms.
    Custom(String),
}

/// What a new browser tab opens.
///
/// Default: new_tab_page
#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    strum::EnumDiscriminants,
)]
#[strum_discriminants(derive(strum::VariantArray, strum::VariantNames, strum::FromRepr))]
#[serde(rename_all = "snake_case")]
pub enum BrowserNewTabBehavior {
    /// The app-rendered new-tab page.
    #[default]
    NewTabPage,
    /// A fixed URL.
    Url(String),
}
