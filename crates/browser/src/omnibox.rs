//! The omnibox: the navigation chrome's combined URL-and-search entry line.
//!
//! M1 scope (ticket #7): a single-line editor that resolves committed text
//! through the URL-versus-search heuristic and emits
//! [`OmniboxEvent::Navigate`]. History-backed suggestions and the dropdown
//! arrive with ticket #11 (`Glass:crates/browser/src/omnibox.rs` is the port
//! source).

use editor::{Editor, actions::SelectAll};
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, Render, Subscription, Window,
};
use ui::prelude::*;

pub enum OmniboxEvent {
    /// The user committed the omnibox text; the payload is a full URL, already
    /// resolved through the URL-versus-search heuristic.
    Navigate(String),
}

pub struct Omnibox {
    url_editor: Entity<Editor>,
    /// URL of the page currently shown, restored into the editor when entry is
    /// cancelled or abandoned.
    current_url: String,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<OmniboxEvent> for Omnibox {}

impl Omnibox {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let url_editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text("Enter URL or search…", window, cx);
            editor
        });

        let focus_subscription =
            cx.on_focus(&url_editor.focus_handle(cx), window, Self::on_editor_focus);
        let blur_subscription =
            cx.on_blur(&url_editor.focus_handle(cx), window, Self::on_editor_blur);

        Self {
            url_editor,
            current_url: String::new(),
            _subscriptions: vec![focus_subscription, blur_subscription],
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

    fn reset_editor_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current_url = self.current_url.clone();
        self.url_editor.update(cx, |editor, cx| {
            if editor.text(cx) != current_url {
                editor.set_text(current_url, window, cx);
            }
        });
    }

    fn on_editor_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.url_editor.update(cx, |editor, cx| {
            editor.select_all(&SelectAll, window, cx);
        });
    }

    fn on_editor_blur(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reset_editor_text(window, cx);
    }

    /// `enter` reaches here as `menu::Confirm`: single-line editors don't bind
    /// `enter` themselves, so the global binding wins and dispatches through
    /// this view's element tree.
    fn confirm(&mut self, _: &menu::Confirm, _window: &mut Window, cx: &mut Context<Self>) {
        let text = self.url_editor.read(cx).text(cx);
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        cx.emit(OmniboxEvent::Navigate(text_to_url(text)));
    }

    fn cancel(&mut self, _: &menu::Cancel, window: &mut Window, cx: &mut Context<Self>) {
        self.reset_editor_text(window, cx);
    }
}

impl Focusable for Omnibox {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.url_editor.focus_handle(cx)
    }
}

impl Render for Omnibox {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .key_context("Omnibox")
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .flex_1()
            .min_w_0()
            .px_2()
            .py_0p5()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().colors().border)
            .bg(cx.theme().colors().editor_background)
            .child(self.url_editor.clone())
    }
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
/// search.
pub fn text_to_url(text: &str) -> String {
    if text.starts_with("http://") || text.starts_with("https://") {
        return text.to_string();
    }

    if !looks_like_url(text) {
        let encoded: String = url::form_urlencoded::byte_serialize(text.as_bytes()).collect();
        return format!("https://www.google.com/search?q={encoded}");
    }

    if should_use_http_by_default(text) {
        format!("http://{text}")
    } else {
        format!("https://{text}")
    }
}

#[cfg(test)]
mod tests {
    use super::{looks_like_url, text_to_url};

    #[test]
    fn localhost_inputs_are_treated_as_urls() {
        assert!(looks_like_url("localhost"));
        assert!(looks_like_url("localhost:3000"));
        assert_eq!(text_to_url("localhost"), "http://localhost");
        assert_eq!(text_to_url("localhost:3000"), "http://localhost:3000");
    }

    #[test]
    fn regular_domains_default_to_https() {
        assert!(looks_like_url("example.com"));
        assert_eq!(text_to_url("example.com"), "https://example.com");
    }

    #[test]
    fn plain_queries_still_search() {
        assert!(!looks_like_url("rust ownership"));
        assert_eq!(
            text_to_url("rust ownership"),
            "https://www.google.com/search?q=rust+ownership"
        );
    }
}
