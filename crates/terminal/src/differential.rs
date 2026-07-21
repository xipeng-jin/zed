//! The P7 differential harness (SPEC.md §6 P7, §7; decision record
//! docs/ghostty-migration/verification-strategy.md §2–§3).
//!
//! Identical raw-byte transcripts — recorded real sessions plus
//! per-parity-matrix-row synthetic sequences, with first-class resize events
//! — feed both backends inside this one test binary, and seam-level
//! snapshots are compared after every event. Parity is "Zed cannot tell the
//! difference": the comparison surface is exactly what Zed consumes through
//! the seam (the `Content` snapshot, the probe set, and the event-derived
//! state the `terminal.rs` glue folds into PTY writes).
//!
//! Divergences must be adjudicated in
//! docs/ghostty-migration/divergence-ledger.md; an accepted entry is carried
//! here as a [`Waiver`] naming its ledger id. A waiver that stops firing
//! fails the run, so stale adjudications are flagged.
//!
//! The perf-baseline scenarios (verification-strategy §6) reuse the
//! transcript-feeding machinery against the alacritty backend only and run
//! on release builds via `script/terminal-perf-baseline`.
//!
//! Selection and display-offset appear in the probe (they are part of the
//! §2 surface and would catch one backend spontaneously growing either),
//! but transcripts drive only bytes and resizes, so their *semantics* are
//! pinned by the backend-to-backend differential tests that landed with
//! P5/P6 (`ghostty::tests`), not by this corpus.

use crate::TerminalBounds;
use gpui::{Bounds, px, size};

/// A transcript is an ordered stream of raw PTY output chunks and resize
/// events (verification-strategy §3.1). Binary fixtures use the ZTRX v1
/// format written by `script/terminal-record-transcript`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TranscriptEvent {
    Bytes(Vec<u8>),
    Resize { columns: u16, rows: u16 },
}

const TRANSCRIPT_MAGIC: &[u8; 5] = b"ZTRX\x01";
const TRANSCRIPT_TAG_BYTES: u8 = 0x01;
const TRANSCRIPT_TAG_RESIZE: u8 = 0x02;

pub(crate) fn decode_transcript(data: &[u8]) -> Result<Vec<TranscriptEvent>, String> {
    let payload = data
        .strip_prefix(TRANSCRIPT_MAGIC.as_slice())
        .ok_or_else(|| "not a ZTRX v1 transcript (bad magic)".to_string())?;
    let mut events = Vec::new();
    let mut rest = payload;
    while let Some((&tag, after_tag)) = rest.split_first() {
        match tag {
            TRANSCRIPT_TAG_BYTES => {
                let (length_bytes, after_length) = after_tag
                    .split_first_chunk::<4>()
                    .ok_or_else(|| "truncated bytes-event length".to_string())?;
                let length = u32::from_le_bytes(*length_bytes) as usize;
                if after_length.len() < length {
                    return Err("truncated bytes-event payload".to_string());
                }
                let (chunk, after_chunk) = after_length.split_at(length);
                events.push(TranscriptEvent::Bytes(chunk.to_vec()));
                rest = after_chunk;
            }
            TRANSCRIPT_TAG_RESIZE => {
                let (size_bytes, after_size) = after_tag
                    .split_first_chunk::<4>()
                    .ok_or_else(|| "truncated resize event".to_string())?;
                let columns = u16::from_le_bytes([size_bytes[0], size_bytes[1]]);
                let rows = u16::from_le_bytes([size_bytes[2], size_bytes[3]]);
                events.push(TranscriptEvent::Resize { columns, rows });
                rest = after_size;
            }
            other => return Err(format!("unknown transcript event tag {other:#x}")),
        }
    }
    Ok(events)
}

pub(crate) fn encode_transcript(events: &[TranscriptEvent]) -> Vec<u8> {
    let mut data = TRANSCRIPT_MAGIC.to_vec();
    for event in events {
        match event {
            TranscriptEvent::Bytes(chunk) => {
                data.push(TRANSCRIPT_TAG_BYTES);
                data.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
                data.extend_from_slice(chunk);
            }
            TranscriptEvent::Resize { columns, rows } => {
                data.push(TRANSCRIPT_TAG_RESIZE);
                data.extend_from_slice(&columns.to_le_bytes());
                data.extend_from_slice(&rows.to_le_bytes());
            }
        }
    }
    data
}

/// Builder for synthetic transcripts, keeping corpus entries readable.
#[derive(Clone, Debug, Default)]
pub(crate) struct Transcript(Vec<TranscriptEvent>);

impl Transcript {
    pub(crate) fn new() -> Self {
        Self(Vec::new())
    }

    pub(crate) fn bytes(mut self, chunk: impl AsRef<[u8]>) -> Self {
        self.0
            .push(TranscriptEvent::Bytes(chunk.as_ref().to_vec()));
        self
    }

    pub(crate) fn resize(mut self, columns: u16, rows: u16) -> Self {
        self.0.push(TranscriptEvent::Resize { columns, rows });
        self
    }

    pub(crate) fn events(self) -> Vec<TranscriptEvent> {
        self.0
    }
}

#[test]
fn transcript_codec_round_trips() {
    let events = Transcript::new()
        .resize(80, 24)
        .bytes(b"plain \x1b[31mred\x1b[0m")
        .resize(120, 40)
        .bytes([0x00, 0xff, 0x1b])
        .events();
    let encoded = encode_transcript(&events);
    assert_eq!(
        decode_transcript(&encoded).expect("round trip decodes"),
        events
    );
    assert!(decode_transcript(b"BOGUS").is_err());
    assert!(decode_transcript(&encoded[..encoded.len() - 1]).is_err());
}

pub(crate) fn harness_bounds(columns: usize, screen_lines: usize) -> TerminalBounds {
    TerminalBounds::new(
        px(16.),
        px(8.),
        Bounds {
            origin: Default::default(),
            size: size(px(8. * columns as f32), px(16. * screen_lines as f32)),
        },
    )
}

/// The differential comparison itself needs both backends in one binary,
/// which exists only where the ghostty crates compile (SPEC.md §5).
#[cfg(target_os = "linux")]
mod harness {
    use super::{TranscriptEvent, harness_bounds};
    use crate::terminal_settings::{AlternateScroll, CursorShape as SettingsCursorShape};
    use crate::{
        Cell, CellFlags, Color, Content, Cursor, Modes, Point, PtyEvent, Rgb, SelectionRange,
        TerminalBackendEvent, TerminalBounds, alacritty, ghostty,
    };
    use futures::channel::mpsc::UnboundedReceiver;
    use std::fmt::Write as _;

    /// The default scrollback for corpus runs: large enough that no recorded
    /// or synthetic transcript trims history, keeping the page-granular
    /// scrollback cap (ledger P5-001) out of every row that is not
    /// specifically about it.
    pub(crate) const CORPUS_SCROLLBACK: usize = 10_000;

    pub(crate) const DEFAULT_COLUMNS: u16 = 80;
    pub(crate) const DEFAULT_ROWS: u16 = 24;

    /// The embedder theme both sides answer color queries from: pushed into
    /// ghostty as its defaults, and used as the theme fallback in the
    /// alacritty `ColorRequest` glue, mirroring how `terminal.rs` answers
    /// with `get_color_at_index` when the emulator has no override.
    pub(crate) fn harness_theme_color(index: usize) -> Rgb {
        match index {
            0..=255 => Rgb {
                r: index as u8,
                g: (index as u8).wrapping_mul(3).wrapping_add(7),
                b: (index as u8) ^ 0x5A,
            },
            256 => Rgb {
                r: 0xd0,
                g: 0xd0,
                b: 0xd0,
            },
            257 => Rgb {
                r: 0x10,
                g: 0x10,
                b: 0x18,
            },
            258 => Rgb {
                r: 0xff,
                g: 0x80,
                b: 0x00,
            },
            _ => Rgb { r: 0, g: 0, b: 0 },
        }
    }

    fn harness_palette() -> [Rgb; 256] {
        let mut palette = [Rgb { r: 0, g: 0, b: 0 }; 256];
        for (index, entry) in palette.iter_mut().enumerate() {
            *entry = harness_theme_color(index);
        }
        palette
    }

    /// The seam surface the harness drives and probes. Both backends expose
    /// this same inherent surface by construction (SPEC.md §4.1); the trait
    /// exists only so the harness can hold either side generically — it is
    /// test-local and not the production seam (which stays duck-typed).
    pub(crate) trait SeamBackend {
        fn write(&mut self, bytes: &[u8]);
        fn resize(&mut self, bounds: TerminalBounds);
        fn make_content(&mut self, last_content: &Content) -> Content;
        fn total_lines(&mut self) -> usize;
        fn content_text(&mut self) -> String;
        fn last_n_non_empty_lines(&mut self, line_count: usize) -> Vec<String>;
        fn cursor_blinking(&mut self) -> bool;
        fn color(&mut self, index: usize) -> Option<Rgb>;
    }

    /// Both backends expose the identical inherent surface by construction,
    /// so the delegation is mechanical (inherent methods win over the trait
    /// method inside the impl, so `Self::method` resolves to the backend's
    /// own).
    macro_rules! impl_seam_backend {
        ($backend:ty) => {
            impl SeamBackend for $backend {
                fn write(&mut self, bytes: &[u8]) {
                    Self::write(self, bytes);
                }
                fn resize(&mut self, bounds: TerminalBounds) {
                    Self::resize(self, bounds);
                }
                fn make_content(&mut self, last_content: &Content) -> Content {
                    Self::make_content(self, last_content)
                }
                fn total_lines(&mut self) -> usize {
                    Self::total_lines(self)
                }
                fn content_text(&mut self) -> String {
                    Self::content_text(self)
                }
                fn last_n_non_empty_lines(&mut self, line_count: usize) -> Vec<String> {
                    Self::last_n_non_empty_lines(self, line_count)
                }
                fn cursor_blinking(&mut self) -> bool {
                    Self::cursor_blinking(self)
                }
                fn color(&mut self, index: usize) -> Option<Rgb> {
                    Self::color(self, index)
                }
            }
        };
    }

    impl_seam_backend!(alacritty::TerminalBackend);
    impl_seam_backend!(ghostty::TerminalBackend);

    /// One cell of the comparison surface: exactly the accessors Zed's
    /// renderer reads (verification-strategy §2). The OSC 8 `id` is excluded
    /// by decision (SPEC.md §4.2 G7): hover equality compares URIs.
    #[derive(Clone, Debug, PartialEq)]
    pub(crate) struct CellProbe {
        character: char,
        foreground: Color,
        background: Color,
        flags: CellFlags,
        zerowidth: Vec<char>,
        hyperlink_uri: Option<String>,
    }

    fn cell_probe(cell: &Cell) -> CellProbe {
        CellProbe {
            character: cell.character(),
            foreground: normalize_color(cell.foreground()),
            background: normalize_color(cell.background()),
            flags: cell.flags,
            zerowidth: cell.zerowidth().unwrap_or_default().to_vec(),
            hyperlink_uri: cell.hyperlink().map(|hyperlink| hyperlink.uri().to_string()),
        }
    }

