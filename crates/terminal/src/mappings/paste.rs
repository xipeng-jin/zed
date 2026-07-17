//! Paste encoding for the PTY. Zed owns the policy (the caller gates on
//! `Modes::BRACKETED_PASTE`); on Linux ghostty's `paste::encode` owns the
//! bytes (SPEC.md §4.3), a verified superset of the alacritty-era
//! sanitization. macOS/Windows keep the alacritty-era path until their §8
//! platform gates open.

pub(crate) fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    #[cfg(target_os = "linux")]
    if let Some(bytes) = ghostty_encode_paste(text, bracketed) {
        return bytes;
    }

    legacy_encode_paste(text, bracketed)
}

#[cfg(target_os = "linux")]
fn ghostty_encode_paste(text: &str, bracketed: bool) -> Option<Vec<u8>> {
    use ghostty_vt::{Error, paste};

    // Ghostty's unbracketed encode maps `\n`→`\r` but leaves `\r` alone, so a
    // Windows-style `\r\n` would become `\r\r`; Zed's contract collapses it to
    // a single `\r`. Pre-normalizing `\r\n`→`\n` preserves that (#32
    // resolution's one contract check).
    let data = if bracketed {
        text.as_bytes().to_vec()
    } else {
        text.replace("\r\n", "\n").into_bytes()
    };

    let mut buf = vec![0u8; data.len() + 16];
    loop {
        // `paste::encode` sanitizes `data` in place, so give it a fresh copy
        // on the retry after an undersized buffer.
        let mut data = data.clone();
        match paste::encode(&mut data, bracketed, &mut buf) {
            Ok(len) => {
                buf.truncate(len);
                return Some(buf);
            }
            Err(Error::OutOfSpace { required }) if required > buf.len() => {
                buf = vec![0u8; required];
            }
            Err(error) => {
                log::error!("ghostty paste encoding failed: {error}");
                return None;
            }
        }
    }
}

// Alacritty-era path: production on macOS/Windows, fallback on Linux for
// encoder failure. Deleted at P10.
fn legacy_encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        format!("{}{}{}", "\x1b[200~", text.replace('\x1b', ""), "\x1b[201~").into_bytes()
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Permanent contract suite (SPEC.md §4.3): expectations are the
    /// alacritty-era bytes and hold on every platform.
    #[test]
    fn unbracketed_newline_normalization() {
        assert_eq!(encode_paste("hello", false), b"hello");
        assert_eq!(encode_paste("a\nb", false), b"a\rb");
        // The #32 resolution's contract check: `\r\n` collapses to a single
        // `\r`, never `\r\r`.
        assert_eq!(encode_paste("a\r\nb", false), b"a\rb");
        assert_eq!(encode_paste("a\rb", false), b"a\rb");
        assert_eq!(encode_paste("a\r\n\nb", false), b"a\r\rb");
    }

    #[test]
    fn bracketed_wrapping() {
        assert_eq!(encode_paste("hello", true), b"\x1b[200~hello\x1b[201~");
        // Newlines pass through untouched when bracketed.
        assert_eq!(encode_paste("a\r\nb", true), b"\x1b[200~a\r\nb\x1b[201~");
        assert_eq!(encode_paste("a\tb", true), b"\x1b[200~a\tb\x1b[201~");
    }

    /// Adjudicated divergence P3-007 (divergence ledger): ghostty's paste
    /// sanitization is a superset of the alacritty-era behavior — ESC (and
    /// NUL/DEL) become spaces instead of ESC being removed (bracketed) or
    /// passed through (unbracketed).
    #[cfg(target_os = "linux")]
    #[test]
    fn ghostty_sanitization_superset() {
        assert_eq!(encode_paste("a\x1bb", false), b"a b");
        assert_eq!(encode_paste("a\x1bb", true), b"\x1b[200~a b\x1b[201~");
        assert_eq!(
            encode_paste("a\x00b\x7fc", true),
            b"\x1b[200~a b c\x1b[201~"
        );
        // A bracketed-paste-end injection is defused by the ESC replacement.
        assert_eq!(
            encode_paste("a\x1b[201~b", true),
            b"\x1b[200~a [201~b\x1b[201~"
        );
    }
}
