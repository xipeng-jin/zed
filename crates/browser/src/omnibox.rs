//! The omnibox: the navigation chrome's combined URL-and-search entry line.
//!
//! Committed text resolves through the URL-versus-search heuristic and emits
//! [`OmniboxEvent::Navigate`]. While the user types, browsing history is
//! fuzzy-searched (debounced, on the background executor) and the matches are
//! offered in a dropdown under the editor, ranked by recency, frequency, and
//! URL prefix; arrow keys move the selection and `enter` navigates to it
//! (ticket #11; port source `Glass:crates/browser/src/omnibox.rs`).

use crate::browser_settings::{BrowserSettings, search_engine_label, search_url};
use crate::history::{BrowserHistory, HistoryMatch};
use editor::{Editor, EditorEvent, actions::SelectAll};
use gpui::{
    Anchor, App, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton,
    Pixels, Render, SharedString, Subscription, Task, Window, anchored, canvas, deferred, point,
};
use settings::{BrowserSearchEngine, Settings as _};
use std::time::Duration;
use ui::prelude::*;
use zed_actions::editor::{MoveDown, MoveUp};

/// How long typing settles before the history search runs
/// (`Glass:crates/browser/src/omnibox.rs:160`).
pub(crate) const SEARCH_DEBOUNCE: Duration = Duration::from_millis(100);

/// How many history matches the dropdown offers.
const MAX_HISTORY_SUGGESTIONS: usize = 8;

pub enum OmniboxEvent {
    /// The user committed the omnibox text; the payload is a full URL, already
    /// resolved through the URL-versus-search heuristic.
    Navigate(String),
}

pub(crate) enum OmniboxSuggestion {
    /// The typed text interpreted as an address.
    Url(String),
    /// The typed text as a web search.
    Search(String),
    /// A page from browsing history.
    History { url: String, title: String },
}

impl OmniboxSuggestion {
    /// The URL navigating to this suggestion loads.
    pub fn resolve(&self, engine: &BrowserSearchEngine) -> String {
        match self {
            Self::Url(text) => text_to_url(text, engine),
            Self::Search(query) => search_url(engine, query),
            Self::History { url, .. } => url.clone(),
        }
    }
}

pub struct Omnibox {
    url_editor: Entity<Editor>,
    history: Entity<BrowserHistory>,
    /// URL of the page currently shown, restored into the editor when entry is
    /// cancelled or abandoned.
    current_url: String,
    suggestions: Vec<OmniboxSuggestion>,
    selected_index: usize,
    is_open: bool,
    /// Debounced history search for the text being typed; replacing it
    /// cancels the previous query.
    pending_search: Option<Task<()>>,
    /// Window-relative bounds of the editor box, captured at draw time; the
    /// dropdown is anchored to its bottom edge.
    editor_bounds: Bounds<Pixels>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<OmniboxEvent> for Omnibox {}

impl Omnibox {
    pub fn new(
        history: Entity<BrowserHistory>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let url_editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text("Enter URL or search…", window, cx);
            editor
        });

        let edit_subscription = cx.subscribe(&url_editor, Self::on_editor_event);
        let focus_subscription =
            cx.on_focus(&url_editor.focus_handle(cx), window, Self::on_editor_focus);
        let blur_subscription =
            cx.on_blur(&url_editor.focus_handle(cx), window, Self::on_editor_blur);

