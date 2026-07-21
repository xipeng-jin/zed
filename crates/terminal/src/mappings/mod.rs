pub(crate) mod colors;
pub(crate) mod focus;
pub(crate) mod keys;
pub(crate) mod mouse;
pub(crate) mod paste;

#[cfg(test)]
pub(crate) mod test_support {
    use crate::terminal_settings::{AlternateScroll, CursorShape};
    use crate::{Modes, PtyEvent, TerminalBackend, TerminalBounds};
    use futures::channel::mpsc::UnboundedReceiver;

    /// A backend whose live terminal state matches `mode`, driven there
    /// through the VT stream: the encoder contract tests express state as
    /// `Modes` flags, while encode-time option sync reads the live terminal
    /// (SPEC.md §4.3). The events receiver rides along so backend events
    /// have somewhere to go.
    pub(crate) fn backend_with_modes(
        mode: Modes,
    ) -> (TerminalBackend, UnboundedReceiver<PtyEvent>) {
        let (events_tx, events_rx) = futures::channel::mpsc::unbounded();
        let mut backend = TerminalBackend::new(
            100,
            CursorShape::default(),
            TerminalBounds::default(),
            events_tx,
            AlternateScroll::On,
        );
        backend.write(&mode_set_sequences(mode));
        (backend, events_rx)
    }

    fn mode_set_sequences(mode: Modes) -> Vec<u8> {
        let mut bytes = Vec::new();
        for (flag, sequence) in [
            (Modes::APP_CURSOR, b"\x1b[?1h".as_slice()),
            (Modes::APP_KEYPAD, b"\x1b="),
            (Modes::MOUSE_REPORT_CLICK, b"\x1b[?1000h"),
            (Modes::MOUSE_DRAG, b"\x1b[?1002h"),
            (Modes::MOUSE_MOTION, b"\x1b[?1003h"),
            (Modes::FOCUS_IN_OUT, b"\x1b[?1004h"),
            (Modes::UTF8_MOUSE, b"\x1b[?1005h"),
            (Modes::SGR_MOUSE, b"\x1b[?1006h"),
            (Modes::ALTERNATE_SCROLL, b"\x1b[?1007h"),
            (Modes::BRACKETED_PASTE, b"\x1b[?2004h"),
        ] {
            if mode.contains(flag) {
                bytes.extend_from_slice(sequence);
            }
        }
        bytes
    }
}
