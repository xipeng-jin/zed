//! GPUI → CEF input event conversion, ported from
//! `Glass:crates/browser/src/input.rs`. Glass built key events from a
//! fork-only `Keystroke.native_key_code` field on macOS; upstream gpui has no
//! such field, so key events are built purely from gpui key names via the
//! portable table in `keycodes.rs` (migration plan §7 M1 step 7). CEF derives
//! physical pixels from view coordinates itself, so all positions here are
//! logical, content-relative pixels.

use crate::keycodes::key_name_to_windows_vk;
use cef::{KeyEvent, KeyEventType, MouseButtonType};
use gpui::{Keystroke, Modifiers, MouseButton, Pixels, Point, ScrollDelta};

const EVENTFLAG_SHIFT_DOWN: u32 = 1 << 1;
const EVENTFLAG_CONTROL_DOWN: u32 = 1 << 2;
const EVENTFLAG_ALT_DOWN: u32 = 1 << 3;
const EVENTFLAG_LEFT_MOUSE_BUTTON: u32 = 1 << 4;
const EVENTFLAG_MIDDLE_MOUSE_BUTTON: u32 = 1 << 5;
const EVENTFLAG_RIGHT_MOUSE_BUTTON: u32 = 1 << 6;
#[cfg(target_os = "macos")]
const EVENTFLAG_COMMAND_DOWN: u32 = 1 << 7;

/// Pixels per line for converting line-based scroll deltas (mouse wheels)
/// into the pixel deltas CEF expects (`Glass:crates/browser/src/input.rs:61`).
const SCROLL_LINE_HEIGHT_PX: f32 = 40.0;

pub(crate) fn mouse_event(position: Point<Pixels>, modifiers: u32) -> cef::MouseEvent {
    cef::MouseEvent {
        x: f32::from(position.x) as i32,
        y: f32::from(position.y) as i32,
        modifiers,
    }
}

pub(crate) fn convert_mouse_button(button: MouseButton) -> MouseButtonType {
    match button {
        MouseButton::Left | MouseButton::Navigate(_) => MouseButtonType::LEFT,
        MouseButton::Middle => MouseButtonType::MIDDLE,
        MouseButton::Right => MouseButtonType::RIGHT,
    }
}

pub(crate) fn pressed_button_flags(pressed_button: Option<MouseButton>) -> u32 {
    match pressed_button {
        Some(MouseButton::Left) | Some(MouseButton::Navigate(_)) => EVENTFLAG_LEFT_MOUSE_BUTTON,
        Some(MouseButton::Middle) => EVENTFLAG_MIDDLE_MOUSE_BUTTON,
        Some(MouseButton::Right) => EVENTFLAG_RIGHT_MOUSE_BUTTON,
        None => 0,
    }
}

pub(crate) fn scroll_delta_to_pixels(delta: ScrollDelta) -> (i32, i32) {
    match delta {
        ScrollDelta::Pixels(delta) => (f32::from(delta.x) as i32, f32::from(delta.y) as i32),
        ScrollDelta::Lines(delta) => (
            (delta.x * SCROLL_LINE_HEIGHT_PX) as i32,
            (delta.y * SCROLL_LINE_HEIGHT_PX) as i32,
        ),
    }
}

pub(crate) fn convert_key_event(keystroke: &Keystroke, is_down: bool) -> KeyEvent {
    KeyEvent {
        type_: if is_down {
            KeyEventType::RAWKEYDOWN
        } else {
            KeyEventType::KEYUP
        },
        modifiers: convert_modifiers(&keystroke.modifiers),
        windows_key_code: key_name_to_windows_vk(&keystroke.key),
        native_key_code: 0,
        is_system_key: 0,
        character: key_character(keystroke),
        unmodified_character: unmodified_key_character(keystroke),
        focus_on_editable_field: 1,
        ..Default::default()
    }
}

pub(crate) fn create_char_event(keystroke: &Keystroke) -> Option<KeyEvent> {
    let character = key_character(keystroke);
    if character == 0 {
        return None;
    }

    Some(KeyEvent {
        type_: KeyEventType::CHAR,
        modifiers: convert_modifiers(&keystroke.modifiers),
        windows_key_code: character as i32,
        character,
        unmodified_character: character,
        focus_on_editable_field: 1,
        ..Default::default()
    })
}

pub(crate) fn should_send_char_event(keystroke: &Keystroke, is_held: bool) -> bool {
    if is_held || keystroke.modifiers.platform || keystroke.modifiers.control {
        return false;
    }

    !crate::text_input::NON_CHARACTER_NAMED_KEYS.contains(&keystroke.key.as_str())
}

