use std::borrow::Cow;

/// The mappings defined in this file where created from reading the alacritty source
use gpui::Keystroke;

use crate::Modes;

#[derive(Debug, PartialEq, Eq)]
enum TerminalModifiers {
    None,
    Alt,
    Ctrl,
    Shift,
    CtrlShift,
    Other,
}

impl TerminalModifiers {
    fn new(ks: &Keystroke) -> Self {
        match (
            ks.modifiers.alt,
            ks.modifiers.control,
            ks.modifiers.shift,
            ks.modifiers.platform,
        ) {
            (false, false, false, false) => TerminalModifiers::None,
            (true, false, false, false) => TerminalModifiers::Alt,
            (false, true, false, false) => TerminalModifiers::Ctrl,
            (false, false, true, false) => TerminalModifiers::Shift,
            (false, true, true, false) => TerminalModifiers::CtrlShift,
            _ => TerminalModifiers::Other,
        }
    }

    fn any(&self) -> bool {
        match &self {
            TerminalModifiers::None => false,
            TerminalModifiers::Alt => true,
            TerminalModifiers::Ctrl => true,
            TerminalModifiers::Shift => true,
            TerminalModifiers::CtrlShift => true,
            TerminalModifiers::Other => true,
        }
    }
}

/// Encode a keystroke into the bytes to write to the PTY.
///
/// On Linux the escape bytes come from ghostty's `key::Encoder` (SPEC.md
/// §4.3); macOS and Windows keep the alacritty-era `to_esc_str` path until
/// their platform gates open (SPEC.md §5, §8).
pub(crate) fn encode_keystroke(
    keystroke: &Keystroke,
    mode: Modes,
    option_as_meta: bool,
) -> Option<Vec<u8>> {
    #[cfg(target_os = "linux")]
    {
        ghostty_encode_keystroke(keystroke, mode, option_as_meta)
    }
    #[cfg(not(target_os = "linux"))]
    {
        to_esc_str(keystroke, mode, option_as_meta).map(|esc| esc.into_owned().into_bytes())
    }
}

