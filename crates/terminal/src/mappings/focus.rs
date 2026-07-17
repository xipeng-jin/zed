//! Focus-report encoding (DEC mode 1004). Zed owns the policy (the caller
//! gates on `Modes::FOCUS_IN_OUT`); on Linux ghostty's focus helper owns the
//! bytes (SPEC.md §4.3). macOS/Windows keep the alacritty-era literals until
//! their §8 platform gates open.

pub(crate) fn focus_report(focused: bool) -> Vec<u8> {
    #[cfg(target_os = "linux")]
    {
        use ghostty_vt::focus;
        use util::ResultExt as _;

        let event = if focused {
            focus::Event::Gained
        } else {
            focus::Event::Lost
        };
        let mut buf = [0u8; 8];
        if let Some(len) = event.encode(&mut buf).log_err() {
            return buf[..len].to_vec();
        }
    }

    // Alacritty-era literals: production on macOS/Windows, fallback on Linux
    // for encoder failure. Deleted at P10.
    if focused {
        b"\x1b[I".to_vec()
    } else {
        b"\x1b[O".to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_reports() {
        assert_eq!(focus_report(true), b"\x1b[I");
        assert_eq!(focus_report(false), b"\x1b[O");
    }
}