    /// `Indexed(0–15)` and the equivalent `Named` color render identically in
    /// Zed (`terminal_element::convert_color` routes both through the theme's
    /// 16 ANSI colors), and ghostty collapses SGR 31 into `Palette(1)` — an
    /// accepted information loss (SPEC.md §9). Normalize before comparing so
    /// the harness measures what Zed can observe.
    fn normalize_color(color: Color) -> Color {
        match color {
            Color::Indexed(index @ 0..=15) => Color::Named(ghostty::named_ansi_color(index)),
            other => other,
        }
    }

    /// The full seam-level snapshot compared after each transcript event
    /// (verification-strategy §2): the `Content` fields Zed consumes, the
    /// fixed probe set, and the event-derived state.
    #[derive(Clone, Debug, PartialEq)]
    pub(crate) struct Probe {
        cells: Vec<(Point, CellProbe)>,
        mode: Modes,
        display_offset: usize,
        cursor: Cursor,
        cursor_char: char,
        selection: Option<SelectionRange>,
        selection_text: Option<String>,
        scrolled_to_top: bool,
        scrolled_to_bottom: bool,
        bottom_row_occupied: bool,
        total_lines: usize,
        cursor_blinking: bool,
        /// Filled on full probes only (they walk the whole grid).
        content_text: Option<String>,
        /// Filled on full probes only.
        last_non_empty_lines: Option<Vec<String>>,
        title: String,
        bells: usize,
        clipboard_stores: Vec<String>,
        pty_writes: String,
    }

