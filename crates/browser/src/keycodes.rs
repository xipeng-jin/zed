//! Key-name → Windows virtual-key mapping, ported from
//! `Glass:crates/browser/src/keycodes.rs`. CEF expects Chromium/Windows
//! virtual key codes on every platform for `send_key_event` in windowless
//! mode. Glass's macOS hardware-keycode table is not ported: it depended on a
//! fork-only `Keystroke.native_key_code` field, and gpui key names alone are
//! sufficient (migration plan §7 M1 step 7).

pub(crate) fn key_name_to_windows_vk(key: &str) -> i32 {
    match key {
        "backspace" => 0x08,
        "tab" => 0x09,
        "enter" => 0x0D,
        "shift" => 0x10,
        "control" => 0x11,
        "alt" => 0x12,
        "escape" => 0x1B,
        "space" => 0x20,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "end" => 0x23,
        "home" => 0x24,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "insert" => 0x2D,
        "delete" => 0x2E,
        "0" => 0x30,
        "1" => 0x31,
        "2" => 0x32,
        "3" => 0x33,
        "4" => 0x34,
        "5" => 0x35,
        "6" => 0x36,
        "7" => 0x37,
        "8" => 0x38,
        "9" => 0x39,
        "f1" => 0x70,
        "f2" => 0x71,
        "f3" => 0x72,
        "f4" => 0x73,
        "f5" => 0x74,
        "f6" => 0x75,
        "f7" => 0x76,
        "f8" => 0x77,
        "f9" => 0x78,
        "f10" => 0x79,
        "f11" => 0x7A,
        "f12" => 0x7B,
        ";" => 0xBA,
        "=" => 0xBB,
        "," => 0xBC,
        "-" => 0xBD,
        "." => 0xBE,
        "/" => 0xBF,
        "`" => 0xC0,
        "[" => 0xDB,
        "\\" => 0xDC,
        "]" => 0xDD,
        "'" => 0xDE,
        // Unlike Glass, only single-character keys fall through to the
        // letter mapping: Glass matched on the first character, which sent
        // multi-character names like "dead-acute" as a letter VK.
        _ => {
            let mut chars = key.chars();
            match (chars.next(), chars.next()) {
                (Some(character), None) if character.is_ascii_alphabetic() => {
                    character.to_ascii_uppercase() as i32
                }
                _ => 0,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::key_name_to_windows_vk;

    #[test]
    fn letters_map_to_uppercase_ascii_vk() {
        assert_eq!(key_name_to_windows_vk("a"), 0x41);
        assert_eq!(key_name_to_windows_vk("z"), 0x5A);
    }

    #[test]
    fn named_keys_map_to_their_vk() {
        assert_eq!(key_name_to_windows_vk("enter"), 0x0D);
        assert_eq!(key_name_to_windows_vk("backspace"), 0x08);
        assert_eq!(key_name_to_windows_vk("left"), 0x25);
        assert_eq!(key_name_to_windows_vk("pagedown"), 0x22);
    }

    #[test]
    fn unknown_keys_map_to_zero() {
        assert_eq!(key_name_to_windows_vk("dead-acute"), 0);
        assert_eq!(key_name_to_windows_vk(""), 0);
    }
}