// Alacritty-era path: production on macOS/Windows, contract-test oracle on
// Linux. Deleted at P10.
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(crate) fn to_esc_str(
    keystroke: &Keystroke,
    mode: Modes,
    option_as_meta: bool,
) -> Option<Cow<'static, str>> {
    let modifiers = TerminalModifiers::new(keystroke);

    // Manual Bindings including modifiers
    let manual_esc_str: Option<&'static str> = match (keystroke.key.as_ref(), &modifiers) {
        //Basic special keys
        ("tab", TerminalModifiers::None) => Some("\x09"),
        ("escape", TerminalModifiers::None) => Some("\x1b"),
        ("enter", TerminalModifiers::None) => Some("\x0d"),
        ("enter", TerminalModifiers::Shift) => Some("\x0a"),
        ("enter", TerminalModifiers::Alt) => Some("\x1b\x0d"),
        ("backspace", TerminalModifiers::None) => Some("\x7f"),
        //Interesting escape codes
        ("tab", TerminalModifiers::Shift) => Some("\x1b[Z"),
        ("backspace", TerminalModifiers::Ctrl) => Some("\x08"),
        ("backspace", TerminalModifiers::Alt) => Some("\x1b\x7f"),
        ("backspace", TerminalModifiers::Shift) => Some("\x7f"),
        ("space", TerminalModifiers::Ctrl) => Some("\x00"),
        ("home", TerminalModifiers::None) if mode.contains(Modes::APP_CURSOR) => Some("\x1bOH"),
        ("home", TerminalModifiers::None) if !mode.contains(Modes::APP_CURSOR) => Some("\x1b[H"),
        ("end", TerminalModifiers::None) if mode.contains(Modes::APP_CURSOR) => Some("\x1bOF"),
        ("end", TerminalModifiers::None) if !mode.contains(Modes::APP_CURSOR) => Some("\x1b[F"),
        ("up", TerminalModifiers::None) if mode.contains(Modes::APP_CURSOR) => Some("\x1bOA"),
        ("up", TerminalModifiers::None) if !mode.contains(Modes::APP_CURSOR) => Some("\x1b[A"),
        ("down", TerminalModifiers::None) if mode.contains(Modes::APP_CURSOR) => Some("\x1bOB"),
        ("down", TerminalModifiers::None) if !mode.contains(Modes::APP_CURSOR) => Some("\x1b[B"),
        ("right", TerminalModifiers::None) if mode.contains(Modes::APP_CURSOR) => Some("\x1bOC"),
        ("right", TerminalModifiers::None) if !mode.contains(Modes::APP_CURSOR) => Some("\x1b[C"),
        ("left", TerminalModifiers::None) if mode.contains(Modes::APP_CURSOR) => Some("\x1bOD"),
        ("left", TerminalModifiers::None) if !mode.contains(Modes::APP_CURSOR) => Some("\x1b[D"),
        ("back", TerminalModifiers::None) => Some("\x7f"),
        ("insert", TerminalModifiers::None) => Some("\x1b[2~"),
        ("delete", TerminalModifiers::None) => Some("\x1b[3~"),
        ("pageup", TerminalModifiers::None) => Some("\x1b[5~"),
        ("pagedown", TerminalModifiers::None) => Some("\x1b[6~"),
        ("f1", TerminalModifiers::None) => Some("\x1bOP"),
        ("f2", TerminalModifiers::None) => Some("\x1bOQ"),
        ("f3", TerminalModifiers::None) => Some("\x1bOR"),
        ("f4", TerminalModifiers::None) => Some("\x1bOS"),
        ("f5", TerminalModifiers::None) => Some("\x1b[15~"),
        ("f6", TerminalModifiers::None) => Some("\x1b[17~"),
        ("f7", TerminalModifiers::None) => Some("\x1b[18~"),
        ("f8", TerminalModifiers::None) => Some("\x1b[19~"),
        ("f9", TerminalModifiers::None) => Some("\x1b[20~"),
        ("f10", TerminalModifiers::None) => Some("\x1b[21~"),
        ("f11", TerminalModifiers::None) => Some("\x1b[23~"),
        ("f12", TerminalModifiers::None) => Some("\x1b[24~"),
        ("f13", TerminalModifiers::None) => Some("\x1b[25~"),
        ("f14", TerminalModifiers::None) => Some("\x1b[26~"),
        ("f15", TerminalModifiers::None) => Some("\x1b[28~"),
        ("f16", TerminalModifiers::None) => Some("\x1b[29~"),
        ("f17", TerminalModifiers::None) => Some("\x1b[31~"),
        ("f18", TerminalModifiers::None) => Some("\x1b[32~"),
        ("f19", TerminalModifiers::None) => Some("\x1b[33~"),
        ("f20", TerminalModifiers::None) => Some("\x1b[34~"),
        // NumpadEnter, Action::Esc("\n".into());
        //Mappings for caret notation keys
        ("a", TerminalModifiers::Ctrl) => Some("\x01"), //1
        ("A", TerminalModifiers::CtrlShift) => Some("\x01"), //1
        ("b", TerminalModifiers::Ctrl) => Some("\x02"), //2
        ("B", TerminalModifiers::CtrlShift) => Some("\x02"), //2
        ("c", TerminalModifiers::Ctrl) => Some("\x03"), //3
        ("C", TerminalModifiers::CtrlShift) => Some("\x03"), //3
        ("d", TerminalModifiers::Ctrl) => Some("\x04"), //4
        ("D", TerminalModifiers::CtrlShift) => Some("\x04"), //4
        ("e", TerminalModifiers::Ctrl) => Some("\x05"), //5
        ("E", TerminalModifiers::CtrlShift) => Some("\x05"), //5
        ("f", TerminalModifiers::Ctrl) => Some("\x06"), //6
        ("F", TerminalModifiers::CtrlShift) => Some("\x06"), //6
        ("g", TerminalModifiers::Ctrl) => Some("\x07"), //7
        ("G", TerminalModifiers::CtrlShift) => Some("\x07"), //7
        ("h", TerminalModifiers::Ctrl) => Some("\x08"), //8
        ("H", TerminalModifiers::CtrlShift) => Some("\x08"), //8
        ("i", TerminalModifiers::Ctrl) => Some("\x09"), //9
        ("I", TerminalModifiers::CtrlShift) => Some("\x09"), //9
        ("j", TerminalModifiers::Ctrl) => Some("\x0a"), //10
        ("J", TerminalModifiers::CtrlShift) => Some("\x0a"), //10
        ("k", TerminalModifiers::Ctrl) => Some("\x0b"), //11
        ("K", TerminalModifiers::CtrlShift) => Some("\x0b"), //11
        ("l", TerminalModifiers::Ctrl) => Some("\x0c"), //12
        ("L", TerminalModifiers::CtrlShift) => Some("\x0c"), //12
        ("m", TerminalModifiers::Ctrl) => Some("\x0d"), //13
        ("M", TerminalModifiers::CtrlShift) => Some("\x0d"), //13
        ("n", TerminalModifiers::Ctrl) => Some("\x0e"), //14
        ("N", TerminalModifiers::CtrlShift) => Some("\x0e"), //14
        ("o", TerminalModifiers::Ctrl) => Some("\x0f"), //15
        ("O", TerminalModifiers::CtrlShift) => Some("\x0f"), //15
        ("p", TerminalModifiers::Ctrl) => Some("\x10"), //16
        ("P", TerminalModifiers::CtrlShift) => Some("\x10"), //16
        ("q", TerminalModifiers::Ctrl) => Some("\x11"), //17
        ("Q", TerminalModifiers::CtrlShift) => Some("\x11"), //17
        ("r", TerminalModifiers::Ctrl) => Some("\x12"), //18
        ("R", TerminalModifiers::CtrlShift) => Some("\x12"), //18
        ("s", TerminalModifiers::Ctrl) => Some("\x13"), //19
        ("S", TerminalModifiers::CtrlShift) => Some("\x13"), //19
        ("t", TerminalModifiers::Ctrl) => Some("\x14"), //20
        ("T", TerminalModifiers::CtrlShift) => Some("\x14"), //20
        ("u", TerminalModifiers::Ctrl) => Some("\x15"), //21
        ("U", TerminalModifiers::CtrlShift) => Some("\x15"), //21
        ("v", TerminalModifiers::Ctrl) => Some("\x16"), //22
        ("V", TerminalModifiers::CtrlShift) => Some("\x16"), //22
        ("w", TerminalModifiers::Ctrl) => Some("\x17"), //23
        ("W", TerminalModifiers::CtrlShift) => Some("\x17"), //23
        ("x", TerminalModifiers::Ctrl) => Some("\x18"), //24
        ("X", TerminalModifiers::CtrlShift) => Some("\x18"), //24
        ("y", TerminalModifiers::Ctrl) => Some("\x19"), //25
        ("Y", TerminalModifiers::CtrlShift) => Some("\x19"), //25
        ("z", TerminalModifiers::Ctrl) => Some("\x1a"), //26
        ("Z", TerminalModifiers::CtrlShift) => Some("\x1a"), //26
        ("@", TerminalModifiers::Ctrl) => Some("\x00"), //0
        ("[", TerminalModifiers::Ctrl) => Some("\x1b"), //27
        ("\\", TerminalModifiers::Ctrl) => Some("\x1c"), //28
        ("]", TerminalModifiers::Ctrl) => Some("\x1d"), //29
        ("^", TerminalModifiers::Ctrl) => Some("\x1e"), //30
        ("_", TerminalModifiers::Ctrl) => Some("\x1f"), //31
        ("?", TerminalModifiers::Ctrl) => Some("\x7f"), //127
        _ => None,
    };
    if let Some(esc_str) = manual_esc_str {
        return Some(Cow::Borrowed(esc_str));
    }

    // Automated bindings applying modifiers
    if modifiers.any() {
        let modifier_code = modifier_code(keystroke);
        let modified_esc_str = match keystroke.key.as_ref() {
            "up" => Some(format!("\x1b[1;{}A", modifier_code)),
            "down" => Some(format!("\x1b[1;{}B", modifier_code)),
            "right" => Some(format!("\x1b[1;{}C", modifier_code)),
            "left" => Some(format!("\x1b[1;{}D", modifier_code)),
            "f1" => Some(format!("\x1b[1;{}P", modifier_code)),
            "f2" => Some(format!("\x1b[1;{}Q", modifier_code)),
            "f3" => Some(format!("\x1b[1;{}R", modifier_code)),
            "f4" => Some(format!("\x1b[1;{}S", modifier_code)),
            "F5" => Some(format!("\x1b[15;{}~", modifier_code)),
            "f6" => Some(format!("\x1b[17;{}~", modifier_code)),
            "f7" => Some(format!("\x1b[18;{}~", modifier_code)),
            "f8" => Some(format!("\x1b[19;{}~", modifier_code)),
            "f9" => Some(format!("\x1b[20;{}~", modifier_code)),
            "f10" => Some(format!("\x1b[21;{}~", modifier_code)),
            "f11" => Some(format!("\x1b[23;{}~", modifier_code)),
            "f12" => Some(format!("\x1b[24;{}~", modifier_code)),
            "f13" => Some(format!("\x1b[25;{}~", modifier_code)),
            "f14" => Some(format!("\x1b[26;{}~", modifier_code)),
            "f15" => Some(format!("\x1b[28;{}~", modifier_code)),
            "f16" => Some(format!("\x1b[29;{}~", modifier_code)),
            "f17" => Some(format!("\x1b[31;{}~", modifier_code)),
            "f18" => Some(format!("\x1b[32;{}~", modifier_code)),
            "f19" => Some(format!("\x1b[33;{}~", modifier_code)),
            "f20" => Some(format!("\x1b[34;{}~", modifier_code)),
            "insert" => Some(format!("\x1b[2;{}~", modifier_code)),
            "pageup" => Some(format!("\x1b[5;{}~", modifier_code)),
            "pagedown" => Some(format!("\x1b[6;{}~", modifier_code)),
            "end" => Some(format!("\x1b[1;{}F", modifier_code)),
            "home" => Some(format!("\x1b[1;{}H", modifier_code)),
            _ => None,
        };
        if let Some(esc_str) = modified_esc_str {
            return Some(Cow::Owned(esc_str));
        }
    }

    if !cfg!(target_os = "macos") || option_as_meta {
        let is_alt_lowercase_ascii =
            modifiers == TerminalModifiers::Alt && keystroke.key.is_ascii();
        let is_alt_uppercase_ascii =
            keystroke.modifiers.alt && keystroke.modifiers.shift && keystroke.key.is_ascii();
        if is_alt_lowercase_ascii || is_alt_uppercase_ascii {
            let key = if is_alt_uppercase_ascii {
                &keystroke.key.to_ascii_uppercase()
            } else {
                &keystroke.key
            };
            return Some(Cow::Owned(format!("\x1b{}", key)));
        }
    }

    None
}