    /// The probe fields, for naming divergences and scoping waivers.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum ProbeField {
        Cells,
        Mode,
        DisplayOffset,
        Cursor,
        CursorChar,
        Selection,
        SelectionText,
        ScrolledToTop,
        ScrolledToBottom,
        BottomRowOccupied,
        TotalLines,
        CursorBlinking,
        ContentText,
        LastNonEmptyLines,
        Title,
        Bells,
        ClipboardStores,
        PtyWrites,
    }

    /// A ledger-adjudicated divergence this corpus entry is allowed to show
    /// (verification-strategy §3.4). Every waiver must fire at least once per
    /// run — a waiver that stops matching is stale and fails the entry. The
    /// optional check pins the adjudicated shape of the divergence, and the
    /// optional step range bounds how much of the transcript the waiver can
    /// absorb (essential for check-less waivers over recorded fixtures).
    pub(crate) struct Waiver {
        pub ledger: &'static str,
        pub field: ProbeField,
        pub check: Option<fn(&Probe, &Probe) -> bool>,
        pub steps: Option<std::ops::Range<usize>>,
    }

    impl Waiver {
        pub(crate) fn new(ledger: &'static str, field: ProbeField) -> Self {
            Self {
                ledger,
                field,
                check: None,
                steps: None,
            }
        }

        pub(crate) fn checked(
            ledger: &'static str,
            field: ProbeField,
            check: fn(&Probe, &Probe) -> bool,
        ) -> Self {
            Self {
                ledger,
                field,
                check: Some(check),
                steps: None,
            }
        }

        pub(crate) fn stepped(mut self, steps: std::ops::Range<usize>) -> Self {
            self.steps = Some(steps);
            self
        }
    }

    pub(crate) struct Divergence {
        step: usize,
        event: String,
        field: ProbeField,
        detail: String,
        alacritty_probe: Probe,
        ghostty_probe: Probe,
    }

    /// Check functions pinning the shape of ledger-adjudicated divergences.
    pub(crate) mod checks {
        use super::Probe;
        use crate::Modes;

        fn trimmed_text(probe: &Probe) -> Option<&str> {
            probe
                .content_text
                .as_deref()
                .map(|text| text.trim_end_matches('\n'))
        }

        /// The full extracted text is identical once trailing blank-row
        /// newlines are stripped — content survived, only row accounting
        /// differs.
        pub(crate) fn text_preserved(alacritty: &Probe, ghostty: &Probe) -> bool {
            match (trimmed_text(alacritty), trimmed_text(ghostty)) {
                (Some(alacritty_text), Some(ghostty_text)) => alacritty_text == ghostty_text,
                _ => false,
            }
        }

        /// Ghostty's text is the tail of alacritty's — alacritty rotated
        /// extra rows into scrollback that ghostty discarded.
        pub(crate) fn ghostty_text_is_suffix(alacritty: &Probe, ghostty: &Probe) -> bool {
            match (trimmed_text(alacritty), trimmed_text(ghostty)) {
                (Some(alacritty_text), Some(ghostty_text)) => {
                    alacritty_text.ends_with(ghostty_text)
                }
                _ => false,
            }
        }

        pub(crate) fn ghostty_lines_are_suffix(alacritty: &Probe, ghostty: &Probe) -> bool {
            match (&alacritty.last_non_empty_lines, &ghostty.last_non_empty_lines) {
                (Some(alacritty_lines), Some(ghostty_lines)) => {
                    alacritty_lines.ends_with(ghostty_lines)
                }
                _ => false,
            }
        }

        /// Alacritty's history grew where ghostty's did not.
        pub(crate) fn alacritty_grew_history(alacritty: &Probe, ghostty: &Probe) -> bool {
            ghostty.total_lines < alacritty.total_lines
        }

        /// Ghostty retained more trimmed scrollback than alacritty's exact
        /// line cap — the page-granular limit of ledger P5-001.
        pub(crate) fn ghostty_retains_more_history(alacritty: &Probe, ghostty: &Probe) -> bool {
            ghostty.total_lines > alacritty.total_lines
        }

        /// Alacritty's text is the tail of ghostty's — ghostty kept rows
        /// alacritty's exact-count trim discarded (ledger P5-001).
        pub(crate) fn alacritty_text_is_suffix(alacritty: &Probe, ghostty: &Probe) -> bool {
            match (trimmed_text(alacritty), trimmed_text(ghostty)) {
                (Some(alacritty_text), Some(ghostty_text)) => {
                    ghostty_text.ends_with(alacritty_text)
                }
                _ => false,
            }
        }

        /// `scrolled_to_top` disagrees only because alacritty has scrollback
        /// the adjudication already covers.
        pub(crate) fn ghostty_scrolled_to_top(alacritty: &Probe, ghostty: &Probe) -> bool {
            !alacritty.scrolled_to_top && ghostty.scrolled_to_top
        }

        /// Ghostty homed the cursor to column 0 on the same line (DEC IL/DL
        /// semantics); alacritty left the column untouched.
        pub(crate) fn ghostty_homed_cursor_column(alacritty: &Probe, ghostty: &Probe) -> bool {
            ghostty.cursor.point.column == 0
                && alacritty.cursor.point.line == ghostty.cursor.point.line
                && alacritty.cursor.shape == ghostty.cursor.shape
        }

        pub(crate) fn cursor_columns_equal(alacritty: &Probe, ghostty: &Probe) -> bool {
            alacritty.cursor.point.column == ghostty.cursor.point.column
                && alacritty.cursor.shape == ghostty.cursor.shape
        }

        /// Every differing cell is alacritty holding the `'\t'` a tab left in
        /// its origin cell where ghostty holds a blank.
        pub(crate) fn cells_differ_only_by_tab(alacritty: &Probe, ghostty: &Probe) -> bool {
            alacritty.cells.len() == ghostty.cells.len()
                && alacritty.cells.iter().zip(ghostty.cells.iter()).all(
                    |((alacritty_point, alacritty_cell), (ghostty_point, ghostty_cell))| {
                        if (alacritty_point, alacritty_cell) == (ghostty_point, ghostty_cell) {
                            return true;
                        }
                        alacritty_point == ghostty_point
                            && alacritty_cell.character == '\t'
                            && ghostty_cell.character == ' '
                            && alacritty_cell.foreground == ghostty_cell.foreground
                            && alacritty_cell.background == ghostty_cell.background
                            && alacritty_cell.flags == ghostty_cell.flags
                            && alacritty_cell.zerowidth == ghostty_cell.zerowidth
                            && alacritty_cell.hyperlink_uri == ghostty_cell.hyperlink_uri
                    },
                )
        }

        /// The modes agree except that ghostty keeps mouse protocol/encoding
        /// flags independently set where alacritty's are mutually exclusive —
        /// ghostty's set is a superset of alacritty's.
        pub(crate) fn ghostty_mouse_flags_superset(alacritty: &Probe, ghostty: &Probe) -> bool {
            let mouse_mask = Modes::MOUSE_MODE | Modes::UTF8_MOUSE | Modes::SGR_MOUSE;
            let mut alacritty_rest = alacritty.mode;
            alacritty_rest.remove(mouse_mask);
            let mut ghostty_rest = ghostty.mode;
            ghostty_rest.remove(mouse_mask);
            alacritty_rest == ghostty_rest
                && ghostty.mode.contains(alacritty_mouse_bits(alacritty.mode))
        }

        fn alacritty_mouse_bits(mode: Modes) -> Modes {
            let mut bits = Modes::empty();
            for flag in [
                Modes::MOUSE_REPORT_CLICK,
                Modes::MOUSE_DRAG,
                Modes::MOUSE_MOTION,
                Modes::UTF8_MOUSE,
                Modes::SGR_MOUSE,
            ] {
                if mode.contains(flag) {
                    bits.insert(flag);
                }
            }
            bits
        }

        /// Ghostty's extra PTY writes are exactly kitty keyboard query
        /// responses (`ESC [ ? <flags> u`); alacritty stays silent with the
        /// kitty protocol disabled.
        pub(crate) fn ghostty_extra_is_kitty_report(alacritty: &Probe, ghostty: &Probe) -> bool {
            alacritty.pty_writes.is_empty() && is_kitty_reports(&ghostty.pty_writes)
        }

        fn is_kitty_reports(mut writes: &str) -> bool {
            if writes.is_empty() {
                return false;
            }
            while let Some(rest) = writes.strip_prefix("\x1b[?") {
                let Some(end) = rest.find('u') else { return false };
                if rest[..end].is_empty() || !rest[..end].bytes().all(|byte| byte.is_ascii_digit())
                {
                    return false;
                }
                writes = &rest[end + 1..];
            }
            writes.is_empty()
        }

        /// Ghostty's PTY writes are alacritty's with extra responses to
        /// queries the alacritty-era core ignored interleaved: the
        /// color-scheme report (`CSI ? 997 ; s n`) and the XTVERSION reply
        /// (`DCS > | libghostty ST`).
        pub(crate) fn ghostty_extra_is_ignored_query_response(
            alacritty: &Probe,
            ghostty: &Probe,
        ) -> bool {
            const KNOWN_EXTRAS: [&str; 3] = [
                "\x1b[?997;1n",
                "\x1b[?997;2n",
                "\x1bP>|libghostty\x1b\\",
            ];
            let mut remaining_alacritty = alacritty.pty_writes.as_str();
            let mut remaining_ghostty = ghostty.pty_writes.as_str();
            let mut saw_extra = false;
            'outer: while !remaining_ghostty.is_empty() {
                for extra in KNOWN_EXTRAS {
                    if let Some(rest) = remaining_ghostty.strip_prefix(extra) {
                        remaining_ghostty = rest;
                        saw_extra = true;
                        continue 'outer;
                    }
                }
                match remaining_alacritty.chars().next() {
                    Some(character) if remaining_ghostty.starts_with(character) => {
                        remaining_alacritty = &remaining_alacritty[character.len_utf8()..];
                        remaining_ghostty = &remaining_ghostty[character.len_utf8()..];
                    }
                    _ => return false,
                }
            }
            remaining_alacritty.is_empty() && saw_extra
        }

        /// Every differing cell is an erased blank where alacritty stamped
        /// the active SGR attribute flags (inverse/bold/…) into the fill and
        /// ghostty erased attribute-free (xterm semantics).
        pub(crate) fn erased_cells_drop_attribute_flags(
            alacritty: &Probe,
            ghostty: &Probe,
        ) -> bool {
            alacritty.cells.len() == ghostty.cells.len()
                && alacritty.cells.iter().zip(ghostty.cells.iter()).all(
                    |((alacritty_point, alacritty_cell), (ghostty_point, ghostty_cell))| {
                        if (alacritty_point, alacritty_cell) == (ghostty_point, ghostty_cell) {
                            return true;
                        }
                        alacritty_point == ghostty_point
                            && alacritty_cell.character == ghostty_cell.character
                            && alacritty_cell.foreground == ghostty_cell.foreground
                            && alacritty_cell.background == ghostty_cell.background
                            && ghostty_cell.flags == crate::CellFlags::empty()
                            && alacritty_cell.flags != crate::CellFlags::empty()
                            && alacritty_cell.zerowidth == ghostty_cell.zerowidth
                            && alacritty_cell.hyperlink_uri == ghostty_cell.hyperlink_uri
                    },
                )
        }

        /// Both cursors are hidden — the position delta is invisible to Zed's
        /// renderer.
        pub(crate) fn cursors_hidden(alacritty: &Probe, ghostty: &Probe) -> bool {
            alacritty.cursor.shape == crate::CursorShape::Hidden
                && ghostty.cursor.shape == crate::CursorShape::Hidden
        }

        /// Alacritty's extra PTY writes are exactly ANSI-mode DECRPM reports
        /// (`ESC [ <mode> ; <state> $ y`), which ghostty leaves unanswered.
        pub(crate) fn alacritty_extra_is_ansi_decrpm(alacritty: &Probe, ghostty: &Probe) -> bool {
            let Some(mut extra) = alacritty.pty_writes.strip_prefix(ghostty.pty_writes.as_str())
            else {
                return false;
            };
            if extra.is_empty() {
                return false;
            }
            while let Some(rest) = extra.strip_prefix("\x1b[") {
                let Some(end) = rest.find("$y") else { return false };
                if !rest[..end]
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b';')
                {
                    return false;
                }
                extra = &rest[end + 2..];
            }
            extra.is_empty()
        }
    }

    struct HarnessSide<Backend> {
        backend: Backend,
        events_rx: UnboundedReceiver<PtyEvent>,
        bounds: TerminalBounds,
        last_content: Content,
        /// Mirrors `Terminal::breadcrumb_text`: `Title` events assign it and
        /// `ResetTitle` empties it, so an empty-title OSC and a reset are the
        /// same observable state, exactly as in the `terminal.rs` glue.
        breadcrumb_text: String,
        bells: usize,
        clipboard_stores: Vec<String>,
        pty_writes: String,
    }

    impl<Backend: SeamBackend> HarnessSide<Backend> {
        fn apply(&mut self, event: &TranscriptEvent) {
            match event {
                TranscriptEvent::Bytes(chunk) => self.backend.write(chunk),
                TranscriptEvent::Resize { columns, rows } => {
                    self.bounds = harness_bounds(*columns as usize, *rows as usize);
                    self.backend.resize(self.bounds);
                }
            }
            self.pump_events();
        }

        /// The event glue, mirroring `Terminal::process_event` in
        /// `terminal.rs`: query-type events fold into PTY writes in event
        /// order (the ordering the alacritty-era comment there calls out),
        /// with the harness theme standing in for the app theme.
        fn pump_events(&mut self) {
            while let Ok(PtyEvent::Event(event)) = self.events_rx.try_recv() {
                match event {
                    TerminalBackendEvent::Title(title) => self.breadcrumb_text = title,
                    TerminalBackendEvent::ResetTitle => self.breadcrumb_text = String::new(),
                    TerminalBackendEvent::ClipboardStore(data) => {
                        self.clipboard_stores.push(data)
                    }
                    TerminalBackendEvent::ClipboardLoad(format) => {
                        // The harness clipboard is empty, matching the glue's
                        // no-clipboard fallback.
                        self.pty_writes.push_str(&format(""));
                    }
                    TerminalBackendEvent::PtyWrite(output) => self.pty_writes.push_str(&output),
                    TerminalBackendEvent::TextAreaSizeRequest(format) => {
                        let response = format(self.bounds);
                        self.pty_writes.push_str(&response);
                    }
                    TerminalBackendEvent::ColorRequest(index, format) => {
                        let color = self
                            .backend
                            .color(index)
                            .unwrap_or_else(|| harness_theme_color(index));
                        self.pty_writes.push_str(&format(color));
                    }
                    TerminalBackendEvent::Bell => self.bells += 1,
                    TerminalBackendEvent::MouseCursorDirty
                    | TerminalBackendEvent::CursorBlinkingChange
                    | TerminalBackendEvent::Wakeup
                    | TerminalBackendEvent::Exit
                    | TerminalBackendEvent::ChildExit(_) => {}
                }
            }
        }

        fn probe(&mut self, full: bool) -> Probe {
            let content = self.backend.make_content(&self.last_content);
            self.last_content = content.clone();
            // Snapshot building can emit derived events (blink/mouse-mode
            // diffs); fold them before reading the event-derived state.
            self.pump_events();

            Probe {
                cells: content
                    .cells
                    .iter()
                    .map(|cell| (cell.point, cell_probe(&cell.cell)))
                    .collect(),
                mode: content.mode,
                display_offset: content.display_offset,
                cursor: content.cursor,
                cursor_char: content.cursor_char,
                selection: content.selection,
                selection_text: content.selection_text.clone(),
                scrolled_to_top: content.scrolled_to_top,
                scrolled_to_bottom: content.scrolled_to_bottom,
                bottom_row_occupied: content.bottom_row_occupied,
                total_lines: self.backend.total_lines(),
                cursor_blinking: self.backend.cursor_blinking(),
                content_text: full.then(|| self.backend.content_text()),
                last_non_empty_lines: full.then(|| self.backend.last_n_non_empty_lines(10)),
                title: self.breadcrumb_text.clone(),
                bells: self.bells,
                clipboard_stores: self.clipboard_stores.clone(),
                pty_writes: self.pty_writes.clone(),
            }
        }
    }

    fn new_side<Backend: SeamBackend>(
        construct: impl FnOnce(
            futures::channel::mpsc::UnboundedSender<PtyEvent>,
            TerminalBounds,
        ) -> Backend,
    ) -> HarnessSide<Backend> {
        let (events_tx, events_rx) = futures::channel::mpsc::unbounded();
        let bounds = harness_bounds(DEFAULT_COLUMNS as usize, DEFAULT_ROWS as usize);
        let backend = construct(events_tx, bounds);
        HarnessSide {
            backend,
            events_rx,
            bounds,
            last_content: Content::default(),
            breadcrumb_text: String::new(),
            bells: 0,
            clipboard_stores: Vec::new(),
            pty_writes: String::new(),
        }
    }

    fn compare(
        step: usize,
        event: &TranscriptEvent,
        alacritty_probe: &Probe,
        ghostty_probe: &Probe,
        divergences: &mut Vec<Divergence>,
    ) {
        let event_description = describe_event(event);
        let mut push = |field: ProbeField, detail: String| {
            divergences.push(Divergence {
                step,
                event: event_description.clone(),
                field,
                detail,
                alacritty_probe: alacritty_probe.clone(),
                ghostty_probe: ghostty_probe.clone(),
            });
        };

        if alacritty_probe.cells != ghostty_probe.cells {
            push(
                ProbeField::Cells,
                describe_cell_divergence(alacritty_probe, ghostty_probe),
            );
        }

        macro_rules! compare_field {
            ($field:ident, $variant:ident) => {
                if alacritty_probe.$field != ghostty_probe.$field {
                    push(
                        ProbeField::$variant,
                        format!(
                            "alacritty: {:?}\n      ghostty:   {:?}",
                            alacritty_probe.$field, ghostty_probe.$field
                        ),
                    );
                }
            };
        }

        compare_field!(mode, Mode);
        compare_field!(display_offset, DisplayOffset);
        compare_field!(cursor, Cursor);
        compare_field!(cursor_char, CursorChar);
        compare_field!(selection, Selection);
        compare_field!(selection_text, SelectionText);
        compare_field!(scrolled_to_top, ScrolledToTop);
        compare_field!(scrolled_to_bottom, ScrolledToBottom);
        compare_field!(bottom_row_occupied, BottomRowOccupied);
        compare_field!(total_lines, TotalLines);
        compare_field!(cursor_blinking, CursorBlinking);
        compare_field!(content_text, ContentText);
        compare_field!(last_non_empty_lines, LastNonEmptyLines);
        compare_field!(title, Title);
        compare_field!(bells, Bells);
        compare_field!(clipboard_stores, ClipboardStores);
        compare_field!(pty_writes, PtyWrites);
    }

    fn describe_event(event: &TranscriptEvent) -> String {
        match event {
            TranscriptEvent::Bytes(chunk) => {
                let text: String = String::from_utf8_lossy(chunk)
                    .chars()
                    .take(80)
                    .map(|character| {
                        if character.is_control() {
                            char::from_u32(0x2400 + character as u32).unwrap_or('.')
                        } else {
                            character
                        }
                    })
                    .collect();
                format!("bytes({} B: {text}…)", chunk.len())
            }
            TranscriptEvent::Resize { columns, rows } => format!("resize({columns}×{rows})"),
        }
    }

    fn describe_cell_divergence(alacritty_probe: &Probe, ghostty_probe: &Probe) -> String {
        let mut detail = String::new();
        if alacritty_probe.cells.len() != ghostty_probe.cells.len() {
            let _ = writeln!(
                detail,
                "cell count: alacritty {} vs ghostty {}",
                alacritty_probe.cells.len(),
                ghostty_probe.cells.len()
            );
        }
        let mut shown = 0;
        for ((alacritty_point, alacritty_cell), (ghostty_point, ghostty_cell)) in alacritty_probe
            .cells
            .iter()
            .zip(ghostty_probe.cells.iter())
        {
            if (alacritty_point, alacritty_cell) != (ghostty_point, ghostty_cell) {
                let _ = writeln!(
                    detail,
                    "  at {alacritty_point:?}/{ghostty_point:?}:\n    alacritty: {alacritty_cell:?}\n    ghostty:   {ghostty_cell:?}"
                );
                shown += 1;
                if shown >= 6 {
                    detail.push_str("  … (further cell differences elided)\n");
                    break;
                }
            }
        }
        detail
    }

    /// Feed one transcript to both backends and compare seam snapshots after
    /// every event, panicking on unadjudicated divergences and on waivers
    /// that never fire. `full_probe_interval` bounds how often the
    /// whole-grid text probes run (they are O(total lines) each); the final
    /// event always gets a full probe.
    pub(crate) fn run_transcript(
        name: &str,
        events: &[TranscriptEvent],
        waivers: &[Waiver],
        full_probe_interval: usize,
    ) {
        let divergences = collect_divergences(events, full_probe_interval, CORPUS_SCROLLBACK);
        if let Some(failure) = adjudicate(name, divergences, waivers, true) {
            panic!("{failure}");
        }
    }

    /// As [`run_transcript`], with an explicit scrollback limit — for the
    /// trim rows, which need history small enough to overflow.
    pub(crate) fn run_transcript_with_scrollback(
        name: &str,
        events: &[TranscriptEvent],
        waivers: &[Waiver],
        full_probe_interval: usize,
        scrollback: usize,
    ) {
        let divergences = collect_divergences(events, full_probe_interval, scrollback);
        if let Some(failure) = adjudicate(name, divergences, waivers, true) {
            panic!("{failure}");
        }
    }

    /// The fuzz-lane entry: same comparison, but waivers are corpus-wide
    /// adjudications that need not fire, and the failure is returned for the
    /// caller to aggregate instead of panicking per case.
    pub(crate) fn transcript_failure(
        name: &str,
        events: &[TranscriptEvent],
        waivers: &[Waiver],
        full_probe_interval: usize,
    ) -> Option<String> {
        let divergences = collect_divergences(events, full_probe_interval, CORPUS_SCROLLBACK);
        adjudicate(name, divergences, waivers, false)
    }

    fn collect_divergences(
        events: &[TranscriptEvent],
        full_probe_interval: usize,
        scrollback: usize,
    ) -> Vec<Divergence> {
        let mut alacritty_side = new_side(|events_tx, bounds| {
            alacritty::TerminalBackend::new(
                scrollback,
                SettingsCursorShape::Block,
                bounds,
                events_tx,
                AlternateScroll::On,
            )
        });
        let mut ghostty_side = new_side(|events_tx, bounds| {
            let mut backend = ghostty::TerminalBackend::new(
                scrollback,
                SettingsCursorShape::Block,
                bounds,
                events_tx,
                AlternateScroll::On,
            );
            backend.push_theme_colors(
                harness_theme_color(256),
                harness_theme_color(257),
                harness_theme_color(258),
                &harness_palette(),
            );
            backend
        });

        let mut divergences = Vec::new();
        for (step, event) in events.iter().enumerate() {
            alacritty_side.apply(event);
            ghostty_side.apply(event);

            let full =
                step == events.len() - 1 || (full_probe_interval > 0 && step % full_probe_interval == 0);
            let alacritty_probe = alacritty_side.probe(full);
            let ghostty_probe = ghostty_side.probe(full);
            compare(step, event, &alacritty_probe, &ghostty_probe, &mut divergences);
        }

        divergences
    }

    fn adjudicate(
        name: &str,
        divergences: Vec<Divergence>,
        waivers: &[Waiver],
        require_waivers_fire: bool,
    ) -> Option<String> {
        let mut waiver_fired = vec![false; waivers.len()];
        let mut unadjudicated = Vec::new();

        for divergence in &divergences {
            let mut waived = false;
            for (index, waiver) in waivers.iter().enumerate() {
                let step_in_range = waiver
                    .steps
                    .as_ref()
                    .is_none_or(|steps| steps.contains(&divergence.step));
                let check_holds = waiver.check.is_none_or(|check| {
                    check(&divergence.alacritty_probe, &divergence.ghostty_probe)
                });
                if waiver.field == divergence.field && step_in_range && check_holds {
                    waiver_fired[index] = true;
                    waived = true;
                }
            }
            if !waived {
                unadjudicated.push(divergence);
            }
        }

        let mut failure = String::new();
        if !unadjudicated.is_empty() {
            let _ = writeln!(
                failure,
                "transcript `{name}`: {} unadjudicated divergence(s) — every divergence needs a docs/ghostty-migration/divergence-ledger.md entry:",
                unadjudicated.len()
            );
            for divergence in unadjudicated.iter().take(10) {
                let _ = writeln!(
                    failure,
                    "  step {} [{}] {:?}:\n      {}",
                    divergence.step, divergence.event, divergence.field, divergence.detail
                );
            }
            if unadjudicated.len() > 10 {
                let _ = writeln!(failure, "  … ({} more)", unadjudicated.len() - 10);
            }
        }
        if require_waivers_fire {
            for (index, waiver) in waivers.iter().enumerate() {
                if !waiver_fired[index] {
                    let _ = writeln!(
                        failure,
                        "transcript `{name}`: waiver {} ({:?}) never fired — stale adjudication?",
                        waiver.ledger, waiver.field
                    );
                }
            }
        }
        (!failure.is_empty()).then_some(failure)
    }
}

