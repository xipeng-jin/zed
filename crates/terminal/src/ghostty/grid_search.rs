//! The Zed-owned grid search engine for the ghostty backend (SPEC.md §4.4,
//! gap G1): ghostty extracts text, the Rust `regex` crate matches, and match
//! byte offsets map back to grid cells. Stateless per call, split across the
//! `find_matches` threading sandwich — foreground extract, background regex,
//! foreground map.
//!
//! Ghostty Screen space (row 0 = top of scrollback) lives only inside this
//! module behind the typed [`ScreenPoint`]; every returned match is converted
//! to the alacritty grid convention (line 0 = top of the active screen,
//! scrollback negative) at the exit (SPEC.md §4.2 S3).
//!
//! Extraction characterization (pinned by the tests below): the bulk
//! `format_selection_buf` extract with `unwrap: false, trim: false` emits one
//! `\n`-separated text row per screen row, starting at the `select_all`
//! selection's start cell and ending at the last content row. Written spaces
//! and interior gap cells emit as spaces, wide chars emit once (spacers emit
//! nothing), grapheme clusters emit whole; cells after the last text cell of
//! a row emit nothing. That last point is a deliberate divergence from
//! alacritty, whose grid feeds trailing blank cells to the search as spaces —
//! a query with trailing whitespace can match beyond the written text there
//! but not here (divergence-ledger candidate for the P7 seeding).

use std::ops::Range as StdRange;

use ghostty_vt::{
    Terminal as GhosttyTerminal,
    error::Error as GhosttyError,
    fmt::Format,
    screen::{CellWide, GridRef},
    selection::{FormatOptions, Order},
    terminal::{Point as GhosttyPoint, PointCoordinate, PointSpace},
};
use regex::{Regex, RegexBuilder};
use util::ResultExt;

use crate::{Point, Range};

/// A point in ghostty Screen space: `row` counts from the top of scrollback.
/// Distinct from the grid-convention `crate::Point` by design (S3) so mixing
/// the two spaces is a compile error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ScreenPoint {
    row: u32,
    column: u16,
}

/// A compiled search query. Construction ports alacritty's `RegexSearch`
/// semantics (SPEC.md §4.4): smart-case (any uppercase in the pattern makes
/// it case-sensitive), and compile failure — including patterns over the
/// regex crate's default size limits, comparable to alacritty's
/// regex-automata cache bounds — yields `None`, which surfaces as no matches.
#[derive(Clone, Debug)]
pub(crate) struct SearchQuery {
    regex: Regex,
}

impl SearchQuery {
    pub(crate) fn new(pattern: &str) -> Option<Self> {
        let case_insensitive = !pattern.chars().any(char::is_uppercase);
        RegexBuilder::new(pattern)
            .case_insensitive(case_insensitive)
            .build()
            .ok()
            .map(|regex| Self { regex })
    }
}

/// Where a logical line sits in the grid: its first screen row, the column
/// the extraction started at on that row (non-zero only for the very first
/// extracted line, whose leading blank cells are outside the `select_all`
/// range), and how many screen rows it spans.
#[derive(Clone, Copy, Debug)]
pub(super) struct LineGeometry {
    first_row: u32,
    first_column: u16,
    row_count: u32,
}

/// One logical line of the extracted buffer: soft-wrapped screen rows joined
/// without a separator, hard newlines splitting lines (the hard-newline
/// barrier of SPEC.md §4.4 is structural — no pattern can match across
/// logical lines).
#[derive(Debug)]
pub(super) struct LogicalLine {
    geometry: LineGeometry,
    text: String,
}

/// The foreground half of the sandwich: the compiled query plus the bulk
/// extract, ready to move to a background thread for matching.
#[derive(Debug)]
pub(crate) struct PreparedSearch {
    query: SearchQuery,
    lines: Vec<LogicalLine>,
}

/// Byte-range matches per logical line, produced on the background thread;
/// only the line geometry and byte ranges travel back for the foreground map.
#[derive(Debug)]
pub(crate) struct FoundMatches {
    lines: Vec<MatchedLine>,
}

#[derive(Debug)]
struct MatchedLine {
    geometry: LineGeometry,
    text_len: usize,
    byte_ranges: Vec<StdRange<usize>>,
}

impl PreparedSearch {
    pub(super) fn new(query: SearchQuery, lines: Vec<LogicalLine>) -> Self {
        Self { query, lines }
    }