///   Code     Modifiers
/// ---------+---------------------------
///    2     | Shift
///    3     | Alt
///    4     | Shift + Alt
///    5     | Control
///    6     | Shift + Control
///    7     | Alt + Control
///    8     | Shift + Alt + Control
/// ---------+---------------------------
/// from: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h2-PC-Style-Function-Keys
fn modifier_code(keystroke: &Keystroke) -> u32 {
    let mut modifier_code = 0;
    if keystroke.modifiers.shift {
        modifier_code |= 1;
    }
    if keystroke.modifiers.alt {
        modifier_code |= 1 << 1;
    }
    if keystroke.modifiers.control {
        modifier_code |= 1 << 2;
    }
    modifier_code + 1
}

#[cfg(target_os = "linux")]
fn ghostty_encode_keystroke(
    keystroke: &Keystroke,
    mode: Modes,
    option_as_meta: bool,
) -> Option<Vec<u8>> {
    use ghostty_vt::key::{Action, Event, Mods};
    use util::ResultExt as _;

    let modifiers = &keystroke.modifiers;
    let shift_only =
        modifiers.shift && !modifiers.control && !modifiers.alt && !modifiers.platform;

    // Zed input policy: shift-enter types a literal newline (multi-line shell
    // input); a plain control byte, not an escape sequence, so it stays
    // upstream of the encoder like vi-mode interception.
    if keystroke.key == "enter" && shift_only {
        return Some(b"\n".to_vec());
    }

    if let Some(bytes) = legacy_fill(keystroke) {
        return Some(bytes);
    }

    // Zed policy decides which keystrokes the terminal owns at all (the rest
    // fall through to the IME/text path); ghostty encodes the bytes for the
    // ones it does.
    let (key, mods, utf8, unshifted) = if let Some(key) = named_key(&keystroke.key) {
        let mut mods = Mods::empty();
        mods.set(Mods::SHIFT, modifiers.shift);
        mods.set(Mods::ALT, modifiers.alt);
        mods.set(Mods::CTRL, modifiers.control);
        mods.set(Mods::SUPER, modifiers.platform);
        (key, mods, None, None)
    } else {
        let character = char_of_key(&keystroke.key)?;
        let unshifted = character.to_ascii_lowercase();
        if modifiers.control && !modifiers.alt && !modifiers.platform {
            // The caret-notation set the alacritty-era table encoded; other
            // ctrl combos (ctrl-1, ctrl-alt AltGr chords, …) keep falling
            // through to the text path.
            let qualifies = character.is_ascii_alphabetic()
                || (!modifiers.shift && matches!(character, '@' | '\\' | ']' | '^' | ' '));
            if !qualifies {
                return None;
            }
            (key_for_char(character), Mods::CTRL, None, Some(unshifted))
        } else if modifiers.alt
            && character.is_ascii()
            && (modifiers.shift || (!modifiers.control && !modifiers.platform))
        {
            // Alt-as-meta: ESC-prefixed text. Ctrl/super are dropped here to
            // match the alacritty-era table, which ignored them on this path.
            let mut mods = Mods::ALT;
            let text = if modifiers.shift {
                mods |= Mods::SHIFT;
                character.to_ascii_uppercase()
            } else {
                character
            };
            (
                key_for_char(character),
                mods,
                Some(text.to_string()),
                Some(unshifted),
            )
        } else {
            return None;
        }
    };

    let mut encoder = key_encoder(mode, option_as_meta)?;
    let mut event = Event::new().log_err()?;
    event
        .set_action(Action::Press)
        .set_key(key)
        .set_mods(mods)
        .set_utf8(utf8);
    if let Some(unshifted) = unshifted {
        event.set_unshifted_codepoint(unshifted);
    }
    let mut bytes = Vec::new();
    encoder.encode_to_vec(&event, &mut bytes).log_err()?;
    if bytes.is_empty() { None } else { Some(bytes) }
}