#[cfg(target_os = "linux")]
mod corpus {
    //! The gating synthetic corpus (verification-strategy §3.2): targeted,
    //! esctest/vttest-inspired sequences scoped to the parity-matrix rows —
    //! each entry names the matrix area it verifies. The corpus is the parity
    //! matrix's executable form.

    use super::Transcript;
    use super::harness::{
        ProbeField, Waiver, checks, run_transcript, run_transcript_with_scrollback,
    };

    fn run(name: &str, transcript: Transcript) {
        run_transcript(name, &transcript.events(), &[], 1);
    }

    fn run_with_waivers(name: &str, transcript: Transcript, waivers: &[Waiver]) {
        run_transcript(name, &transcript.events(), waivers, 1);
    }

    // ── SGR permutations (matrix §B grid/cell read path, §M colors) ──

    #[test]
    fn sgr_basic_attributes() {
        run(
            "sgr_basic_attributes",
            Transcript::new().bytes(
                "\x1b[1mbold\x1b[22m \x1b[2mdim\x1b[22m \x1b[3mitalic\x1b[23m \
                 \x1b[4munder\x1b[24m \x1b[4:2mdouble\x1b[4:0m \x1b[4:3mcurly\x1b[4:0m \
                 \x1b[4:4mdotted\x1b[4:0m \x1b[4:5mdashed\x1b[4:0m \x1b[7minverse\x1b[27m \
                 \x1b[9mstrike\x1b[29m \x1b[0mreset\r\n\x1b[1;3;4;7mstacked\x1b[m done\r\n",
            ),
        );
    }

    #[test]
    fn sgr_16_colors() {
        let mut bytes = String::new();
        for code in 30..=37 {
            bytes.push_str(&format!("\x1b[{code}mF{code}\x1b[39m "));
        }
        bytes.push_str("\r\n");
        for code in 40..=47 {
            bytes.push_str(&format!("\x1b[{code}mB{code}\x1b[49m "));
        }
        bytes.push_str("\r\n");
        for code in 90..=97 {
            bytes.push_str(&format!("\x1b[{code}mF{code}\x1b[39m "));
        }
        bytes.push_str("\r\n");
        for code in 100..=107 {
            bytes.push_str(&format!("\x1b[{code}mB{code}\x1b[49m "));
        }
        bytes.push_str("\r\n");
        run("sgr_16_colors", Transcript::new().bytes(bytes));
    }

    #[test]
    fn sgr_256_palette() {
        let mut bytes = String::new();
        for index in 0..=255u16 {
            bytes.push_str(&format!("\x1b[38;5;{index}mx\x1b[39m"));
            if index % 64 == 63 {
                bytes.push_str("\r\n");
            }
        }
        for index in (0..=255u16).step_by(17) {
            bytes.push_str(&format!("\x1b[48;5;{index}m \x1b[49m"));
        }
        bytes.push_str("\r\n");
        run("sgr_256_palette", Transcript::new().bytes(bytes));
    }

    #[test]
    fn sgr_truecolor() {
        run(
            "sgr_truecolor",
            Transcript::new().bytes(
                "\x1b[38;2;12;34;56mfg\x1b[39m \x1b[48;2;250;128;3mbg\x1b[49m \
                 \x1b[38;2;0;0;0;48;2;255;255;255mboth\x1b[m\r\n\
                 \x1b[38:2:1:2:3mcolonform\x1b[m\r\n",
            ),
        );
    }

    /// SPEC.md §9 accepted information loss: SGR 31 and SGR 38;5;1 both
    /// surface as red; the comparator normalizes `Indexed(0–15)` to `Named`
    /// because Zed renders them identically.
    #[test]
    fn sgr_named_vs_indexed_collapse() {
        run(
            "sgr_named_vs_indexed_collapse",
            Transcript::new()
                .bytes("\x1b[31mnamed-red\x1b[39m \x1b[38;5;1mindexed-red\x1b[39m\r\n"),
        );
    }

    // ── Cursor movement + scroll regions (matrix §E, §L) ──

    #[test]
    fn cursor_movement_basic() {
        run(
            "cursor_movement_basic",
            Transcript::new()
                .bytes("line1\r\nline2\r\nline3\r\n")
                .bytes("\x1b[Hhome\x1b[2;5Hat25\x1b[2A\x1b[3B\x1b[4C\x1b[2Dmoved")
                .bytes("\x1b[Gcol1\x1b[10Gcol10\rback\x08\x08\ttabbed\r\n"),
        );
    }

    #[test]
    fn scroll_region_decstbm() {
        run_with_waivers(
            "scroll_region_decstbm",
            Transcript::new()
                .bytes("top\r\nr1\r\nr2\r\nr3\r\nbottom\r\n")
                .bytes("\x1b[2;4r")
                .bytes("\x1b[2;1Hinside\r\n\x1b[3S\x1b[2T")
                .bytes("\x1b[2;1H\x1b[2Linserted\x1b[1M")
                .bytes("\x1b[r\x1b[6;1Hafter-reset\r\n"),
            &[Waiver::checked("P7-004", ProbeField::Cursor, checks::ghostty_homed_cursor_column)],
        );
    }

    #[test]
    fn cursor_save_restore() {
        run(
            "cursor_save_restore",
            Transcript::new()
                .bytes("abc\x1b7def\x1b[31m\x1b[5;5H*\x1b8ghi\r\n")
                .bytes("\x1b[s..\x1b[10;10H\x1b[umarker\r\n"),
        );
    }

    #[test]
    fn origin_mode() {
        run(
            "origin_mode",
            Transcript::new()
                .bytes("\x1b[3;10r\x1b[?6h\x1b[Horigin-top\r\n")
                .bytes("\x1b[2;2Hrelative\r\n\x1b[?6l\x1b[r\x1b[Habsolute\r\n"),
        );
    }