    /// Run the regex over every logical line slice, discarding zero-length
    /// matches (alacritty cannot highlight them and resets past them).
    /// Pure CPU over owned text — this is the background stage, crate-visible
    /// so `Terminal::find_matches` can drive the sandwich at the P8 swap.
    pub(crate) fn find_matches(self) -> FoundMatches {
        let lines = self
            .lines
            .into_iter()
            .filter_map(|line| {
                let byte_ranges: Vec<_> = self
                    .query
                    .regex
                    .find_iter(&line.text)
                    .map(|regex_match| regex_match.range())
                    .filter(|range| !range.is_empty())
                    .collect();
                (!byte_ranges.is_empty()).then_some(MatchedLine {
                    geometry: line.geometry,
                    text_len: line.text.len(),
                    byte_ranges,
                })
            })
            .collect();
        FoundMatches { lines }
    }
}

/// Phase-1 bulk extraction (SPEC.md §4.4): one `format_selection_buf` call
/// over a `select_all` selection plus a per-row wrap-flag pass joining
/// soft-wrapped rows into logical lines. Whole scrollback, no cap.
pub(super) fn extract_logical_lines(terminal: &GhosttyTerminal<'_, '_>) -> Vec<LogicalLine> {
    match try_extract_logical_lines(terminal) {
        Ok(lines) => lines,
        Err(error) => {
            log::error!("ghostty grid search failed to extract the buffer: {error}");
            Vec::new()
        }
    }
}

fn try_extract_logical_lines(
    terminal: &GhosttyTerminal<'_, '_>,
) -> Result<Vec<LogicalLine>, GhosttyError> {
    let Some(selection) = terminal.select_all()? else {
        return Ok(Vec::new());
    };
    let selection = selection.to_ordered(terminal, Order::Forward)?;
    let Some(start) = terminal.point_from_grid_ref(&selection.start(), PointSpace::Screen)? else {
        return Ok(Vec::new());
    };

    // Sized so the ASCII-dominant common case formats in one call; the
    // OutOfSpace retry reports the exact size otherwise.
    let mut buffer = vec![0; terminal.total_rows()? * (terminal.cols()? as usize + 1)];
    let text = loop {
        let options = FormatOptions::new()
            .with_emit_format(Format::Plain)
            .with_unwrap(false)
            .with_trim(false)
            .with_selection(&selection);
        match terminal.format_selection_buf(options, &mut buffer) {
            Ok(Some(written)) => {
                let Some(bytes) = buffer.get(..written) else {
                    log::error!(
                        "ghostty grid search formatter reported {written} bytes for a \
                         {}-byte buffer",
                        buffer.len()
                    );
                    return Ok(Vec::new());
                };
                match String::from_utf8(bytes.to_vec()) {
                    Ok(text) => break text,
                    Err(error) => {
                        log::error!("ghostty grid search extracted invalid utf-8: {error}");
                        return Ok(Vec::new());
                    }
                }
            }
            Ok(None) => return Ok(Vec::new()),
            Err(GhosttyError::OutOfSpace { required }) => buffer.resize(required, 0),
            Err(error) => return Err(error),
        }
    };

    let mut lines: Vec<LogicalLine> = Vec::new();
    for (row_offset, row_text) in text.split('\n').enumerate() {
        let row = start.y + row_offset as u32;
        // The first extracted row has no predecessor to join even when the
        // row is a wrap continuation of a line already pruned from
        // scrollback.
        let continuation = row_offset > 0
            && terminal
                .grid_ref(GhosttyPoint::Screen(PointCoordinate { x: 0, y: row }))?
                .row()?
                .is_wrap_continuation()?;
        match lines.last_mut() {
            Some(line) if continuation => {
                line.text.push_str(row_text);
                line.geometry.row_count += 1;
            }
            _ => lines.push(LogicalLine {
                geometry: LineGeometry {
                    first_row: row,
                    first_column: if row_offset == 0 { start.x } else { 0 },
                    row_count: 1,
                },
                text: row_text.to_string(),
            }),
        }
    }
    Ok(lines)
}

/// The byte-offset ↔ cell table for one logical line (phase 2, SPEC.md §4.4):
/// one span per emitted chunk. A wide char's span covers its head cell and
/// trailing spacer, so matches ending on a wide char highlight both columns,
/// matching alacritty's match ranges.
#[derive(Debug)]
pub(super) struct ByteCellMap {
    spans: Vec<CellSpan>,
    total_bytes: usize,
}

#[derive(Debug)]
struct CellSpan {
    byte_offset: usize,
    start: ScreenPoint,
    end: ScreenPoint,
}