/// The temporary Zed-`Modes` → encoder-options shim (SPEC.md §4.3): feeds
/// ghostty's encoder from the alacritty core's mode snapshot. Replaced by
/// `set_options_from_terminal` at P8.
#[cfg(target_os = "linux")]
fn key_encoder(
    mode: Modes,
    option_as_meta: bool,
) -> Option<ghostty_vt::key::Encoder<'static>> {
    use ghostty_vt::key::{Encoder, KittyKeyFlags, OptionAsAlt};
    use util::ResultExt as _;

    let mut encoder = Encoder::new().log_err()?;
    encoder
        .set_cursor_key_application(mode.contains(Modes::APP_CURSOR))
        .set_keypad_key_application(mode.contains(Modes::APP_KEYPAD))
        // Zed's `Modes` does not track DEC 1036; the alacritty-era path
        // unconditionally ESC-prefixed alt on Linux.
        .set_alt_esc_prefix(true)
        .set_modify_other_keys_state_2(false)
        // Structurally off until the core swap (P8): the alacritty core never
        // answers the kitty progressive-enhancement query.
        .set_kitty_flags(KittyKeyFlags::DISABLED)
        .set_macos_option_as_alt(if option_as_meta {
            OptionAsAlt::True
        } else {
            OptionAsAlt::False
        });
    Some(encoder)
}

/// Sequences ghostty's legacy encoder deliberately does not produce, supplied
/// by the seam to preserve today's bytes (divergence ledger P3-001, P3-002):
/// F13–F20 xterm codes, ctrl-punctuation control bytes ghostty keys off the
/// physical key rather than the delivered character, and ctrl-i / ctrl-m,
/// which ghostty reserves so applications can tell them apart from tab/enter.
#[cfg(target_os = "linux")]
fn legacy_fill(keystroke: &Keystroke) -> Option<Vec<u8>> {
    let modifiers = &keystroke.modifiers;
    if modifiers.control && !modifiers.alt && !modifiers.platform {
        if !modifiers.shift {
            match keystroke.key.as_str() {
                "[" => return Some(vec![0x1b]),
                "_" => return Some(vec![0x1f]),
                "?" => return Some(vec![0x7f]),
                _ => {}
            }
        }
        // Shift is allowed for letters, matching the alacritty-era
        // ctrl-shift caret rows.
        match keystroke.key.to_ascii_lowercase().as_str() {
            "i" => return Some(vec![0x09]),
            "m" => return Some(vec![0x0d]),
            _ => {}
        }
    }

    let code = match keystroke.key.to_ascii_lowercase().as_str() {
        "f13" => 25,
        "f14" => 26,
        "f15" => 28,
        "f16" => 29,
        "f17" => 31,
        "f18" => 32,
        "f19" => 33,
        "f20" => 34,
        _ => return None,
    };
    let modifier_code = modifier_code(keystroke);
    let sequence = if modifier_code == 1 {
        format!("\x1b[{}~", code)
    } else {
        format!("\x1b[{};{}~", code, modifier_code)
    };
    Some(sequence.into_bytes())
}