    #[test]
    fn index_and_reverse_index() {
        run(
            "index_and_reverse_index",
            Transcript::new()
                .bytes("first\r\nsecond\r\nthird")
                .bytes("\x1bM\x1bM up2 \x1bD down \x1bE nel\r\n")
                .bytes("\x1b[Hat-top\x1bM scrolled-back\r\n"),
        );
    }

    // ── Erase / insert / delete (matrix §B, §J) ──

    #[test]
    fn erase_operations() {
        run_with_waivers(
            "erase_operations",
            Transcript::new()
                .bytes("aaaaaaaaaa\r\nbbbbbbbbbb\r\ncccccccccc\r\ndddddddddd\x1b[2;5H")
                .bytes("\x1b[0K")
                .bytes("\x1b[3;5H\x1b[1K")
                .bytes("\x1b[4;5H\x1b[2K")
                .bytes("\x1b[1;1H\x1b[0J")
                .bytes("filler\r\nfiller\r\n\x1b[1;3H\x1b[1J")
                .bytes("\x1b[2J\x1b[Hcleared\r\n"),
            &[
                Waiver::checked("P7-003", ProbeField::TotalLines, checks::alacritty_grew_history),
                Waiver::checked("P7-003", ProbeField::ScrolledToTop, checks::ghostty_scrolled_to_top),
                Waiver::checked("P7-003", ProbeField::ContentText, checks::ghostty_text_is_suffix),
                Waiver::checked("P7-003", ProbeField::LastNonEmptyLines, checks::ghostty_lines_are_suffix),
            ],
        );
    }

    #[test]
    fn erase_scrollback_ed3() {
        run(
            "erase_scrollback_ed3",
            Transcript::new()
                .bytes("one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\neight\r\n")
                .resize(80, 4)
                .bytes("\x1b[3J")
                .bytes("after-clear\r\n"),
        );
    }

    #[test]
    fn insert_delete_chars_lines() {
        run_with_waivers(
            "insert_delete_chars_lines",
            Transcript::new()
                .bytes("0123456789\r\nabcdefghij\r\nklmnopqrst\x1b[1;3H")
                .bytes("\x1b[3@INS")
                .bytes("\x1b[1;1H\x1b[2P")
                .bytes("\x1b[2;4H\x1b[4X")
                .bytes("\x1b[1;1H\x1b[2L")
                .bytes("\x1b[1M\r\n"),
            &[
                Waiver::checked("P7-003", ProbeField::TotalLines, checks::alacritty_grew_history),
                Waiver::checked("P7-003", ProbeField::ScrolledToTop, checks::ghostty_scrolled_to_top),
                Waiver::checked("P7-003", ProbeField::ContentText, checks::ghostty_text_is_suffix),
            ],
        );
    }

    /// Tab-stop *positions* verify via the cursor probe; the cell and
    /// extraction deltas are the adjudicated tab-cell representation.
    #[test]
    fn tab_stops() {
        run_with_waivers(
            "tab_stops",
            Transcript::new()
                .bytes("\ta\tb\tc\r\n")
                .bytes("\x1b[5G\x1bH\x1b[1G\x1b[I after-cht\r\n")
                .bytes("x\t\x1b[Z\x1b[Z cbt\r\n")
                .bytes("\x1b[3g\ttabs-cleared\r\n"),
            &[
                Waiver::checked("P7-001", ProbeField::Cells, checks::cells_differ_only_by_tab),
                Waiver::new("P7-001", ProbeField::ContentText),
                Waiver::new("P7-001", ProbeField::LastNonEmptyLines),
            ],
        );
    }

    // ── Wide chars / graphemes at row boundaries (matrix §B) ──

    #[test]
    fn wide_chars_cjk() {
        run(
            "wide_chars_cjk",
            Transcript::new()
                .bytes("汉字宽度测试 mixed 漢字 かな カナ\r\n")
                .bytes("\x1b[31m赤い\x1b[39m plain\r\n"),
        );
    }

    #[test]
    fn wide_char_at_row_boundary() {
        run(
            "wide_char_at_row_boundary",
            Transcript::new()
                .resize(10, 6)
                .bytes("abcdefghi汉")
                .bytes("\r\nxxxxxxxxx字tail\r\n"),
        );
    }

    #[test]
    fn combining_marks_zerowidth() {
        run(
            "combining_marks_zerowidth",
            Transcript::new()
                .bytes("e\u{0301}combining a\u{0308}\u{0332}stacked\r\n")
                .bytes("plain\r\n"),
        );
    }

    // ── WRAPLINE / reflow across resizes (matrix §B, §D) ──

    #[test]
    fn wrap_pending_edge_cases() {
        run(
            "wrap_pending_edge_cases",
            Transcript::new()
                .resize(10, 6)
                .bytes("0123456789")
                .bytes("wrapped\r\n")
                .bytes("\x1b[?7l0123456789overflow\x1b[?7h\r\n")
                .bytes("0123456789\rX\r\n"),
        );
    }

    #[test]
    fn reflow_shrink_and_grow() {
        run_with_waivers(
            "reflow_shrink_and_grow",
            Transcript::new()
                .bytes("the quick brown fox jumps over the lazy dog again and again\r\nshort\r\n")
                .resize(40, 24)
                .resize(20, 24)
                .resize(60, 24)
                .bytes("after-reflow\r\n"),
            &[
                Waiver::checked("P7-005", ProbeField::Cells, checks::text_preserved),
                Waiver::checked("P7-005", ProbeField::Cursor, checks::cursor_columns_equal),
                Waiver::checked("P7-005", ProbeField::TotalLines, checks::alacritty_grew_history),
                Waiver::checked("P7-005", ProbeField::ScrolledToTop, checks::ghostty_scrolled_to_top),
                Waiver::checked("P7-005", ProbeField::ContentText, checks::text_preserved),
            ],
        );
    }

    #[test]
    fn reflow_rows_and_columns() {
        run(
            "reflow_rows_and_columns",
            Transcript::new()
                .resize(30, 10)
                .bytes("aaaa bbbb cccc dddd eeee ffff gggg hhhh\r\nsecond line content here\r\n")
                .resize(30, 5)
                .resize(45, 12)
                .bytes("done\r\n"),
        );
    }

    // ── Scrollback fill + trim (matrix §E) ──

    #[test]
    fn scrollback_fill_within_limit() {
        let mut transcript = Transcript::new().resize(80, 6);
        let mut bytes = String::new();
        for line in 0..200 {
            bytes.push_str(&format!("scrollback line {line}\r\n"));
        }
        transcript = transcript.bytes(bytes);
        run("scrollback_fill_within_limit", transcript);
    }

    /// Scrollback overflow past the configured limit: alacritty trims to the
    /// exact line count, ghostty's cap is page-granular and retains more
    /// (ledger P5-001). The viewport and the recent-line probes must still
    /// agree exactly.
    #[test]
    fn scrollback_trim_past_limit() {
        let mut bytes = String::new();
        for line in 0..1000 {
            bytes.push_str(&format!("trimmed history line {line}\r\n"));
        }
        run_transcript_with_scrollback(
            "scrollback_trim_past_limit",
            &Transcript::new().resize(80, 6).bytes(bytes).events(),
            &[
                Waiver::checked(
                    "P5-001",
                    ProbeField::TotalLines,
                    checks::ghostty_retains_more_history,
                ),
                Waiver::checked(
                    "P5-001",
                    ProbeField::ContentText,
                    checks::alacritty_text_is_suffix,
                ),
            ],
            1,
            100,
        );
    }

    // ── Alt screen (matrix §E, §K) ──

    #[test]
    fn alt_screen_enter_exit() {
        run(
            "alt_screen_enter_exit",
            Transcript::new()
                .bytes("primary content\r\nmore primary\r\n")
                .bytes("\x1b[?1049h")
                .bytes("\x1b[Halt content\x1b[31mcolored\x1b[m")
                .bytes("\x1b[?1049l")
                .bytes("back on primary\r\n"),
        );
    }

    /// The legacy alt-screen variants (?47 / ?1047), which Zed's alacritty
    /// fork ignores while ghostty implements the standard buffer swap
    /// (ledger P7-002).
    #[test]
    fn alt_screen_legacy_modes() {
        run_with_waivers(
            "alt_screen_legacy_modes",
            Transcript::new()
                .bytes("primary content\r\nmore primary\r\n")
                .bytes("\x1b[?47hlegacy alt\x1b[?47l")
                .bytes("\x1b[?1047h\x1b[Hlegacy2\x1b[?1047l\r\n"),
            &[
                Waiver::new("P7-002", ProbeField::Cells),
                Waiver::new("P7-002", ProbeField::ContentText),
                Waiver::new("P7-002", ProbeField::LastNonEmptyLines),
            ],
        );
    }

    #[test]
    fn alt_screen_resize() {
        run(
            "alt_screen_resize",
            Transcript::new()
                .bytes("primary before alt\r\n")
                .bytes("\x1b[?1049h\x1b[Halt-line-one\r\nalt-line-two")
                .resize(40, 12)
                .bytes("\x1b[?1049l")
                .resize(80, 24)
                .bytes("primary after\r\n"),
        );
    }

    // ── Modes Zed reads (matrix §K) ──

    #[test]
    fn modes_toggle_all() {
        run_with_waivers(
            "modes_toggle_all",
            Transcript::new()
                .bytes("\x1b[?1h\x1b[?1l")
                .bytes("\x1b=\x1b>")
                .bytes("\x1b[?25l\x1b[?25h")
                .bytes("\x1b[?7l\x1b[?7h")
                .bytes("\x1b[?6h\x1b[?6l")
                .bytes("\x1b[4h\x1b[4l")
                .bytes("\x1b[20h\x1b[20l")
                .bytes("\x1b[?1004h\x1b[?1004l")
                .bytes("\x1b[?1007h\x1b[?1007l")
                .bytes("\x1b[?2004h\x1b[?2004l")
                .bytes("\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1005h\x1b[?1006h")
                .bytes("\x1b[?1006l\x1b[?1005l\x1b[?1003l\x1b[?1002l\x1b[?1000l")
                .bytes("modes done\r\n"),
            &[Waiver::checked("P7-006", ProbeField::Mode, checks::ghostty_mouse_flags_superset)],
        );
    }

    /// Mouse protocol downgrades and encoding swaps, where alacritty's
    /// last-set-wins exclusivity and ghostty's independent flags visibly part
    /// (ledger P7-006).
    #[test]
    fn mouse_mode_exclusivity() {
        run_with_waivers(
            "mouse_mode_exclusivity",
            Transcript::new()
                .bytes("\x1b[?1003h\x1b[?1000h")
                .bytes("\x1b[?1006h\x1b[?1005h")
                .bytes("\x1b[?1000l\x1b[?1005l")
                .bytes("\x1b[?1003h\x1b[?1003l")
                .bytes("mouse done\r\n"),
            &[Waiver::checked("P7-006", ProbeField::Mode, checks::ghostty_mouse_flags_superset)],
        );
    }