        Self {
            url_editor,
            history,
            current_url: String::new(),
            suggestions: Vec::new(),
            selected_index: 0,
            is_open: false,
            pending_search: None,
            editor_bounds: Bounds::default(),
            _subscriptions: vec![edit_subscription, focus_subscription, blur_subscription],
        }
    }

    /// Record the URL of the page currently shown, mirroring it into the
    /// editor unless the user is editing there.
    pub fn set_current_url(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.current_url == url {
            return;
        }
        self.current_url = url.to_string();
        if !self.url_editor.focus_handle(cx).is_focused(window) {
            self.reset_editor_text(window, cx);
        }
    }

    pub fn focus_and_select_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus_handle = self.url_editor.focus_handle(cx);
        window.focus(&focus_handle, cx);
        self.url_editor.update(cx, |editor, cx| {
            editor.select_all(&SelectAll, window, cx);
        });
    }

    /// Begin entry on a fresh tab: the previous page's URL must not linger in
    /// the editor, even though it is normally left alone while focused. The
    /// mirror sync (`set_current_url`) runs on the next render, after focus
    /// has already moved here, so the stale text is cleared explicitly.
    pub fn start_blank_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.current_url.clear();
        self.url_editor.update(cx, |editor, cx| {
            editor.clear(window, cx);
        });
        let focus_handle = self.url_editor.focus_handle(cx);
        window.focus(&focus_handle, cx);
    }

    #[cfg(test)]
    pub fn editor_text(&self, cx: &App) -> String {
        self.url_editor.read(cx).text(cx)
    }

    #[cfg(test)]
    pub fn set_editor_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.url_editor.update(cx, |editor, cx| {
            editor.set_text(text, window, cx);
        });
    }

    #[cfg(test)]
    pub(crate) fn suggestions(&self) -> &[OmniboxSuggestion] {
        &self.suggestions
    }

    #[cfg(test)]
    pub fn is_dropdown_open(&self) -> bool {
        self.is_open
    }

    #[cfg(test)]
    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn reset_editor_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current_url = self.current_url.clone();
        self.url_editor.update(cx, |editor, cx| {
            if editor.text(cx) != current_url {
                editor.set_text(current_url, window, cx);
            }
        });
    }

    fn on_editor_event(
        &mut self,
        _editor: Entity<Editor>,
        event: &EditorEvent,
        cx: &mut Context<Self>,
    ) {
        if matches!(event, EditorEvent::BufferEdited) {
            self.schedule_search(cx);
        }
    }

    fn on_editor_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.url_editor.update(cx, |editor, cx| {
            editor.select_all(&SelectAll, window, cx);
        });
    }

    fn on_editor_blur(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_dropdown(cx);
        self.reset_editor_text(window, cx);
    }

    /// Kick off the debounced history search for the current editor text.
    /// Programmatic edits (URL mirroring, cancel/blur resets) always set the
    /// text to `current_url`, which is filtered out here — so only user typing
    /// opens the dropdown, without a suppression flag.
    fn schedule_search(&mut self, cx: &mut Context<Self>) {
        let query = self.url_editor.read(cx).text(cx);
        if query.trim().is_empty() || query == self.current_url {
            self.close_dropdown(cx);
            return;
        }

        let executor = cx.background_executor().clone();
        self.pending_search = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;

            let Ok((query, current_url, entries)) = this.read_with(cx, |this, cx| {
                (
                    this.url_editor.read(cx).text(cx),
                    this.current_url.clone(),
                    this.history.read(cx).entries().to_vec(),
                )
            }) else {
                return;
            };
            if query.trim().is_empty() || query == current_url {
                this.update(cx, |this, cx| this.close_dropdown(cx)).ok();
                return;
            }

            let matches =
                BrowserHistory::search(entries, query.clone(), MAX_HISTORY_SUGGESTIONS, executor)
                    .await;
            this.update(cx, |this, cx| {
                this.build_suggestions(query, matches);
                this.pending_search = None;
                cx.notify();
            })
            .ok();
        }));
    }

    /// Unlike Glass (which always puts the search row first), the top —
    /// default-selected — suggestion mirrors what plain `enter` has always
    /// done: the typed text as a URL when it looks like one, else as a
    /// search. Opening the dropdown must not change where `enter` goes.
    fn build_suggestions(&mut self, query: String, history_matches: Vec<HistoryMatch>) {
        self.suggestions.clear();
        if looks_like_url(&query) {
            self.suggestions.push(OmniboxSuggestion::Url(query.clone()));
            self.suggestions.push(OmniboxSuggestion::Search(query));
        } else {
            self.suggestions.push(OmniboxSuggestion::Search(query));
        }
        self.suggestions
            .extend(
                history_matches
                    .into_iter()
                    .map(|history_match| OmniboxSuggestion::History {
                        url: history_match.url,
                        title: history_match.title,
                    }),
            );
        self.selected_index = 0;
        self.is_open = true;
    }

    fn close_dropdown(&mut self, cx: &mut Context<Self>) {
        self.suggestions.clear();
        self.selected_index = 0;
        self.is_open = false;
        self.pending_search = None;
        cx.notify();
    }

    fn navigate_to_suggestion(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(suggestion) = self.suggestions.get(index) else {
            return;
        };
        let url = suggestion.resolve(&BrowserSettings::get_global(cx).search_engine);
        self.close_dropdown(cx);
        cx.emit(OmniboxEvent::Navigate(url));
    }

    /// `enter` reaches here as `menu::Confirm`: single-line editors don't bind
    /// `enter` themselves, so the global binding wins and dispatches through
    /// this view's element tree.
    fn confirm(&mut self, _: &menu::Confirm, _window: &mut Window, cx: &mut Context<Self>) {
        if self.is_open && !self.suggestions.is_empty() {
            let index = self.selected_index.min(self.suggestions.len() - 1);
            self.navigate_to_suggestion(index, cx);
            return;
        }

        let text = self.url_editor.read(cx).text(cx);
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let url = text_to_url(text, &BrowserSettings::get_global(cx).search_engine);
        cx.emit(OmniboxEvent::Navigate(url));
    }

    fn cancel(&mut self, _: &menu::Cancel, window: &mut Window, cx: &mut Context<Self>) {
        self.close_dropdown(cx);
        self.reset_editor_text(window, cx);
    }

    /// `up`/`down` reach here because single-line editors propagate
    /// `MoveUp`/`MoveDown` instead of handling them (the same seam the buffer
    /// search bar uses for query history).
    fn move_up(&mut self, _: &MoveUp, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_open || self.suggestions.is_empty() {
            return;
        }
        self.selected_index = if self.selected_index == 0 {
            self.suggestions.len() - 1
        } else {
            self.selected_index - 1
        };
        cx.notify();
    }

    fn move_down(&mut self, _: &MoveDown, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_open || self.suggestions.is_empty() {
            return;
        }
        self.selected_index = (self.selected_index + 1) % self.suggestions.len();
        cx.notify();
    }

    fn render_suggestion_row(&self, index: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let suggestion = self.suggestions.get(index)?;
        let is_selected = index == self.selected_index;
        let (icon, title, subtitle): (IconName, SharedString, Option<SharedString>) =
            match suggestion {
                OmniboxSuggestion::Url(text) => (IconName::ToolWeb, text.clone().into(), None),
                OmniboxSuggestion::Search(query) => (
                    IconName::MagnifyingGlass,
                    format!(
                        "Search {} for \"{}\"",
                        search_engine_label(&BrowserSettings::get_global(cx).search_engine),
                        truncate_label(query, 72)
                    )
                    .into(),
                    None,
                ),
                OmniboxSuggestion::History { url, title } => (
                    IconName::HistoryRerun,
                    if title.is_empty() {
                        url.clone().into()
                    } else {
                        title.clone().into()
                    },
                    Some(url.clone().into()),
                ),
            };

        Some(
            div()
                .id(("omnibox-suggestion", index))
                .w_full()
                .px_2()
                .py_0p5()
                .when(is_selected, |this| {
                    this.bg(cx.theme().colors().ghost_element_selected)
                })
                .when(!is_selected, |this| {
                    this.hover(|style| style.bg(cx.theme().colors().ghost_element_hover))
                })
                .cursor_pointer()
                // Mouse down, not click: pressing in the dropdown blurs the
                // editor, and the blur closes the dropdown before a click's
                // mouse-up could land on the row.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| {
                        this.navigate_to_suggestion(index, cx);
                    }),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(Icon::new(icon).size(IconSize::Small).color(Color::Muted))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Label::new(title).size(LabelSize::Small).truncate()),
                        )
                        .when_some(subtitle, |this, subtitle| {
                            this.child(
                                div().flex_none().max_w_72().child(
                                    Label::new(subtitle)
                                        .size(LabelSize::Small)
                                        .color(Color::Muted)
                                        .truncate(),
                                ),
                            )
                        }),
                )
                .into_any_element(),
        )
    }

    fn render_dropdown(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = (0..self.suggestions.len())
            .filter_map(|index| self.render_suggestion_row(index, cx))
            .collect::<Vec<_>>();

        let position = point(
            self.editor_bounds.origin.x,
            self.editor_bounds.origin.y + self.editor_bounds.size.height,
        );

        deferred(
            anchored()
                .position(position)
                .anchor(Anchor::TopLeft)
                .snap_to_window_with_margin(px(8.))
                .child(
                    v_flex()
                        .id("omnibox-dropdown")
                        .occlude()
                        .w(self.editor_bounds.size.width)
                        .max_h(px(320.))
                        .overflow_y_scroll()
                        .mt_1()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(cx.theme().colors().border)
                        .bg(cx.theme().colors().elevated_surface_background)
                        .shadow_md()
                        .children(rows),
                ),
        )
        .with_priority(1)
    }
}