impl ByteCellMap {
    /// Total bytes the mapped cells would emit; disagreement with the
    /// extracted text length means the grid changed between extract and map
    /// (or the walk failed) and the line's matches must be dropped — drift
    /// self-heals through the search-bar re-query cadence.
    pub(super) fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    /// Resolve a non-empty byte range to an inclusive Screen-space cell
    /// range.
    pub(super) fn cell_range(&self, bytes: &StdRange<usize>) -> Option<(ScreenPoint, ScreenPoint)> {
        if bytes.start >= bytes.end || bytes.end > self.total_bytes {
            return None;
        }
        let start = self.span_at(bytes.start)?.start;
        let end = self.span_at(bytes.end - 1)?.end;
        Some((start, end))
    }

    fn span_at(&self, byte: usize) -> Option<&CellSpan> {
        let index = self
            .spans
            .partition_point(|span| span.byte_offset <= byte)
            .checked_sub(1)?;
        self.spans.get(index)
    }
}

/// Walk one logical line's cells and reconstruct the byte stream the phase-1
/// extraction emitted for it: grapheme clusters accumulate their UTF-8
/// lengths, spacers emit nothing (a `SpacerTail` extends its wide char's
/// span), and gap cells without text emit one space each only when more text
/// follows on the same row. FFI cost scales with match rows, not cells.
pub(super) fn cell_map_for_line(
    terminal: &GhosttyTerminal<'_, '_>,
    geometry: LineGeometry,
) -> ByteCellMap {
    match try_cell_map_for_line(terminal, geometry) {
        Ok(map) => map,
        Err(error) => {
            log::error!("ghostty grid search failed to map a line: {error}");
            // An empty map fails the total-bytes drift check, dropping the
            // line's matches instead of mis-mapping them.
            ByteCellMap {
                spans: Vec::new(),
                total_bytes: 0,
            }
        }
    }
}

fn try_cell_map_for_line(
    terminal: &GhosttyTerminal<'_, '_>,
    geometry: LineGeometry,
) -> Result<ByteCellMap, GhosttyError> {
    let columns = terminal.cols()?;
    let mut spans: Vec<CellSpan> = Vec::new();
    let mut total_bytes = 0;
    let mut grapheme_buffer = vec!['\0'; 8];

    for row_offset in 0..geometry.row_count {
        let row = geometry.first_row + row_offset;
        let start_column = if row_offset == 0 {
            geometry.first_column
        } else {
            0
        };
        let mut pending_gap_columns: Vec<u16> = Vec::new();

        for column in start_column..columns {
            let grid_ref =
                terminal.grid_ref(GhosttyPoint::Screen(PointCoordinate { x: column, y: row }))?;
            let cell = grid_ref.cell()?;
            match cell.wide()? {
                CellWide::SpacerTail => {
                    if let Some(last) = spans.last_mut()
                        && last.end.row == row
                        && last.end.column + 1 == column
                    {
                        last.end = ScreenPoint { row, column };
                    }
                    continue;
                }
                CellWide::SpacerHead => continue,
                CellWide::Narrow | CellWide::Wide => {}
            }

            if !cell.has_text()? {
                pending_gap_columns.push(column);
                continue;
            }

            for gap_column in pending_gap_columns.drain(..) {
                let point = ScreenPoint {
                    row,
                    column: gap_column,
                };
                spans.push(CellSpan {
                    byte_offset: total_bytes,
                    start: point,
                    end: point,
                });
                total_bytes += 1;
            }

            let byte_len = cluster_byte_len(&grid_ref, &mut grapheme_buffer)?;
            if byte_len == 0 {
                continue;
            }
            let point = ScreenPoint { row, column };
            spans.push(CellSpan {
                byte_offset: total_bytes,
                start: point,
                end: point,
            });
            total_bytes += byte_len;
        }
        // Cells after a row's last text cell were never emitted by the
        // extraction, so pending gaps die at the row boundary.
    }

    Ok(ByteCellMap { spans, total_bytes })
}

fn cluster_byte_len(
    grid_ref: &GridRef<'_>,
    buffer: &mut Vec<char>,
) -> Result<usize, GhosttyError> {
    loop {
        match grid_ref.graphemes(buffer) {
            Ok(count) => {
                return Ok(buffer
                    .get(..count)
                    .unwrap_or_default()
                    .iter()
                    .map(|character| character.len_utf8())
                    .sum());
            }
            Err(GhosttyError::OutOfSpace { required }) => buffer.resize(required, '\0'),
            Err(error) => return Err(error),
        }
    }
}