    #[test]
    fn modes_left_enabled() {
        run(
            "modes_left_enabled",
            Transcript::new()
                .bytes("\x1b[?1h\x1b[?25l\x1b[4h\x1b[?1004h\x1b[?2004h\x1b[?1002h\x1b[?1006h")
                .bytes("sticky modes\r\n"),
        );
    }

    // ── Cursor styles (matrix §L) ──

    #[test]
    fn cursor_style_decscusr() {
        run(
            "cursor_style_decscusr",
            Transcript::new()
                .bytes("\x1b[1 qblink-block ")
                .bytes("\x1b[2 qsteady-block ")
                .bytes("\x1b[3 qblink-under ")
                .bytes("\x1b[4 qsteady-under ")
                .bytes("\x1b[5 qblink-bar ")
                .bytes("\x1b[6 qsteady-bar ")
                .bytes("\x1b[0 qdefault\r\n"),
        );
    }

    // ── OSC handling (matrix §M, §N) ──

    #[test]
    fn osc_title() {
        run(
            "osc_title",
            Transcript::new()
                .bytes("\x1b]0;window title\x07titled\r\n")
                .bytes("\x1b]2;second title\x1b\\more\r\n")
                .bytes("\x1b]0;\x07reset\r\n"),
        );
    }

    #[test]
    fn osc_7_working_directory() {
        // Zed reads the working directory from the PTY layer
        // (`pty_info`), not the emulator; the row pins that both cores
        // swallow OSC 7 without observable effect.
        run(
            "osc_7_working_directory",
            Transcript::new()
                .bytes("\x1b]7;file://localhost/tmp/somewhere\x07after-osc7\r\n")
                .bytes("\x1b]7;file:///home/user/project\x1b\\done\r\n"),
        );
    }

    #[test]
    fn osc_8_hyperlinks() {
        run(
            "osc_8_hyperlinks",
            Transcript::new()
                .bytes("\x1b]8;;https://zed.dev\x07zed\x1b]8;;\x07 plain ")
                .bytes("\x1b]8;id=a;https://example.com\x07one\x1b]8;;\x07\r\n")
                .bytes("\x1b]8;id=x;https://same.uri\x07left\x1b]8;id=y;https://same.uri\x07right\x1b]8;;\x07\r\n"),
        );
    }

    #[test]
    fn osc_52_clipboard() {
        run(
            "osc_52_clipboard",
            Transcript::new()
                // "hello" base64; the write surfaces as ClipboardStore.
                .bytes("\x1b]52;c;aGVsbG8=\x07written\r\n")
                // OSC 52 read is disabled on both cores (SPEC.md §9).
                .bytes("\x1b]52;c;?\x07queried\r\n"),
        );
    }

    #[test]
    fn osc_133_prompt_marks() {
        run(
            "osc_133_prompt_marks",
            Transcript::new()
                .bytes("\x1b]133;A\x07prompt> \x1b]133;B\x07ls -la\r\n")
                .bytes("\x1b]133;C\x07output line\r\n\x1b]133;D;0\x07")
                .bytes("\x1b]133;A\x07prompt> \r\n"),
        );
    }

    #[test]
    fn osc_color_set_and_query() {
        run(
            "osc_color_set_and_query",
            Transcript::new()
                .bytes("\x1b]4;1;#aabbcc\x07")
                .bytes("\x1b]4;1;?\x07")
                .bytes("\x1b]10;?\x07\x1b]11;?\x07\x1b]12;?\x07")
                .bytes("\x1b]10;#112233\x07\x1b]10;?\x07")
                .bytes("\x1b]11;#445566\x07\x1b]11;?\x07")
                .bytes("\x1b]12;#778899\x07\x1b]12;?\x07")
                .bytes("\x1b]104;1\x07\x1b]4;1;?\x07")
                .bytes("\x1b]110\x07\x1b]10;?\x07")
                .bytes("\x1b]111\x07\x1b]11;?\x07")
                .bytes("\x1b]112\x07\x1b]12;?\x07")
                .bytes("colors done\r\n"),
        );
    }

    // ── Charsets and chunk-split sequences (matrix §Q) ──

    #[test]
    fn charset_dec_special() {
        run(
            "charset_dec_special",
            Transcript::new()
                .bytes("\x1b(0lqqqk\x1b(B ascii \x0e\x1b(0x\x0f done\r\n"),
        );
    }

    #[test]
    fn utf8_split_across_chunks() {
        let encoded = "汉".as_bytes();
        run(
            "utf8_split_across_chunks",
            Transcript::new()
                .bytes(&encoded[..1])
                .bytes(&encoded[1..2])
                .bytes(&encoded[2..])
                .bytes(" split-utf8\r\n"),
        );
    }

    #[test]
    fn escape_split_across_chunks() {
        run(
            "escape_split_across_chunks",
            Transcript::new()
                .bytes("\x1b")
                .bytes("[3")
                .bytes("1mred")
                .bytes("\x1b[")
                .bytes("39m plain\r\n"),
        );
    }

    /// Background-color-erase: EL/ED fill with the current SGR background.
    #[test]
    fn bce_erase_with_background() {
        run(
            "bce_erase_with_background",
            Transcript::new()
                .bytes("\x1b[42m\x1b[K green-bg-erase\r\n")
                .bytes("\x1b[0m\x1b[44mtext\x1b[K\r\n")
                .bytes("\x1b[0m\x1b[45m\x1b[2;10H\x1b[1K\r\n")
                .bytes("\x1b[0mdone\r\n"),
        );
    }

    // ── Bell (matrix §N) ──

    #[test]
    fn bell() {
        run(
            "bell",
            Transcript::new().bytes("ding\x07dong\x07\r\n"),
        );
    }

    // ── Reports and queries (matrix §N) ──

    #[test]
    fn device_status_reports() {
        run(
            "device_status_reports",
            Transcript::new()
                .bytes("\x1b[5n")
                .bytes("\x1b[3;7H\x1b[6n")
                .bytes("\r\n"),
        );
    }

    #[test]
    fn device_attributes() {
        run(
            "device_attributes",
            Transcript::new().bytes("\x1b[c").bytes("\x1b[0c").bytes("\r\n"),
        );
    }

    /// Secondary DA (`CSI > c`), which vim sends on startup (t_RV).
    #[test]
    fn secondary_device_attributes() {
        run(
            "secondary_device_attributes",
            Transcript::new().bytes("\x1b[>c").bytes("\x1b[>0c").bytes("\r\n"),
        );
    }

    #[test]
    fn xtwinops_size_reports() {
        run(
            "xtwinops_size_reports",
            Transcript::new().bytes("\x1b[14t").bytes("\x1b[18t").bytes("\r\n"),
        );
    }

    #[test]
    fn decrqm_private_mode_reports() {
        run(
            "decrqm_private_mode_reports",
            Transcript::new()
                .bytes("\x1b[?2004h\x1b[?2004$p")
                .bytes("\x1b[?2004l\x1b[?2004$p")
                .bytes("\x1b[?25$p")
                .bytes("\r\n"),
        );
    }

    /// ANSI-mode DECRQM (`CSI Ps $ p`), which alacritty answers and ghostty
    /// leaves unanswered (ledger P7-007).
    #[test]
    fn decrqm_ansi_mode_reports() {
        run_with_waivers(
            "decrqm_ansi_mode_reports",
            Transcript::new()
                .bytes("\x1b[4$p")
                .bytes("\x1b[4h\x1b[4$p\x1b[4l")
                .bytes("\r\n"),
            &[Waiver::checked("P7-007", ProbeField::PtyWrites, checks::alacritty_extra_is_ansi_decrpm)],
        );
    }

    // ── Recorded real-session transcripts (verification-strategy §3.2) ──
    //
    // Binary fixtures under `transcripts/recorded/`, captured with
    // `script/terminal-record-transcript`. They cover interleavings
    // hand-written sequences never produce; each file carries its own
    // waiver table for ledger-adjudicated divergences its content trips.
    //
    // Recording-machine substitutions for the §3.2 list, to refresh when
    // the real tools are at hand: classic `vi` stands in for vim/nvim,
    // `top` for htop, `cargo check --color always` for cargo/clippy
    // output, and the agent-tool run is the same command-batch shell
    // session the Zed agent terminal tool produces (colored ls/grep,
    // progress-line CR rewrites) rather than a capture from a live agent.

    fn run_recorded(name: &str, waivers: &[Waiver]) {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("transcripts/recorded")
            .join(name);
        let data = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let events = super::decode_transcript(&data)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));
        assert!(!events.is_empty(), "{name}: empty transcript");
        run_transcript(name, &events, waivers, 8);
    }

    // Waiver step bounds below index the checked-in fixtures' event streams;
    // re-recording a fixture means re-deriving them (a shifted bound fails
    // as either an unadjudicated divergence or a never-fired waiver).

    #[test]
    fn recorded_vi_editing_session() {
        // The 100×30 → 70×20 shrink (step 19) lands while vi holds the alt
        // screen; the pre-redraw state differs by resize anchoring (ledger
        // P7-015) and converges as soon as vi's own redraw bytes arrive.
        run_recorded(
            "vi_editing_session.ztrx",
            &[
                Waiver::new("P7-015", ProbeField::Cells).stepped(19..20),
                Waiver::new("P7-015", ProbeField::CursorChar).stepped(19..20),
            ],
        );
    }

    #[test]
    fn recorded_tmux_split_scroll() {
        // The 110×30 → 80×24 shrink (step 34) lands on tmux's alt screen;
        // the pre-redraw rows anchor differently (ledger P7-015) and
        // converge on tmux's redraw two steps later.
        run_recorded(
            "tmux_split_scroll.ztrx",
            &[
                Waiver::checked(
                    "P7-012",
                    ProbeField::PtyWrites,
                    checks::ghostty_extra_is_ignored_query_response,
                ),
                Waiver::new("P7-015", ProbeField::Cells).stepped(34..36),
                Waiver::checked("P7-015", ProbeField::Cursor, checks::cursor_columns_equal)
                    .stepped(34..36),
                Waiver::new("P7-015", ProbeField::CursorChar).stepped(35..36),
            ],
        );
    }

    #[test]
    fn recorded_top_process_viewer() {
        run_recorded(
            "top_process_viewer.ztrx",
            &[
                Waiver::checked("P7-013", ProbeField::Cells, checks::erased_cells_drop_attribute_flags),
                Waiver::checked("P7-014", ProbeField::Cursor, checks::cursors_hidden),
            ],
        );
    }

    #[test]
    fn recorded_less_paging() {
        run_recorded("less_paging.ztrx", &[]);
    }

    #[test]
    fn recorded_cargo_colored_output() {
        run_recorded("cargo_colored_output.ztrx", &[]);
    }

    #[test]
    fn recorded_agent_tool_shell() {
        run_recorded("agent_tool_shell.ztrx", &[]);
    }

    #[test]
    fn recorded_shell_osc133_prompt_marks() {
        run_recorded("shell_osc133_prompt_marks.ztrx", &[]);
    }

    /// The kitty keyboard progressive-enhancement query, which ghostty's
    /// core answers itself — the forced finding of the #32 input-encoding
    /// resolution (ledger P7-008).
    #[test]
    fn kitty_keyboard_query() {
        run_with_waivers(
            "kitty_keyboard_query",
            Transcript::new()
                .bytes("\x1b[?u")
                .bytes("\x1b[>1u\x1b[?u\x1b[<u")
                .bytes("\r\n"),
            &[Waiver::checked("P7-008", ProbeField::PtyWrites, checks::ghostty_extra_is_kitty_report)],
        );
    }
}