#[cfg(target_os = "linux")]
fn named_key(key: &str) -> Option<ghostty_vt::key::Key> {
    use ghostty_vt::key::Key;

    // F13–F20 are handled by `legacy_fill` above.
    Some(match key.to_ascii_lowercase().as_str() {
        "tab" => Key::Tab,
        "escape" => Key::Escape,
        "enter" => Key::Enter,
        "backspace" | "back" => Key::Backspace,
        "up" => Key::ArrowUp,
        "down" => Key::ArrowDown,
        "left" => Key::ArrowLeft,
        "right" => Key::ArrowRight,
        "home" => Key::Home,
        "end" => Key::End,
        "insert" => Key::Insert,
        "delete" => Key::Delete,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "f1" => Key::F1,
        "f2" => Key::F2,
        "f3" => Key::F3,
        "f4" => Key::F4,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "f9" => Key::F9,
        "f10" => Key::F10,
        "f11" => Key::F11,
        "f12" => Key::F12,
        _ => return None,
    })
}

#[cfg(target_os = "linux")]
fn char_of_key(key: &str) -> Option<char> {
    if key == "space" {
        return Some(' ');
    }
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(character), None) => Some(character),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn key_for_char(character: char) -> ghostty_vt::key::Key {
    use ghostty_vt::key::Key;

    match character.to_ascii_lowercase() {
        'a' => Key::A,
        'b' => Key::B,
        'c' => Key::C,
        'd' => Key::D,
        'e' => Key::E,
        'f' => Key::F,
        'g' => Key::G,
        'h' => Key::H,
        'i' => Key::I,
        'j' => Key::J,
        'k' => Key::K,
        'l' => Key::L,
        'm' => Key::M,
        'n' => Key::N,
        'o' => Key::O,
        'p' => Key::P,
        'q' => Key::Q,
        'r' => Key::R,
        's' => Key::S,
        't' => Key::T,
        'u' => Key::U,
        'v' => Key::V,
        'w' => Key::W,
        'x' => Key::X,
        'y' => Key::Y,
        'z' => Key::Z,
        '0' | ')' => Key::Digit0,
        '1' | '!' => Key::Digit1,
        '2' | '@' => Key::Digit2,
        '3' | '#' => Key::Digit3,
        '4' | '$' => Key::Digit4,
        '5' | '%' => Key::Digit5,
        '6' | '^' => Key::Digit6,
        '7' | '&' => Key::Digit7,
        '8' | '*' => Key::Digit8,
        '9' | '(' => Key::Digit9,
        '-' | '_' => Key::Minus,
        '=' | '+' => Key::Equal,
        '[' | '{' => Key::BracketLeft,
        ']' | '}' => Key::BracketRight,
        '\\' | '|' => Key::Backslash,
        ';' | ':' => Key::Semicolon,
        '\'' | '"' => Key::Quote,
        ',' | '<' => Key::Comma,
        '.' | '>' => Key::Period,
        '/' | '?' => Key::Slash,
        '`' | '~' => Key::Backquote,
        ' ' => Key::Space,
        _ => Key::Unidentified,
    }
}

/// Alternate-scroll arrow bytes for [`super::mouse::alt_scroll`]: Zed
/// synthesizes the arrow-key events, ghostty encodes them. The alt screen
/// scroll always uses the application-cursor form (parity with the
/// alacritty-era hardcoded `\x1bO` prefix).
#[cfg(target_os = "linux")]
pub(super) fn ghostty_scroll_arrow_bytes(up: bool) -> Option<Vec<u8>> {
    use ghostty_vt::key::{Action, Encoder, Event, Key, KittyKeyFlags, Mods};
    use util::ResultExt as _;

    let mut encoder = Encoder::new().log_err()?;
    encoder
        .set_cursor_key_application(true)
        .set_kitty_flags(KittyKeyFlags::DISABLED);
    let mut event = Event::new().log_err()?;
    event
        .set_action(Action::Press)
        .set_key(if up { Key::ArrowUp } else { Key::ArrowDown })
        .set_mods(Mods::empty());
    let mut bytes = Vec::new();
    encoder.encode_to_vec(&event, &mut bytes).log_err()?;
    if bytes.is_empty() { None } else { Some(bytes) }
}

#[cfg(test)]
mod test {
    use gpui::Modifiers;

    use super::*;

    #[test]
    fn test_plain_inputs() {
        let ks = Keystroke {
            modifiers: Modifiers {
                control: false,
                alt: false,
                shift: false,
                platform: false,
                function: false,
            },
            key: "🖖🏻".to_string(), //2 char string
            key_char: None,
        };
        assert_eq!(to_esc_str(&ks, Modes::NONE, false), None);
    }