impl Focusable for Omnibox {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.url_editor.focus_handle(cx)
    }
}

impl Render for Omnibox {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Weak for the same reason as the browser view's bounds tracker: the
        // window retains recent element trees, and a strong handle would keep
        // a closed view alive across unrelated redraws.
        let this = cx.weak_entity();
        let bounds_tracker = canvas(
            move |bounds, _window, cx| {
                this.update(cx, |omnibox, _| omnibox.editor_bounds = bounds)
                    .ok();
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();

        let show_dropdown = self.is_open && !self.suggestions.is_empty();

        h_flex()
            .key_context("Omnibox")
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::move_up))
            .on_action(cx.listener(Self::move_down))
            .relative()
            .flex_1()
            .min_w_0()
            .px_2()
            .py_0p5()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().colors().border)
            .bg(cx.theme().colors().editor_background)
            .child(bounds_tracker)
            .child(self.url_editor.clone())
            .when(show_dropdown, |this| this.child(self.render_dropdown(cx)))
    }
}

fn truncate_label(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    let truncated: String = input.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{truncated}…")
}

fn looks_like_url(input: &str) -> bool {
    if input.starts_with("http://") || input.starts_with("https://") {
        return true;
    }
    if input.contains("://") {
        return true;
    }

    if input.chars().any(char::is_whitespace) {
        return false;
    }

    let Ok(url) = url::Url::parse(&format!("http://{input}")) else {
        return false;
    };

    let Some(host) = url.host_str() else {
        return false;
    };

    host.eq_ignore_ascii_case("localhost")
        || host.contains('.')
        || host.parse::<std::net::IpAddr>().is_ok()
        || (url.port().is_some() && !host.contains('.'))
}