#[cfg(target_os = "linux")]
mod fuzz {
    //! The structured VT-sequence fuzzer (verification-strategy §3.3):
    //! valid-ish and malformed streams run differentially as a bounded,
    //! NON-GATING lane — `#[ignore]`d here, run by CI with
    //! `continue-on-error`. Confirmed real divergences get promoted into the
    //! synthetic corpus; legitimate parser disagreements are
    //! ledger-adjudicated. The lane applies the ledger's *checked*
    //! adjudications corpus-wide (they need not fire), so reported findings
    //! are new by construction.
    //!
    //! Reproduce a finding with
    //! `DIFFERENTIAL_FUZZ_SEED=<seed> DIFFERENTIAL_FUZZ_CASES=<n> cargo test
    //! -p terminal --lib -- --ignored differential::fuzz`.

    use super::TranscriptEvent;
    use super::harness::{ProbeField, Waiver, checks, transcript_failure};
    use rand::{Rng, SeedableRng, rngs::StdRng};

    /// Adjudications applied to every fuzz case. Checkless waivers are
    /// deliberately absent — they would blind the lane.
    fn fuzz_waivers() -> Vec<Waiver> {
        vec![
            Waiver::checked("P7-003", ProbeField::TotalLines, checks::alacritty_grew_history),
            Waiver::checked("P7-003", ProbeField::ScrolledToTop, checks::ghostty_scrolled_to_top),
            Waiver::checked("P7-003", ProbeField::ContentText, checks::ghostty_text_is_suffix),
            Waiver::checked("P7-003", ProbeField::LastNonEmptyLines, checks::ghostty_lines_are_suffix),
            Waiver::checked("P7-004", ProbeField::Cursor, checks::ghostty_homed_cursor_column),
            Waiver::checked("P7-005", ProbeField::Cells, checks::text_preserved),
            Waiver::checked("P7-005", ProbeField::ContentText, checks::text_preserved),
            Waiver::checked("P7-013", ProbeField::Cells, checks::erased_cells_drop_attribute_flags),
        ]
    }

    fn random_text(rng: &mut StdRng, output: &mut Vec<u8>) {
        let alphabets: [&[char]; 4] = [
            &['a', 'b', 'x', ' ', '!', '/', '~', '0', '9'],
            &['汉', '字', 'テ', 'ス', 'ト', '中'],
            &['é', 'ü', 'ß', 'π', 'Ω'],
            &['e', '\u{0301}', 'a', '\u{0308}'],
        ];
        let alphabet = alphabets[rng.random_range(0..alphabets.len())];
        let length = rng.random_range(1..40);
        let mut text = String::new();
        for _ in 0..length {
            text.push(alphabet[rng.random_range(0..alphabet.len())]);
        }
        output.extend_from_slice(text.as_bytes());
    }

    fn random_sgr(rng: &mut StdRng, output: &mut Vec<u8>) {
        let params: [&str; 12] = [
            "0", "1", "3", "4", "7", "9", "31", "42", "38;5;123", "48;5;200", "38;2;10;20;30",
            "4:3",
        ];
        let count = rng.random_range(1..4);
        let mut sequence = String::from("\x1b[");
        for index in 0..count {
            if index > 0 {
                sequence.push(';');
            }
            sequence.push_str(params[rng.random_range(0..params.len())]);
        }
        sequence.push('m');
        output.extend_from_slice(sequence.as_bytes());
    }

    fn random_cursor_op(rng: &mut StdRng, output: &mut Vec<u8>) {
        let row = rng.random_range(1..30);
        let column = rng.random_range(1..100);
        let count = rng.random_range(1..6);
        let ops: [String; 10] = [
            format!("\x1b[{row};{column}H"),
            format!("\x1b[{count}A"),
            format!("\x1b[{count}B"),
            format!("\x1b[{count}C"),
            format!("\x1b[{count}D"),
            format!("\x1b[{column}G"),
            "\r".to_string(),
            "\x1b7".to_string(),
            "\x1b8".to_string(),
            "\x1bM".to_string(),
        ];
        output.extend_from_slice(ops[rng.random_range(0..ops.len())].as_bytes());
    }

    fn random_edit_op(rng: &mut StdRng, output: &mut Vec<u8>) {
        let count = rng.random_range(1..5);
        let ops: [String; 9] = [
            format!("\x1b[{count}J"),
            format!("\x1b[{}K", rng.random_range(0..3)),
            format!("\x1b[{count}@"),
            format!("\x1b[{count}P"),
            format!("\x1b[{count}X"),
            format!("\x1b[{count}L"),
            format!("\x1b[{count}M"),
            format!("\x1b[{count}S"),
            format!("\x1b[{count}T"),
        ];
        // ED 3 erases scrollback; ED 0/1/2 all stay in the mix via n 1..5
        // (2J adjudicated via the P7-003 waivers).
        output.extend_from_slice(ops[rng.random_range(0..ops.len())].as_bytes());
    }

    fn random_region_or_mode(rng: &mut StdRng, output: &mut Vec<u8>) {
        let top = rng.random_range(1..12);
        let bottom = rng.random_range(top + 1..26);
        // Mouse modes, legacy alt-screen (?47/?1047), and the query
        // sequences ghostty answers differently are excluded: those
        // divergences are already ledger-adjudicated and would only re-fire.
        let ops: [String; 8] = [
            format!("\x1b[{top};{bottom}r"),
            "\x1b[r".to_string(),
            "\x1b[?6h".to_string(),
            "\x1b[?6l".to_string(),
            "\x1b[?7l".to_string(),
            "\x1b[?7h".to_string(),
            "\x1b[?25l".to_string(),
            "\x1b[?25h".to_string(),
        ];
        output.extend_from_slice(ops[rng.random_range(0..ops.len())].as_bytes());
    }

    fn random_malformed(rng: &mut StdRng, output: &mut Vec<u8>) {
        match rng.random_range(0..4) {
            0 => {
                // Truncated escape sequence followed by text.
                output.extend_from_slice(b"\x1b[12;");
                output.extend_from_slice(b"plain");
            }
            1 => {
                // Overlong parameter.
                output.extend_from_slice(b"\x1b[99999999999999999999A");
            }
            2 => {
                // Raw control bytes and an invalid UTF-8 tail.
                output.extend_from_slice(&[0x00, 0x01, 0x7f, 0xc3]);
            }
            _ => {
                // OSC with garbage payload, BEL-terminated.
                output.extend_from_slice(b"\x1b]4;1;#zzzzzz\x07");
            }
        }
    }

    fn generate_case(rng: &mut StdRng) -> Vec<TranscriptEvent> {
        let mut events = Vec::new();
        let event_count = rng.random_range(4..16);
        for _ in 0..event_count {
            if rng.random_ratio(1, 12) {
                events.push(TranscriptEvent::Resize {
                    columns: rng.random_range(8..=132),
                    rows: rng.random_range(4..=44),
                });
                continue;
            }
            let mut chunk = Vec::new();
            for _ in 0..rng.random_range(1..12) {
                match rng.random_range(0..12) {
                    0..=3 => random_text(rng, &mut chunk),
                    4..=5 => random_sgr(rng, &mut chunk),
                    6..=7 => random_cursor_op(rng, &mut chunk),
                    8 => random_edit_op(rng, &mut chunk),
                    9 => random_region_or_mode(rng, &mut chunk),
                    10 => chunk.extend_from_slice(b"\r\n"),
                    _ => random_malformed(rng, &mut chunk),
                }
            }
            events.push(TranscriptEvent::Bytes(chunk));
        }
        events
    }

    #[test]
    #[ignore = "non-gating differential fuzz lane (SPEC.md §7); run explicitly or via CI"]
    fn structured_vt_fuzz() {
        let seed = std::env::var("DIFFERENTIAL_FUZZ_SEED")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0x5EED_2026_u64);
        let cases = std::env::var("DIFFERENTIAL_FUZZ_CASES")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(96_usize);
        let waivers = fuzz_waivers();

        let mut findings = Vec::new();
        for case in 0..cases {
            let mut rng = StdRng::seed_from_u64(seed.wrapping_add(case as u64));
            let events = generate_case(&mut rng);
            let name = format!("fuzz case {case} (seed {seed})");
            if let Some(failure) = transcript_failure(&name, &events, &waivers, 1) {
                println!("=== {name} ===");
                println!("{failure}");
                println!("events: {events:?}");
                findings.push(name);
            }
        }

        assert!(
            findings.is_empty(),
            "{} fuzz finding(s) — adjudicate via the divergence ledger or promote to the \
             synthetic corpus:\n{}",
            findings.len(),
            findings.join("\n"),
        );
    }
}

mod perf {
    //! The alacritty perf-baseline scenarios (verification-strategy §6),
    //! reusing the transcript-feeding shape headless through the seam:
    //! bytes feed the backend in pump-sized chunks with a `Content` snapshot
    //! per chunk. Run on RELEASE builds via `script/terminal-perf-baseline`;
    //! baseline numbers are recorded in
    //! docs/ghostty-migration/perf-baseline.md. The fifth scenario —
    //! sustained flood through the full PTY seam — landed at P4 as
    //! `script/terminal-flood-bench`.

    use super::harness_bounds;
    use crate::terminal_settings::{AlternateScroll, CursorShape as SettingsCursorShape};
    use crate::{Content, Scroll, alacritty};

    const CHUNK: usize = 64 * 1024;
    const SCROLLBACK: usize = 10_000;

    fn baseline_backend() -> alacritty::TerminalBackend {
        let (events_tx, _events_rx) = futures::channel::mpsc::unbounded();
        // The receiver is dropped: alacritty's ZedListener sends events with
        // `unbounded_send(..).ok()`, so a benchmark run without an event
        // consumer is fine.
        alacritty::TerminalBackend::new(
            SCROLLBACK,
            SettingsCursorShape::Block,
            harness_bounds(120, 40),
            events_tx,
            AlternateScroll::On,
        )
    }