    #[test]
    fn test_application_mode() {
        let app_cursor = Modes::APP_CURSOR;
        let none = Modes::NONE;

        let up = Keystroke::parse("up").unwrap();
        let down = Keystroke::parse("down").unwrap();
        let left = Keystroke::parse("left").unwrap();
        let right = Keystroke::parse("right").unwrap();

        assert_eq!(to_esc_str(&up, none, false), Some("\x1b[A".into()));
        assert_eq!(to_esc_str(&down, none, false), Some("\x1b[B".into()));
        assert_eq!(to_esc_str(&right, none, false), Some("\x1b[C".into()));
        assert_eq!(to_esc_str(&left, none, false), Some("\x1b[D".into()));

        assert_eq!(to_esc_str(&up, app_cursor, false), Some("\x1bOA".into()));
        assert_eq!(to_esc_str(&down, app_cursor, false), Some("\x1bOB".into()));
        assert_eq!(to_esc_str(&right, app_cursor, false), Some("\x1bOC".into()));
        assert_eq!(to_esc_str(&left, app_cursor, false), Some("\x1bOD".into()));

        let home = Keystroke::parse("home").unwrap();
        let end = Keystroke::parse("end").unwrap();
        assert_eq!(to_esc_str(&home, none, false), Some("\x1b[H".into()));
        assert_eq!(to_esc_str(&end, none, false), Some("\x1b[F".into()));
        assert_eq!(to_esc_str(&home, app_cursor, false), Some("\x1bOH".into()));
        assert_eq!(to_esc_str(&end, app_cursor, false), Some("\x1bOF".into()));

        let shift_up = Keystroke::parse("shift-up").unwrap();
        let shift_down = Keystroke::parse("shift-down").unwrap();
        let shift_home = Keystroke::parse("shift-home").unwrap();
        let shift_end = Keystroke::parse("shift-end").unwrap();
        assert_eq!(to_esc_str(&shift_up, none, false), Some("\x1b[1;2A".into()));
        assert_eq!(
            to_esc_str(&shift_down, none, false),
            Some("\x1b[1;2B".into())
        );
        assert_eq!(
            to_esc_str(&shift_home, none, false),
            Some("\x1b[1;2H".into())
        );
        assert_eq!(
            to_esc_str(&shift_end, none, false),
            Some("\x1b[1;2F".into())
        );
    }

    #[test]
    fn test_ctrl_codes() {
        let letters_lower = 'a'..='z';
        let letters_upper = 'A'..='Z';
        let mode = Modes::APP_CURSOR;

        for (lower, upper) in letters_lower.zip(letters_upper) {
            assert_eq!(
                to_esc_str(
                    &Keystroke::parse(&format!("ctrl-shift-{}", lower)).unwrap(),
                    mode,
                    false
                ),
                to_esc_str(
                    &Keystroke::parse(&format!("ctrl-{}", upper)).unwrap(),
                    mode,
                    false
                ),
                "On letter: {}/{}",
                lower,
                upper
            )
        }
    }

    #[test]
    fn alt_is_meta() {
        let ascii_printable = ' '..='~';
        for character in ascii_printable {
            assert_eq!(
                to_esc_str(
                    &Keystroke::parse(&format!("alt-{}", character)).unwrap(),
                    Modes::NONE,
                    true
                )
                .unwrap(),
                format!("\x1b{}", character)
            );
        }

        let gpui_keys = [
            "up", "down", "right", "left", "f1", "f2", "f3", "f4", "F5", "f6", "f7", "f8", "f9",
            "f10", "f11", "f12", "f13", "f14", "f15", "f16", "f17", "f18", "f19", "f20", "insert",
            "pageup", "pagedown", "end", "home",
        ];

        for key in gpui_keys {
            assert_ne!(
                to_esc_str(
                    &Keystroke::parse(&format!("alt-{}", key)).unwrap(),
                    Modes::NONE,
                    true
                )
                .unwrap(),
                format!("\x1b{}", key)
            );
        }
    }

    #[test]
    fn test_shift_enter_newline() {
        let shift_enter = Keystroke::parse("shift-enter").unwrap();
        let regular_enter = Keystroke::parse("enter").unwrap();
        let mode = Modes::NONE;

        // Shift-enter should send line feed (newline)
        assert_eq!(to_esc_str(&shift_enter, mode, false), Some("\x0a".into()));

        // Regular enter should still send carriage return
        assert_eq!(to_esc_str(&regular_enter, mode, false), Some("\x0d".into()));
    }

    /// Permanent contract suite on the encoding path (SPEC.md §4.3): same
    /// keystroke + modes ⇒ byte-identical output to the alacritty-era
    /// `to_esc_str`. On Linux this exercises the ghostty `key::Encoder`
    /// behind the `Modes` shim; elsewhere the legacy path, so the
    /// expectations hold everywhere by construction.
    mod contract {
        use super::*;

        #[track_caller]
        fn assert_bytes(keystroke: &str, mode: Modes, expected: &[u8]) {
            let keystroke = Keystroke::parse(keystroke).unwrap();
            let actual = encode_keystroke(&keystroke, mode, false);
            assert_eq!(
                actual.as_deref(),
                Some(expected),
                "keystroke {keystroke} in mode {mode:?}: got {:?}, expected {:?}",
                actual.as_deref().map(String::from_utf8_lossy),
                String::from_utf8_lossy(expected),
            );
        }

        #[track_caller]
        fn assert_no_bytes(keystroke: &str, mode: Modes) {
            let keystroke = Keystroke::parse(keystroke).unwrap();
            let actual = encode_keystroke(&keystroke, mode, false);
            assert_eq!(
                actual, None,
                "keystroke {keystroke} in mode {mode:?} should fall through to the text path",
            );
        }