/// The foreground map stage: resolve every match's byte range through its
/// line's byte↔cell table and convert out of Screen space at the exit —
/// the only place matches cross from `ScreenPoint` to the grid convention.
pub(super) fn resolve_matches(
    terminal: &GhosttyTerminal<'_, '_>,
    found: FoundMatches,
) -> Vec<Range> {
    // Without the scrollback depth the Screen→grid conversion is impossible,
    // and matches must never leave this module in Screen space (S3) — drop
    // them all; a re-search self-heals.
    let Some(scrollback_rows) = terminal.scrollback_rows().log_err() else {
        return Vec::new();
    };
    let scrollback_rows = scrollback_rows as i32;
    let mut matches = Vec::new();
    for line in found.lines {
        let map = cell_map_for_line(terminal, line.geometry);
        if map.total_bytes() != line.text_len {
            continue;
        }
        for byte_range in &line.byte_ranges {
            if let Some((start, end)) = map.cell_range(byte_range) {
                matches.push(Range::new(
                    grid_point(start, scrollback_rows),
                    grid_point(end, scrollback_rows),
                ));
            }
        }
    }
    matches
}

fn grid_point(point: ScreenPoint, scrollback_rows: i32) -> Point {
    Point::new(point.row as i32 - scrollback_rows, point.column as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        PtyEvent, Search, TerminalBounds,
        terminal_settings::{AlternateScroll, CursorShape as SettingsCursorShape},
    };
    use futures::channel::mpsc::UnboundedReceiver;
    use gpui::{Bounds, px, size};

    fn test_bounds(columns: usize, screen_lines: usize) -> TerminalBounds {
        TerminalBounds::new(
            px(16.),
            px(8.),
            Bounds {
                origin: Default::default(),
                size: size(px(8. * columns as f32), px(16. * screen_lines as f32)),
            },
        )
    }

    fn ghostty_backend(
        columns: usize,
        screen_lines: usize,
        scrollback: usize,
    ) -> (super::super::TerminalBackend, UnboundedReceiver<PtyEvent>) {
        let (events_tx, events_rx) = futures::channel::mpsc::unbounded();
        let backend = super::super::TerminalBackend::new(
            scrollback,
            SettingsCursorShape::Block,
            test_bounds(columns, screen_lines),
            events_tx,
            AlternateScroll::On,
        );
        (backend, events_rx)
    }

    fn ghostty_search(backend: &super::super::TerminalBackend, pattern: &str) -> Vec<Range> {
        let query = SearchQuery::new(pattern).expect("pattern should compile");
        let found = backend.prepare_search(query).find_matches();
        backend.search_matches(found)
    }

    fn alacritty_search(
        columns: usize,
        screen_lines: usize,
        scrollback: usize,
        input: &[u8],
        pattern: &str,
    ) -> Vec<Range> {
        let (events_tx, _events_rx) = futures::channel::mpsc::unbounded();
        let mut backend = crate::alacritty::TerminalBackend::new(
            scrollback,
            SettingsCursorShape::Block,
            test_bounds(columns, screen_lines),
            events_tx,
            AlternateScroll::On,
        );
        backend.write(input);
        let searcher = Search::new(pattern).expect("pattern should compile");
        backend.prepare_search(searcher).run()
    }

    /// Compare the ghostty engine against the alacritty search as oracle:
    /// same bytes, same pattern, identical grid-convention ranges.
    fn assert_matches_oracle(
        columns: usize,
        screen_lines: usize,
        scrollback: usize,
        input: &[u8],
        pattern: &str,
    ) -> Vec<Range> {
        let (mut backend, _events_rx) = ghostty_backend(columns, screen_lines, scrollback);
        backend.write(input);
        let ghostty_matches = ghostty_search(&backend, pattern);
        let alacritty_matches =
            alacritty_search(columns, screen_lines, scrollback, input, pattern);
        assert_eq!(
            ghostty_matches, alacritty_matches,
            "ghostty and alacritty matches should agree for {pattern:?}"
        );
        ghostty_matches
    }

    #[test]
    fn literal_matches_cover_scrollback_in_grid_coordinates() {
        let input =
            b"alpha one\r\nbeta two\r\nalpha three\r\nfour\r\nfive\r\nsix\r\nseven\r\nalpha end";
        let matches = assert_matches_oracle(20, 5, 100, input, "alpha");
        assert_eq!(
            matches,
            vec![
                Range::new(Point::new(-3, 0), Point::new(-3, 4)),
                Range::new(Point::new(-1, 0), Point::new(-1, 4)),
                Range::new(Point::new(4, 0), Point::new(4, 4)),
            ],
            "matches should arrive top-to-bottom with scrollback lines negative"
        );
    }

    #[test]
    fn multiple_matches_on_one_line_stay_ordered() {
        let matches = assert_matches_oracle(30, 5, 0, b"ab ab ab", "ab");
        assert_eq!(
            matches,
            vec![
                Range::new(Point::new(0, 0), Point::new(0, 1)),
                Range::new(Point::new(0, 3), Point::new(0, 4)),
                Range::new(Point::new(0, 6), Point::new(0, 7)),
            ]
        );
    }

    #[test]
    fn smart_case_ports_exactly() {
        let input = b"Hello hello HELLO";
        let insensitive = assert_matches_oracle(30, 5, 0, input, "hello");
        assert_eq!(insensitive.len(), 3, "lowercase query is case-insensitive");
        let sensitive = assert_matches_oracle(30, 5, 0, input, "Hello");
        assert_eq!(
            sensitive,
            vec![Range::new(Point::new(0, 0), Point::new(0, 4))],
            "any uppercase makes the query case-sensitive"
        );
    }

    #[test]
    fn matches_cross_soft_wraps_but_not_hard_newlines() {
        // "abcdefghij" soft-wraps on a 6-column screen; the logical line is
        // whole for the regex.
        let wrapped = assert_matches_oracle(6, 5, 0, b"abcdefghij", "efgh");
        assert_eq!(
            wrapped,
            vec![Range::new(Point::new(0, 4), Point::new(1, 1))],
            "a match should span the soft-wrap boundary"
        );

        // The same adjacency across a hard newline is structurally
        // unmatchable.
        let hard = assert_matches_oracle(6, 5, 0, b"abcd\r\nefgh", "cdef");
        assert_eq!(hard, vec![]);
    }

    #[test]
    fn wide_char_matches_include_the_trailing_spacer() {
        let matches = assert_matches_oracle(20, 5, 0, "a\u{4e16}b".as_bytes(), "\u{4e16}");
        assert_eq!(
            matches,
            vec![Range::new(Point::new(0, 1), Point::new(0, 2))],
            "a match ending on a wide char covers its spacer cell"
        );

        let spanning = assert_matches_oracle(20, 5, 0, "a\u{4e16}b".as_bytes(), "a\u{4e16}b");
        assert_eq!(
            spanning,
            vec![Range::new(Point::new(0, 0), Point::new(0, 3))]
        );
    }

    #[test]
    fn wide_char_at_a_soft_wrap_boundary_matches_across_rows() {
        // The wide char doesn't fit in column 3 of a 4-column screen: a
        // spacer head fills it and the char wraps.
        let matches = assert_matches_oracle(4, 5, 0, "abc\u{4e16}xy".as_bytes(), "c\u{4e16}x");
        assert_eq!(
            matches,
            vec![Range::new(Point::new(0, 2), Point::new(1, 2))]
        );
    }

    #[test]
    fn empty_matches_are_discarded() {
        let star = assert_matches_oracle(20, 5, 0, b"baaa c", "a*");
        assert_eq!(
            star,
            vec![Range::new(Point::new(0, 1), Point::new(0, 3))],
            "only the non-empty match survives"
        );
        let never = assert_matches_oracle(20, 5, 0, b"abc", "x*");
        assert_eq!(never, vec![]);
    }

    #[test]
    fn compile_failures_yield_no_query() {
        assert!(SearchQuery::new("[").is_none(), "invalid syntax");
        assert!(
            SearchQuery::new("x{99999999}").is_none(),
            "patterns over the size limit degrade to no matches"
        );
        // Parity with the alacritty-backed Search contract.
        assert!(Search::new("[").is_none());
    }

    #[test]
    fn matches_are_independent_of_the_display_offset() {
        let input = b"alpha\r\n1\r\n2\r\n3\r\n4\r\n5\r\n6\r\nalpha";
        let (mut backend, _events_rx) = ghostty_backend(20, 5, 100);
        backend.write(input);

        let at_bottom = ghostty_search(&backend, "alpha");
        backend.scroll_display(crate::Scroll::Top);
        let scrolled = ghostty_search(&backend, "alpha");
        assert_eq!(at_bottom, scrolled, "scrolling must not move matches");
        assert_eq!(scrolled, alacritty_search(20, 5, 100, input, "alpha"));
    }

    #[test]
    fn interior_gap_cells_match_as_spaces() {
        // Cursor-jump gaps have no text but sit between written cells; both
        // engines see them as spaces.
        let matches = assert_matches_oracle(10, 5, 0, b"a\x1b[5Gb", "a   b");
        assert_eq!(
            matches,
            vec![Range::new(Point::new(0, 0), Point::new(0, 4))]
        );
    }

    #[test]
    fn written_trailing_spaces_stay_matchable() {
        let matches = assert_matches_oracle(10, 5, 0, b"ab  \r\ncd", "b ");
        assert_eq!(
            matches,
            vec![Range::new(Point::new(0, 1), Point::new(0, 2))]
        );
    }

    /// Characterization of the accepted extraction divergence (module doc,
    /// divergence-ledger candidate for the P7 seeding): alacritty's grid
    /// feeds never-written trailing cells to the search as spaces, the
    /// ghostty extraction stops at the last text cell of each row.
    #[test]
    fn trailing_unwritten_cells_do_not_match_unlike_alacritty() {
        let input = b"ab\r\ncd";
        let (mut backend, _events_rx) = ghostty_backend(10, 5, 0);
        backend.write(input);
        assert_eq!(ghostty_search(&backend, "ab "), vec![]);
        assert_eq!(
            alacritty_search(10, 5, 0, input, "ab "),
            vec![Range::new(Point::new(0, 0), Point::new(0, 2))],
            "the oracle matching here is what makes this a divergence"
        );
    }

    #[test]
    fn grapheme_clusters_resolve_to_their_cell() {
        let input = "e\u{301}xe".as_bytes();
        let matches = assert_matches_oracle(10, 5, 0, input, "x");
        assert_eq!(
            matches,
            vec![Range::new(Point::new(0, 1), Point::new(0, 1))]
        );
        let e_matches = assert_matches_oracle(10, 5, 0, input, "e");
        assert_eq!(
            e_matches,
            vec![
                Range::new(Point::new(0, 0), Point::new(0, 0)),
                Range::new(Point::new(0, 2), Point::new(0, 2)),
            ],
            "a match on a cluster's base char resolves to the cluster's cell"
        );
    }

    #[test]
    fn empty_terminal_and_absent_patterns_yield_no_matches() {
        let (backend, _events_rx) = ghostty_backend(10, 5, 0);
        assert_eq!(ghostty_search(&backend, "anything"), vec![]);

        let (mut backend, _events_rx) = ghostty_backend(10, 5, 0);
        backend.write(b"some content");
        assert_eq!(ghostty_search(&backend, "absent"), vec![]);
    }

    // Byte↔cell map unit tests (#35 §4): wide chars, spacer head at
    // soft-wrap, multi-codepoint graphemes, and match bounds on each.

    fn single_line_map(
        backend: &super::super::TerminalBackend,
    ) -> (ByteCellMap, LineGeometry, String) {
        let mut lines = extract_logical_lines(&backend.terminal);
        assert_eq!(lines.len(), 1, "expected one logical line");
        let line = lines.remove(0);
        let map = cell_map_for_line(&backend.terminal, line.geometry);
        (map, line.geometry, line.text)
    }

    fn screen_point(row: u32, column: u16) -> ScreenPoint {
        ScreenPoint { row, column }
    }

    #[test]
    fn byte_cell_map_expands_wide_chars() {
        let (mut backend, _events_rx) = ghostty_backend(10, 5, 0);
        backend.write("a\u{4e16}b".as_bytes());
        let (map, _geometry, text) = single_line_map(&backend);

        assert_eq!(text, "a\u{4e16}b");
        assert_eq!(map.total_bytes(), text.len());
        assert_eq!(
            map.cell_range(&(0..1)),
            Some((screen_point(0, 0), screen_point(0, 0)))
        );
        assert_eq!(
            map.cell_range(&(1..4)),
            Some((screen_point(0, 1), screen_point(0, 2))),
            "the wide char's span covers head and spacer"
        );
        assert_eq!(
            map.cell_range(&(4..5)),
            Some((screen_point(0, 3), screen_point(0, 3))),
            "the cell after the spacer is column 3"
        );
        assert_eq!(
            map.cell_range(&(0..5)),
            Some((screen_point(0, 0), screen_point(0, 3)))
        );
    }

    #[test]
    fn byte_cell_map_skips_the_spacer_head_at_soft_wraps() {
        let (mut backend, _events_rx) = ghostty_backend(4, 5, 0);
        backend.write("abc\u{4e16}xy".as_bytes());
        let (map, geometry, text) = single_line_map(&backend);

        assert_eq!(text, "abc\u{4e16}xy");
        assert_eq!(geometry.row_count, 2);
        assert_eq!(map.total_bytes(), text.len());
        assert_eq!(
            map.cell_range(&(2..3)),
            Some((screen_point(0, 2), screen_point(0, 2))),
            "a match ending before the spacer head stops at the text cell"
        );
        assert_eq!(
            map.cell_range(&(3..6)),
            Some((screen_point(1, 0), screen_point(1, 1))),
            "the wrapped wide char starts the next row"
        );
        assert_eq!(
            map.cell_range(&(2..7)),
            Some((screen_point(0, 2), screen_point(1, 2))),
            "a match across the wrap spans both rows"
        );
    }

    #[test]
    fn byte_cell_map_keeps_grapheme_clusters_on_one_cell() {
        let (mut backend, _events_rx) = ghostty_backend(10, 5, 0);
        backend.write("e\u{301}x".as_bytes());
        let (map, _geometry, text) = single_line_map(&backend);

        assert_eq!(text, "e\u{301}x");
        assert_eq!(map.total_bytes(), text.len());
        for byte_range in [0..1, 0..3, 1..3] {
            assert_eq!(
                map.cell_range(&byte_range.clone()),
                Some((screen_point(0, 0), screen_point(0, 0))),
                "any slice of the cluster resolves to its cell ({byte_range:?})"
            );
        }
        assert_eq!(
            map.cell_range(&(3..4)),
            Some((screen_point(0, 1), screen_point(0, 1)))
        );
    }

    #[test]
    fn byte_cell_map_rejects_empty_and_out_of_bounds_ranges() {
        let (mut backend, _events_rx) = ghostty_backend(10, 5, 0);
        backend.write(b"abc");
        let (map, _geometry, text) = single_line_map(&backend);

        assert_eq!(map.total_bytes(), text.len());
        assert_eq!(map.cell_range(&(1..1)), None, "empty ranges are unmappable");
        assert_eq!(
            map.cell_range(&(0..text.len() + 1)),
            None,
            "out-of-bounds ranges mean drift and must not mis-map"
        );
    }

    // Extraction characterization: pin the phase-1 semantics the engine
    // depends on (module doc; #35 §4 pinned risk).

    #[test]
    fn extraction_aligns_rows_and_blank_lines() {
        let (mut backend, _events_rx) = ghostty_backend(10, 5, 0);
        backend.write(b"top\r\n\r\n\r\nbottom");
        let lines = extract_logical_lines(&backend.terminal);
        let rows: Vec<_> = lines
            .iter()
            .map(|line| (line.geometry.first_row, line.text.as_str()))
            .collect();
        assert_eq!(
            rows,
            vec![(0, "top"), (1, ""), (2, ""), (3, "bottom")],
            "blank rows emit empty logical lines and keep row alignment"
        );
    }

    #[test]
    fn extraction_joins_soft_wrapped_rows() {
        let (mut backend, _events_rx) = ghostty_backend(6, 5, 100);
        backend.write(b"abcdefghij\r\n1\r\n2\r\n3\r\n4\r\n5");
        let lines = extract_logical_lines(&backend.terminal);
        assert_eq!(lines[0].text, "abcdefghij");
        assert_eq!(lines[0].geometry.row_count, 2);
        assert_eq!(
            lines.len(),
            6,
            "wrapped rows join; scrollback rows extract too"
        );
    }

    #[test]
    fn extraction_anchors_on_the_selections_start_cell() {
        let (mut backend, _events_rx) = ghostty_backend(10, 5, 0);
        backend.write(b"\x1b[3;2Hx");
        let lines = extract_logical_lines(&backend.terminal);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].geometry.first_row, 2);
        assert_eq!(
            lines[0].geometry.first_column, 1,
            "select_all starts at the first content cell, not column 0"
        );
        assert_eq!(lines[0].text, "x");
    }

    /// The max-scrollback extraction benchmark (SPEC.md §4.4, #35 §4): bulk
    /// extract + line map at the maximum configured scrollback, the
    /// foreground-stall number that gates any future cache retrofit, plus
    /// the background regex and foreground map stages for context. Run in
    /// release via `script/terminal-search-bench`.
    #[test]
    #[ignore = "benchmark; run via script/terminal-search-bench"]
    fn max_scrollback_extraction_benchmark() {
        use std::time::Instant;

        const COLUMNS: usize = 120;
        const SCREEN_LINES: usize = 40;
        const FILL_LINES: usize = crate::MAX_SCROLL_HISTORY_LINES + 50;
        const ITERATIONS: usize = 10;

        let (mut backend, _events_rx) =
            ghostty_backend(COLUMNS, SCREEN_LINES, crate::MAX_SCROLL_HISTORY_LINES);
        let mut input = Vec::new();
        for index in 0..FILL_LINES {
            let needle = if index % 1000 == 0 { "needle" } else { "filler" };
            input.extend_from_slice(
                format!(
                    "line {index:06} {needle} lorem ipsum dolor sit amet consectetur \
                     adipiscing elit sed do eiusmod tempor incididunt ut labore\r\n"
                )
                .as_bytes(),
            );
        }
        let ingest_started = Instant::now();
        backend.write(&input);
        let ingest_time = ingest_started.elapsed();

        let total_lines = backend.total_lines();
        assert!(
            total_lines >= crate::MAX_SCROLL_HISTORY_LINES,
            "the benchmark must run at max scrollback (got {total_lines} rows)"
        );

        let mut extract_times = Vec::new();
        let mut line_count = 0;
        for _ in 0..ITERATIONS {
            let started = Instant::now();
            let lines = extract_logical_lines(&backend.terminal);
            extract_times.push(started.elapsed());
            line_count = lines.len();
        }
        extract_times.sort();

        // The extraction's two sub-stages measured standalone, so the record
        // says where the time goes: the one bulk format call vs the
        // per-row wrap-flag walk.
        let mut format_times = Vec::new();
        let mut flag_walk_times = Vec::new();
        for _ in 0..ITERATIONS {
            let started = Instant::now();
            let selection = backend
                .terminal
                .select_all()
                .expect("select_all")
                .expect("non-empty buffer");
            let mut buffer =
                vec![0; backend.terminal.total_rows().expect("total_rows") * (COLUMNS + 1)];
            let options = FormatOptions::new()
                .with_emit_format(Format::Plain)
                .with_unwrap(false)
                .with_trim(false)
                .with_selection(&selection);
            let written = backend
                .terminal
                .format_selection_buf(options, &mut buffer)
                .expect("format")
                .expect("non-empty selection");
            format_times.push(started.elapsed());
            assert!(written > 0);

            let started = Instant::now();
            let mut continuations = 0usize;
            for row in 0..total_lines {
                let continuation = backend
                    .terminal
                    .grid_ref(GhosttyPoint::Screen(PointCoordinate {
                        x: 0,
                        y: row as u32,
                    }))
                    .and_then(|grid_ref| grid_ref.row())
                    .and_then(|grid_row| grid_row.is_wrap_continuation())
                    .expect("wrap flag");
                continuations += continuation as usize;
            }
            flag_walk_times.push(started.elapsed());
            assert_eq!(continuations, 0);
        }
        format_times.sort();
        flag_walk_times.sort();

        let report_stage = |name: &str, times: &[std::time::Duration]| {
            let total: std::time::Duration = times.iter().sum();
            println!(
                "{name}: min {:?} / median {:?} / mean {:?} over {} runs",
                times.first().copied().unwrap_or_default(),
                times.get(times.len() / 2).copied().unwrap_or_default(),
                total.checked_div(times.len() as u32).unwrap_or_default(),
                times.len(),
            );
        };

        println!(
            "extraction benchmark: {total_lines} rows x {COLUMNS} cols, {line_count} logical lines, \
             ingest {ingest_time:?}"
        );
        report_stage("bulk extract + line map (foreground)", &extract_times);
        report_stage("  sub-stage: format_selection_buf only", &format_times);
        report_stage("  sub-stage: wrap-flag walk only", &flag_walk_times);

        for (name, pattern) in [("sparse", "needle"), ("dense", "lorem")] {
            let mut regex_times = Vec::new();
            let mut resolve_times = Vec::new();
            let mut match_count = 0;
            for _ in 0..ITERATIONS {
                let query = SearchQuery::new(pattern).expect("pattern should compile");
                let prepared =
                    PreparedSearch::new(query, extract_logical_lines(&backend.terminal));
                let started = Instant::now();
                let found = prepared.find_matches();
                regex_times.push(started.elapsed());
                let started = Instant::now();
                let matches = backend.search_matches(found);
                resolve_times.push(started.elapsed());
                match_count = matches.len();
            }
            regex_times.sort();
            resolve_times.sort();
            println!("query {name:?} ({pattern:?}): {match_count} matches");
            report_stage("  regex over logical lines (background)", &regex_times);
            report_stage("  byte-cell map + resolve (foreground)", &resolve_times);
        }
    }
}