/// Scheme-less hosts that are clearly local development targets (localhost,
/// loopback addresses, bare hosts with a port) rarely serve TLS; everything
/// else defaults to https.
fn should_use_http_by_default(input: &str) -> bool {
    let Ok(url) = url::Url::parse(&format!("http://{input}")) else {
        return false;
    };

    let Some(host) = url.host_str() else {
        return false;
    };

    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }

    if let Ok(address) = host.parse::<std::net::IpAddr>() {
        return address.is_loopback();
    }

    url.port().is_some() && !host.contains('.')
}

/// Resolve omnibox text into a navigable URL: explicit or inferable URLs pass
/// through (with a default scheme added), everything else becomes a web
/// search on `engine`.
pub fn text_to_url(text: &str, engine: &BrowserSearchEngine) -> String {
    if text.starts_with("http://") || text.starts_with("https://") {
        return text.to_string();
    }

    if !looks_like_url(text) {
        return search_url(engine, text);
    }

    if should_use_http_by_default(text) {
        format!("http://{text}")
    } else {
        format!("https://{text}")
    }
}

#[cfg(test)]
mod tests {
    use super::{BrowserSearchEngine, looks_like_url, text_to_url};

    #[test]
    fn localhost_inputs_are_treated_as_urls() {
        let engine = BrowserSearchEngine::Google;
        assert!(looks_like_url("localhost"));
        assert!(looks_like_url("localhost:3000"));
        assert_eq!(text_to_url("localhost", &engine), "http://localhost");
        assert_eq!(
            text_to_url("localhost:3000", &engine),
            "http://localhost:3000"
        );
    }

    #[test]
    fn regular_domains_default_to_https() {
        assert!(looks_like_url("example.com"));
        assert_eq!(
            text_to_url("example.com", &BrowserSearchEngine::Google),
            "https://example.com"
        );
    }

    #[test]
    fn plain_queries_search_on_the_configured_engine() {
        assert!(!looks_like_url("rust ownership"));
        assert_eq!(
            text_to_url("rust ownership", &BrowserSearchEngine::Google),
            "https://www.google.com/search?q=rust+ownership"
        );
        assert_eq!(
            text_to_url("rust ownership", &BrowserSearchEngine::DuckDuckGo),
            "https://duckduckgo.com/?q=rust+ownership"
        );
    }
}