    /// Scenario bodies shared by the alacritty baseline and the ghostty P8
    /// comparison, duck-typed over the backend like the production seam (a
    /// trait would not compile off-Linux, where only alacritty exists).
    macro_rules! feed_scenario {
        ($name:expr, $backend:expr, $bytes:expr) => {{
            let mut backend = $backend;
            let bytes = $bytes;
            let mut content = Content::default();
            let started = std::time::Instant::now();
            for chunk in bytes.chunks(CHUNK) {
                backend.write(chunk);
                content = backend.make_content(&content);
            }
            let elapsed = started.elapsed();
            let mebibytes = bytes.len() as f64 / (1024.0 * 1024.0);
            println!(
                "  {}: {mebibytes:.1} MiB in {elapsed:.2?} → {:.1} MiB/s",
                $name,
                mebibytes / elapsed.as_secs_f64()
            );
            content
        }};
    }

    // Sustained scrolling with full scrollback: snapshot per scroll step,
    // the shape find/scroll interactions drive through the seam.
    macro_rules! scroll_scenario {
        ($backend:expr) => {{
            let mut backend = $backend;
            let mut fill = Vec::new();
            for line in 0..(SCROLLBACK + 100) {
                fill.extend_from_slice(format!("scrollback fill line {line}\r\n").as_bytes());
            }
            for chunk in fill.chunks(CHUNK) {
                backend.write(chunk);
            }
            let mut content = Content::default();
            let operations = 20_000;
            let started = std::time::Instant::now();
            for step in 0..operations {
                let scroll = match step % 100 {
                    0 => Scroll::Top,
                    50 => Scroll::Bottom,
                    s if s < 50 => Scroll::Delta(3),
                    _ => Scroll::Delta(-3),
                };
                backend.scroll_display(scroll);
                content = backend.make_content(&content);
            }
            let elapsed = started.elapsed();
            println!(
                "  sustained_scroll: {operations} scroll+snapshot ops over {} history lines in {elapsed:.2?} → {:.0} ops/s",
                SCROLLBACK,
                operations as f64 / elapsed.as_secs_f64()
            );
            assert!(!content.cells.is_empty(), "scroll scenario produced no snapshot");
        }};
    }

    fn colored_dump_bytes() -> Vec<u8> {
        let mut bytes = Vec::new();
        for line in 0..250_000_u32 {
            let foreground = line % 256;
            let background = (line * 7 + 3) % 256;
            bytes.extend_from_slice(
                format!(
                    "\x1b[38;5;{foreground}m\x1b[48;5;{background}mcolored dump line {line} with payload text\x1b[0m\r\n"
                )
                .as_bytes(),
            );
        }
        bytes
    }

    fn wide_char_bytes() -> Vec<u8> {
        let mut bytes = Vec::new();
        for line in 0..120_000_u32 {
            bytes.extend_from_slice(
                format!("\x1b[3{}m宽字符行 {line} 漢字かなカナ混在テキスト широкий 텍스트\x1b[0m\r\n", line % 8)
                    .as_bytes(),
            );
        }
        bytes
    }

    fn alt_screen_churn_bytes() -> Vec<u8> {
        let mut bytes = Vec::new();
        for cycle in 0..2_000_u32 {
            bytes.extend_from_slice(b"\x1b[?1049h\x1b[H");
            for row in 1..=40_u32 {
                bytes.extend_from_slice(
                    format!("\x1b[{row};1H\x1b[38;5;{}mredraw {cycle} row {row}\x1b[K", (cycle + row) % 256)
                        .as_bytes(),
                );
            }
            bytes.extend_from_slice(b"\x1b[?1049l");
        }
        bytes
    }

    #[test]
    #[ignore = "release-build benchmark; run via script/terminal-perf-baseline"]
    fn perf_baseline() {
        if cfg!(debug_assertions) {
            println!("WARNING: debug build — baseline numbers are only valid from release runs");
        }
        println!("terminal perf baseline (alacritty backend):");

        feed_scenario!("colored_dump", baseline_backend(), colored_dump_bytes());
        feed_scenario!("wide_char_cjk", baseline_backend(), wide_char_bytes());
        feed_scenario!("alt_screen_churn", baseline_backend(), alt_screen_churn_bytes());
        scroll_scenario!(baseline_backend());
    }

    /// The same scenarios through the ghostty backend, for the P8 gate:
    /// no scenario may regress more than 20% against the recorded alacritty
    /// baseline (SPEC.md §6 P8, §7; docs/ghostty-migration/perf-baseline.md).
    #[cfg(target_os = "linux")]
    mod ghostty_swap {
        use super::*;
        use crate::ghostty;

        fn swap_backend() -> ghostty::TerminalBackend {
            let (events_tx, _events_rx) = futures::channel::mpsc::unbounded();
            // The receiver is dropped: the ghostty backend forwards events
            // with `unbounded_send(..).ok()`, so a benchmark run without an
            // event consumer is fine.
            ghostty::TerminalBackend::new(
                SCROLLBACK,
                SettingsCursorShape::Block,
                harness_bounds(120, 40),
                events_tx,
                AlternateScroll::On,
            )
        }

        #[test]
        #[ignore = "release-build benchmark; run via script/terminal-perf-baseline"]
        fn perf_swap_ghostty() {
            if cfg!(debug_assertions) {
                println!(
                    "WARNING: debug build — swap numbers are only valid from release runs"
                );
            }
            println!("terminal perf (ghostty backend, P8 swap):");

            feed_scenario!("colored_dump", swap_backend(), colored_dump_bytes());
            feed_scenario!("wide_char_cjk", swap_backend(), wide_char_bytes());
            feed_scenario!("alt_screen_churn", swap_backend(), alt_screen_churn_bytes());
            scroll_scenario!(swap_backend());
        }

        /// The sustained-flood scenario through the ghostty backend: the P8
        /// twin of `pty::tests::sustained_flood_benchmark` (which stays on
        /// alacritty as the recorded-baseline probe), with the same seam —
        /// child → reader thread → bounded channel → batch-capped ingest —
        /// and the same reporting. Run in release via
        /// `script/terminal-flood-bench`.
        #[cfg(unix)]
        #[gpui::test]
        #[ignore = "benchmark; run via script/terminal-flood-bench"]
        async fn sustained_flood_ghostty(cx: &mut gpui::TestAppContext) {
            use crate::pty::{self, PtyOutput};
            use futures::FutureExt as _;
            use std::time::{Duration, Instant};

            cx.executor().allow_parking();
            let executor = cx.background_executor.clone();

            const FLOOD_BYTES: u64 = 256 * 1024 * 1024;
            const MARKER_INTERVAL: Duration = Duration::from_millis(250);
            const RECV_TIMEOUT: Duration = Duration::from_secs(20);

            let (output_tx, output_rx) = pty::output_channel();
            // The flood alphabet deliberately excludes 'Z', the echo marker.
            let spawned = pty::spawn_pty(
                pty::PtyOptions {
                    shell: Some((
                        "/bin/sh".to_string(),
                        vec![
                            "-c".to_string(),
                            format!(
                                "yes 0123456789abcdefghijklmnopqrstuv | head -c {FLOOD_BYTES}"
                            ),
                        ],
                    )),
                    working_directory: None,
                    env: collections::HashMap::default(),
                    window_id: 0,
                },
                crate::TerminalBounds::default(),
                output_tx,
                &executor,
            )
            .expect("failed to spawn flood pty");

            let mut backend = swap_backend();

            let mut total_bytes = 0u64;
            let mut turns = 0u64;
            let mut total_turn_time = Duration::ZERO;
            let mut max_turn_time = Duration::ZERO;
            let mut echo_samples = Vec::new();
            let mut marker_sent_at: Option<Instant> = None;
            let mut last_marker_at = Instant::now();
            let started_at = Instant::now();

            let mut ingest = |bytes: &[u8],
                              total_bytes: &mut u64,
                              marker_sent_at: &mut Option<Instant>,
                              echo_samples: &mut Vec<Duration>| {
                if let Some(sent_at) = *marker_sent_at
                    && bytes.contains(&b'Z')
                {
                    echo_samples.push(sent_at.elapsed());
                    *marker_sent_at = None;
                }
                backend.write(bytes);
                *total_bytes += bytes.len() as u64;
            };

            'pump: loop {
                let first = futures::select_biased! {
                    output = output_rx.recv().fuse() => {
                        output.expect("pty output channel closed unexpectedly")
                    }
                    _ = executor.timer(RECV_TIMEOUT).fuse() => {
                        panic!("timed out waiting for pty output")
                    }
                };
                let turn_started_at = Instant::now();
                let mut exited = false;
                match first {
                    PtyOutput::Bytes(bytes) => {
                        ingest(
                            &bytes,
                            &mut total_bytes,
                            &mut marker_sent_at,
                            &mut echo_samples,
                        );
                        let mut batches = 1;
                        while batches < pty::MAX_BATCHES_PER_TURN {
                            match output_rx.try_recv() {
                                Ok(PtyOutput::Bytes(bytes)) => {
                                    ingest(
                                        &bytes,
                                        &mut total_bytes,
                                        &mut marker_sent_at,
                                        &mut echo_samples,
                                    );
                                    batches += 1;
                                }
                                Ok(PtyOutput::Event(event)) => {
                                    exited = matches!(
                                        event,
                                        crate::TerminalBackendEvent::ChildExit(_)
                                    );
                                    break;
                                }
                                Err(_) => break,
                            }
                        }
                    }
                    PtyOutput::Event(event) => {
                        exited =
                            matches!(event, crate::TerminalBackendEvent::ChildExit(_));
                    }
                }

                let turn_time = turn_started_at.elapsed();
                turns += 1;
                total_turn_time += turn_time;
                max_turn_time = max_turn_time.max(turn_time);

                if exited {
                    break 'pump;
                }

                if marker_sent_at.is_none() && last_marker_at.elapsed() > MARKER_INTERVAL {
                    spawned.handle.notify(&b"Z"[..]);
                    marker_sent_at = Some(Instant::now());
                    last_marker_at = Instant::now();
                }
            }

            let wall = started_at.elapsed();

            assert!(
                total_bytes >= FLOOD_BYTES,
                "flood should deliver every byte, got {total_bytes} of {FLOOD_BYTES}"
            );

            let mib = total_bytes as f64 / (1024.0 * 1024.0);
            println!("sustained-flood benchmark (ghostty backend, P8 swap):");
            println!(
                "  throughput: {mib:.1} MiB in {:.2}s = {:.1} MiB/s",
                wall.as_secs_f64(),
                mib / wall.as_secs_f64()
            );
            println!(
                "  foreground turns: {turns}, mean {:?}, max (UI-stall bound) {:?}",
                total_turn_time / turns.max(1) as u32,
                max_turn_time
            );
            if echo_samples.is_empty() {
                println!("  echo latency: no samples");
            } else {
                let total: Duration = echo_samples.iter().sum();
                let min = echo_samples.iter().min().expect("nonempty");
                let max = echo_samples.iter().max().expect("nonempty");
                println!(
                    "  echo latency during flood: n={}, min {min:?}, mean {:?}, max {max:?}",
                    echo_samples.len(),
                    total / echo_samples.len() as u32,
                );
            }
        }
    }
}
