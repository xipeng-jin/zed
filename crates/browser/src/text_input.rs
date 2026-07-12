//! Web text-input routing, ported from
//! `Glass:crates/browser/src/text_input.rs` and the editability half of
//! `Glass:crates/browser/src/page_chrome.rs` (plan §7 M2 step 7).
//!
//! Each keystroke aimed at page content classifies three ways: `App`
//! (ctrl/platform chords belong to Zed even when no binding matched, so pages
//! cannot shadow app shortcuts), `TextInput` (printable input for an editable
//! field, routed through GPUI's input-handler path so IME composition works),
//! or `Browser` (raw engine key event). Whether the focused DOM node is
//! editable is reported by the render process via a process message whenever
//! the focused node changes; the browser process folds it into per-tab state.

use gpui::Keystroke;

/// The render process's report of the page's focused-node editability,
/// carried across the process boundary and stored per browser tab.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BrowserTextInputState {
    pub editable: bool,
}

impl BrowserTextInputState {
    /// Whether the app should accept platform text input for the page. An
    /// in-flight composition keeps text input active even if the render
    /// process briefly reports a non-editable focus.
    pub fn is_active(self, has_marked_text: bool) -> bool {
        self.editable || has_marked_text
    }
}

/// Where a keystroke aimed at page content routes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BrowserKeyDispatch {
    /// Zed's own dispatch handles it; do not swallow or forward it.
    App,
    /// Forward as a raw engine key event.
    Browser,
    /// Leave it to GPUI's text-input path (`EntityInputHandler`), which
    /// commits or composes through the IME methods on the tab-backend seam.
    TextInput,
}

pub(crate) fn key_down_dispatch(
    keystroke: &Keystroke,
    text_input_editable: bool,
    text_input_composing: bool,
) -> BrowserKeyDispatch {
    if keystroke.modifiers.platform || keystroke.modifiers.control {
        BrowserKeyDispatch::App
    } else if should_use_text_input(keystroke, text_input_editable, text_input_composing) {
        BrowserKeyDispatch::TextInput
    } else {
        BrowserKeyDispatch::Browser
    }
}

pub(crate) fn key_up_dispatch(
    keystroke: &Keystroke,
    text_input_editable: bool,
    text_input_composing: bool,
) -> BrowserKeyDispatch {
    if keystroke.modifiers.platform || keystroke.modifiers.control {
        BrowserKeyDispatch::App
    } else if should_use_text_input(keystroke, text_input_editable, text_input_composing) {
        BrowserKeyDispatch::TextInput
    } else {
        BrowserKeyDispatch::Browser
    }
}

fn should_use_text_input(
    keystroke: &Keystroke,
    text_input_editable: bool,
    text_input_composing: bool,
) -> bool {
    if text_input_composing {
        return true;
    }

    if !text_input_editable {
        return false;
    }

    // Enter and tab act on the page (submit, focus traversal) even though
    // platforms report them as character input.
    if matches!(keystroke.key.as_str(), "enter" | "tab") {
        return false;
    }

    if keystroke.key_char.is_some() {
        return true;
    }

    // Named keys without character input (navigation, editing, function keys)
    // act on the page; everything else — dead keys included — belongs to the
    // text-input path.
    !matches!(
        keystroke.key.as_str(),
        "enter"
            | "backspace"
            | "tab"
            | "delete"
            | "escape"
            | "space"
            | "left"
            | "right"
            | "up"
            | "down"
            | "home"
            | "end"
            | "pageup"
            | "pagedown"
            | "f1"
            | "f2"
            | "f3"
            | "f4"
            | "f5"
            | "f6"
            | "f7"
            | "f8"
            | "f9"
            | "f10"
            | "f11"
            | "f12"
    )
}

/// What committing `text` through the input handler should do to the page.
/// Some IMEs commit a bare newline for the confirm key; the page expects an
/// Enter keypress (submit), not an inserted character
/// (`Glass:crates/browser/src/input.rs:205`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommittedTextAction {
    InsertText,
    PressEnter,
}

pub(crate) fn committed_text_action(text: &str) -> CommittedTextAction {
    if matches!(text, "\r" | "\n" | "\r\n") {
        CommittedTextAction::PressEnter
    } else {
        CommittedTextAction::InsertText
    }
}

#[cfg(feature = "cef")]
pub(crate) use engine::{extract_text_input_state_from_message, send_text_input_state};

/// The process-message plumbing that carries the editability signal from the
/// render process to the browser process.
#[cfg(feature = "cef")]
mod engine {
    use super::BrowserTextInputState;
    use cef::{
        CefString, Domnode, Frame, ImplDomnode, ImplFrame, ImplListValue, ImplProcessMessage,
        ProcessId, ProcessMessage, process_message_create,
    };

    const TEXT_INPUT_STATE_MESSAGE_NAME: &str = "zed.text_input_state";

    /// Decode a `zed.text_input_state` process message in the browser
    /// process. Returns `None` for unrelated messages.
    pub(crate) fn extract_text_input_state_from_message(
        message: &mut ProcessMessage,
    ) -> Option<BrowserTextInputState> {
        if CefString::from(&message.name()).to_string() != TEXT_INPUT_STATE_MESSAGE_NAME {
            return None;
        }

        let args = message.argument_list()?;
        Some(BrowserTextInputState {
            editable: args.bool(0) != 0,
        })
    }