/// The fixed codepoint a named key contributes to CEF char events, shared by
/// the modified and unmodified character fields.
fn named_key_character(key: &str) -> Option<u16> {
    match key {
        "enter" => Some(0x0D),
        "backspace" => Some(0x08),
        "tab" => Some(0x09),
        "escape" => Some(0x1B),
        "space" => Some(' ' as u16),
        "delete" => Some(0x7F),
        _ => None,
    }
}

fn key_character(keystroke: &Keystroke) -> u16 {
    named_key_character(&keystroke.key).unwrap_or_else(|| {
        keystroke
            .key_char
            .as_ref()
            .and_then(|text| text.chars().next())
            .map(|character| character as u16)
            .or_else(|| {
                (keystroke.key.len() == 1)
                    .then(|| keystroke.key.chars().next())
                    .flatten()
                    .filter(|character| !character.is_control())
                    .map(|character| character as u16)
            })
            .unwrap_or(0)
    })
}

fn unmodified_key_character(keystroke: &Keystroke) -> u16 {
    named_key_character(&keystroke.key).unwrap_or_else(|| {
        if keystroke.key.len() == 1 {
            keystroke
                .key
                .chars()
                .next()
                .map(|character| character as u16)
                .unwrap_or(0)
        } else {
            0
        }
    })
}

pub(crate) fn convert_modifiers(modifiers: &Modifiers) -> u32 {
    let mut result = 0u32;

    if modifiers.shift {
        result |= EVENTFLAG_SHIFT_DOWN;
    }
    if modifiers.control {
        result |= EVENTFLAG_CONTROL_DOWN;
    }
    if modifiers.alt {
        result |= EVENTFLAG_ALT_DOWN;
    }
    if modifiers.platform {
        #[cfg(target_os = "macos")]
        {
            result |= EVENTFLAG_COMMAND_DOWN;
        }
        #[cfg(not(target_os = "macos"))]
        {
            result |= EVENTFLAG_CONTROL_DOWN;
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Modifiers, point, px};

    fn keystroke(key: &str, key_char: Option<&str>, modifiers: Modifiers) -> Keystroke {
        Keystroke {
            key: key.into(),
            key_char: key_char.map(str::to_string),
            modifiers,
        }
    }

    #[test]
    fn enter_emits_char_event() {
        let keystroke = keystroke("enter", None, Modifiers::default());

        assert!(should_send_char_event(&keystroke, false));
        let char_event = create_char_event(&keystroke).unwrap();
        assert_eq!(char_event.character, 0x0D);
    }

    #[test]
    fn printable_text_emits_char_event() {
        let keystroke = keystroke("e", Some("e"), Modifiers::default());

        assert!(should_send_char_event(&keystroke, false));
        let char_event = create_char_event(&keystroke).unwrap();
        assert_eq!(char_event.character, 'e' as u16);
    }

    #[test]
    fn held_and_modified_keys_emit_no_char_event() {
        let plain = keystroke("e", Some("e"), Modifiers::default());
        assert!(!should_send_char_event(&plain, true));

        let control = keystroke("e", Some("e"), Modifiers::control());
        assert!(!should_send_char_event(&control, false));
    }

    #[test]
    fn shifted_letter_reports_shifted_and_unmodified_characters() {
        let keystroke = keystroke("a", Some("A"), Modifiers::shift());

        let event = convert_key_event(&keystroke, true);
        assert_eq!(event.windows_key_code, 0x41);
        assert_eq!(event.character, 'A' as u16);
        assert_eq!(event.unmodified_character, 'a' as u16);
        assert_eq!(event.modifiers, 1 << 1);
    }

    #[test]
    fn platform_modifier_maps_to_control_on_non_mac() {
        let modifiers = convert_modifiers(&Modifiers {
            platform: true,
            ..Modifiers::default()
        });

        #[cfg(not(target_os = "macos"))]
        assert_eq!(modifiers, 1 << 2);
        #[cfg(target_os = "macos")]
        assert_eq!(modifiers, 1 << 7);
    }

    #[test]
    fn line_scroll_deltas_convert_at_forty_pixels_per_line() {
        assert_eq!(
            scroll_delta_to_pixels(ScrollDelta::Lines(point(0.0, -3.0))),
            (0, -120)
        );
        assert_eq!(
            scroll_delta_to_pixels(ScrollDelta::Pixels(point(px(12.0), px(-7.0)))),
            (12, -7)
        );
    }
}