        #[test]
        fn special_keys() {
            let none = Modes::NONE;
            assert_bytes("tab", none, b"\x09");
            assert_bytes("shift-tab", none, b"\x1b[Z");
            assert_bytes("escape", none, b"\x1b");
            assert_bytes("enter", none, b"\x0d");
            assert_bytes("shift-enter", none, b"\x0a");
            assert_bytes("alt-enter", none, b"\x1b\x0d");
            assert_bytes("backspace", none, b"\x7f");
            assert_bytes("ctrl-backspace", none, b"\x08");
            assert_bytes("alt-backspace", none, b"\x1b\x7f");
            assert_bytes("shift-backspace", none, b"\x7f");
            assert_bytes("ctrl-space", none, b"\x00");
            assert_bytes("insert", none, b"\x1b[2~");
            assert_bytes("delete", none, b"\x1b[3~");
            assert_bytes("pageup", none, b"\x1b[5~");
            assert_bytes("pagedown", none, b"\x1b[6~");
        }

        #[test]
        fn application_modes() {
            let none = Modes::NONE;
            let app_cursor = Modes::APP_CURSOR;

            for (key, csi, ss3) in [
                ("up", b"\x1b[A".as_slice(), b"\x1bOA".as_slice()),
                ("down", b"\x1b[B", b"\x1bOB"),
                ("right", b"\x1b[C", b"\x1bOC"),
                ("left", b"\x1b[D", b"\x1bOD"),
                ("home", b"\x1b[H", b"\x1bOH"),
                ("end", b"\x1b[F", b"\x1bOF"),
            ] {
                assert_bytes(key, none, csi);
                assert_bytes(key, app_cursor, ss3);
            }

            assert_bytes("shift-up", none, b"\x1b[1;2A");
            assert_bytes("shift-down", none, b"\x1b[1;2B");
            assert_bytes("shift-home", none, b"\x1b[1;2H");
            assert_bytes("shift-end", none, b"\x1b[1;2F");
            // Modified keys ignore application cursor mode.
            assert_bytes("shift-up", app_cursor, b"\x1b[1;2A");
        }

        #[test]
        fn modifier_codes() {
            for (keystroke, code) in [
                ("shift-up", 2),
                ("alt-up", 3),
                ("shift-alt-up", 4),
                ("ctrl-up", 5),
                ("shift-ctrl-up", 6),
                ("alt-ctrl-up", 7),
                ("shift-alt-ctrl-up", 8),
            ] {
                assert_bytes(keystroke, Modes::NONE, format!("\x1b[1;{code}A").as_bytes());
            }
        }

        #[test]
        fn function_keys() {
            let none = Modes::NONE;
            for (key, plain) in [
                ("f1", "\x1bOP"),
                ("f2", "\x1bOQ"),
                ("f3", "\x1bOR"),
                ("f4", "\x1bOS"),
                ("f5", "\x1b[15~"),
                ("f6", "\x1b[17~"),
                ("f7", "\x1b[18~"),
                ("f8", "\x1b[19~"),
                ("f9", "\x1b[20~"),
                ("f10", "\x1b[21~"),
                ("f11", "\x1b[23~"),
                ("f12", "\x1b[24~"),
                ("f13", "\x1b[25~"),
                ("f14", "\x1b[26~"),
                ("f15", "\x1b[28~"),
                ("f16", "\x1b[29~"),
                ("f17", "\x1b[31~"),
                ("f18", "\x1b[32~"),
                ("f19", "\x1b[33~"),
                ("f20", "\x1b[34~"),
            ] {
                assert_bytes(key, none, plain.as_bytes());
            }

            assert_bytes("shift-f1", none, b"\x1b[1;2P");
            assert_bytes("ctrl-f2", none, b"\x1b[1;5Q");
            // Modified F3 is a ledgered divergence (P3-008), tested below.
            assert_bytes("shift-f4", none, b"\x1b[1;2S");
            assert_bytes("ctrl-f6", none, b"\x1b[17;5~");
            assert_bytes("shift-f12", none, b"\x1b[24;2~");
            assert_bytes("alt-f13", none, b"\x1b[25;3~");
            assert_bytes("ctrl-f20", none, b"\x1b[34;5~");
            assert_bytes("shift-insert", none, b"\x1b[2;2~");
            assert_bytes("ctrl-pageup", none, b"\x1b[5;5~");
            assert_bytes("shift-pagedown", none, b"\x1b[6;2~");
        }

        #[test]
        fn ctrl_caret_codes() {
            let none = Modes::NONE;
            for (index, letter) in ('a'..='z').enumerate() {
                let byte = [index as u8 + 1];
                assert_bytes(&format!("ctrl-{letter}"), none, &byte);
                // ctrl-shift-{letter} is a ledgered divergence (P3-010),
                // tested below: GPUI's canonical keystroke is lowercase key +
                // shift, which the legacy uppercase CtrlShift rows never
                // matched, so the alacritty-era path encoded nothing.
            }
            assert_bytes("ctrl-@", none, b"\x00");
            assert_bytes("ctrl-[", none, b"\x1b");
            assert_bytes("ctrl-\\", none, b"\x1c");
            assert_bytes("ctrl-]", none, b"\x1d");
            assert_bytes("ctrl-^", none, b"\x1e");
            assert_bytes("ctrl-_", none, b"\x1f");
            assert_bytes("ctrl-?", none, b"\x7f");
        }