    /// Report the focused node's editability from the render process. Invoked
    /// by the crate's render-process handler (`page_chrome.rs`).
    pub(crate) fn send_text_input_state(frame: &mut Frame, focused_node: Option<&Domnode>) {
        let Some(message) =
            process_message_create(Some(&CefString::from(TEXT_INPUT_STATE_MESSAGE_NAME)))
        else {
            log::warn!("[browser::text_input] failed to create the text-input state message");
            return;
        };

        let Some(args) = message.argument_list() else {
            log::warn!("[browser::text_input] text-input state message has no argument list");
            return;
        };

        args.set_bool(
            0,
            focused_node.is_some_and(|node| node.is_editable() != 0) as i32,
        );

        let mut message = message;
        frame.send_process_message(ProcessId::BROWSER, Some(&mut message));
    }

}

#[cfg(test)]
mod tests {
    use super::{
        BrowserKeyDispatch, CommittedTextAction, committed_text_action, key_down_dispatch,
        key_up_dispatch,
    };
    use gpui::{Keystroke, Modifiers};

    fn keystroke(key: &str, key_char: Option<&str>, modifiers: Modifiers) -> Keystroke {
        Keystroke {
            key: key.into(),
            key_char: key_char.map(str::to_string),
            modifiers,
        }
    }

    #[test]
    fn printable_keys_use_browser_route_when_page_is_not_editable() {
        let keystroke = keystroke("e", Some("e"), Modifiers::default());

        assert_eq!(
            key_down_dispatch(&keystroke, false, false),
            BrowserKeyDispatch::Browser
        );
        assert_eq!(
            key_up_dispatch(&keystroke, false, false),
            BrowserKeyDispatch::Browser
        );
    }

    #[test]
    fn printable_keys_use_text_input_when_page_is_editable() {
        let keystroke = keystroke("e", Some("e"), Modifiers::default());

        assert_eq!(
            key_down_dispatch(&keystroke, true, false),
            BrowserKeyDispatch::TextInput
        );
        assert_eq!(
            key_up_dispatch(&keystroke, true, false),
            BrowserKeyDispatch::TextInput
        );
    }

    #[test]
    fn navigation_keys_still_reach_browser_when_page_is_editable() {
        let keystroke = keystroke("left", None, Modifiers::default());

        assert_eq!(
            key_down_dispatch(&keystroke, true, false),
            BrowserKeyDispatch::Browser
        );
        assert_eq!(
            key_up_dispatch(&keystroke, true, false),
            BrowserKeyDispatch::Browser
        );
    }

    #[test]
    fn enter_routes_to_browser_even_when_platform_reports_character_input() {
        let keystroke = keystroke("enter", Some("\r"), Modifiers::default());

        assert_eq!(
            key_down_dispatch(&keystroke, true, false),
            BrowserKeyDispatch::Browser
        );
        assert_eq!(
            key_up_dispatch(&keystroke, true, false),
            BrowserKeyDispatch::Browser
        );
    }

    #[test]
    fn command_shortcuts_stay_in_app_dispatch() {
        let keystroke = keystroke("c", Some("c"), Modifiers::command());

        assert_eq!(
            key_down_dispatch(&keystroke, true, false),
            BrowserKeyDispatch::App
        );
        assert_eq!(
            key_up_dispatch(&keystroke, true, false),
            BrowserKeyDispatch::App
        );
    }

    #[test]
    fn function_printable_keys_use_browser_route_when_page_is_not_editable() {
        let keystroke = keystroke(
            "e",
            Some("e"),
            Modifiers {
                function: true,
                ..Modifiers::default()
            },
        );

        assert_eq!(
            key_down_dispatch(&keystroke, false, false),
            BrowserKeyDispatch::Browser
        );
        assert_eq!(
            key_up_dispatch(&keystroke, false, false),
            BrowserKeyDispatch::Browser
        );
    }

    #[test]
    fn function_printable_keys_use_text_input_when_page_is_editable() {
        let keystroke = keystroke(
            "e",
            Some("e"),
            Modifiers {
                function: true,
                ..Modifiers::default()
            },
        );

        assert_eq!(
            key_down_dispatch(&keystroke, true, false),
            BrowserKeyDispatch::TextInput
        );
        assert_eq!(
            key_up_dispatch(&keystroke, true, false),
            BrowserKeyDispatch::TextInput
        );
    }

    #[test]
    fn composing_keys_stay_in_text_input_route() {
        let keystroke = keystroke("e", Some("e"), Modifiers::default());

        assert_eq!(
            key_down_dispatch(&keystroke, true, true),
            BrowserKeyDispatch::TextInput
        );
        assert_eq!(
            key_up_dispatch(&keystroke, true, true),
            BrowserKeyDispatch::TextInput
        );
    }

    #[test]
    fn dead_keys_stay_in_text_input_route_for_editable_fields() {
        let keystroke = keystroke("dead-acute", None, Modifiers::default());

        assert_eq!(
            key_down_dispatch(&keystroke, true, false),
            BrowserKeyDispatch::TextInput
        );
        assert_eq!(
            key_up_dispatch(&keystroke, true, false),
            BrowserKeyDispatch::TextInput
        );
    }

    #[test]
    fn committed_newlines_press_enter_instead_of_inserting() {
        assert_eq!(committed_text_action("\n"), CommittedTextAction::PressEnter);
        assert_eq!(committed_text_action("\r"), CommittedTextAction::PressEnter);
        assert_eq!(
            committed_text_action("\r\n"),
            CommittedTextAction::PressEnter
        );
        assert_eq!(committed_text_action("你"), CommittedTextAction::InsertText);
        assert_eq!(
            committed_text_action("a\n"),
            CommittedTextAction::InsertText
        );
    }
}