        #[test]
        fn alt_is_meta() {
            for character in '!'..='~' {
                assert_bytes(
                    &format!("alt-{character}"),
                    Modes::NONE,
                    format!("\x1b{character}").as_bytes(),
                );
            }
            assert_bytes("alt-shift-a", Modes::NONE, b"\x1bA");
        }

        #[test]
        fn text_path_fallthrough() {
            let none = Modes::NONE;
            // Plain and shift-modified text belongs to the IME/text path.
            assert_no_bytes("a", none);
            assert_no_bytes("shift-a", none);
            assert_no_bytes("space", none);
            assert_no_bytes("cmd-a", none);
            // Ctrl chords without a caret mapping keep falling through
            // (ctrl-alt chords double as AltGr on some layouts).
            assert_no_bytes("ctrl-1", none);
            assert_no_bytes("ctrl-;", none);
            assert_no_bytes("ctrl-alt-a", none);
            let multigrapheme = Keystroke {
                modifiers: gpui::Modifiers::default(),
                key: "🖖🏻".to_string(),
                key_char: None,
            };
            assert_eq!(encode_keystroke(&multigrapheme, none, false), None);
        }
    }

    /// Adjudicated behavior changes on the ghostty encoding path, recorded in
    /// docs/ghostty-migration/divergence-ledger.md. Linux-only: other
    /// platforms keep the alacritty-era behavior until their gates open.
    #[cfg(target_os = "linux")]
    mod adjudicated_divergences {
        use super::*;

        #[track_caller]
        fn assert_bytes(keystroke: &str, expected: &[u8]) {
            let keystroke = Keystroke::parse(keystroke).unwrap();
            let actual = encode_keystroke(&keystroke, Modes::NONE, false);
            assert_eq!(
                actual.as_deref(),
                Some(expected),
                "keystroke {keystroke}: got {:?}",
                actual.as_deref().map(String::from_utf8_lossy),
            );
        }

        // P3-003: alt + multi-character key names previously leaked the name
        // into the output ("\x1btab"); now properly encoded.
        #[test]
        fn alt_named_keys() {
            assert_bytes("alt-tab", b"\x1b\x09");
            assert_bytes("alt-escape", b"\x1b\x1b");
            assert_bytes("alt-space", b"\x1b ");
        }

        // P3-004: keys missing from the legacy modified-key table (the "F5"
        // typo, delete) previously fell through; now encoded.
        #[test]
        fn modified_key_table_gaps() {
            assert_bytes("shift-f5", b"\x1b[15;2~");
            assert_bytes("ctrl-f5", b"\x1b[15;5~");
            assert_bytes("shift-delete", b"\x1b[3;2~");
        }

        // P3-005: super-modified keys previously emitted a malformed
        // modifier code 1; now the kitty-style super encoding (8).
        #[test]
        fn super_modifier() {
            assert_bytes("cmd-up", b"\x1b[1;9A");
        }

        // P3-006: previously-silent combos now emit xterm "other keys"
        // (CSI 27) encodings.
        #[test]
        fn csi_27_combos() {
            assert_bytes("ctrl-enter", b"\x1b[27;5;13~");
            assert_bytes("ctrl-tab", b"\x1b[27;5;9~");
            assert_bytes("shift-escape", b"\x1b[27;2;27~");
        }

        // P3-008: modified F3 previously used `CSI 1;N R`, which collides
        // with the cursor position report; now xterm's modern `CSI 13;N~`.
        #[test]
        fn modified_f3() {
            assert_bytes("shift-f3", b"\x1b[13;2~");
            assert_bytes("alt-f3", b"\x1b[13;3~");
        }

        // P3-010: ctrl-shift-letter previously encoded nothing (the legacy
        // table's CtrlShift rows keyed on uppercase keys, which GPUI's
        // canonical lowercase-plus-shift keystrokes never matched); now the
        // caret code, matching ctrl-letter like every other terminal.
        #[test]
        fn ctrl_shift_letters() {
            for (index, letter) in ('a'..='z').enumerate() {
                let byte = [index as u8 + 1];
                let keystroke = format!("ctrl-shift-{letter}");
                let keystroke = Keystroke::parse(&keystroke).unwrap();
                assert_eq!(
                    encode_keystroke(&keystroke, Modes::NONE, false).as_deref(),
                    Some(byte.as_slice()),
                    "ctrl-shift-{letter}",
                );
            }
        }
    }

    #[test]
    fn test_modifier_code_calc() {
        //   Code     Modifiers
        // ---------+---------------------------
        //    2     | Shift
        //    3     | Alt
        //    4     | Shift + Alt
        //    5     | Control
        //    6     | Shift + Control
        //    7     | Alt + Control
        //    8     | Shift + Alt + Control
        // ---------+---------------------------
        // from: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h2-PC-Style-Function-Keys
        assert_eq!(2, modifier_code(&Keystroke::parse("shift-a").unwrap()));
        assert_eq!(3, modifier_code(&Keystroke::parse("alt-a").unwrap()));
        assert_eq!(4, modifier_code(&Keystroke::parse("shift-alt-a").unwrap()));
        assert_eq!(5, modifier_code(&Keystroke::parse("ctrl-a").unwrap()));
        assert_eq!(6, modifier_code(&Keystroke::parse("shift-ctrl-a").unwrap()));
        assert_eq!(7, modifier_code(&Keystroke::parse("alt-ctrl-a").unwrap()));
        assert_eq!(
            8,
            modifier_code(&Keystroke::parse("shift-ctrl-alt-a").unwrap())
        );
    }
}
