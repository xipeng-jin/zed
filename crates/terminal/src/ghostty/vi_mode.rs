//! The vi-mode motion port (SPEC.md §4.5 G2, §6 P6): alacritty's
//! `vi_mode.rs` scoped to the 16 motions Zed dispatches, implemented over
//! seam grid reads. The `ghostty::TerminalBackend` owns the vi cursor and
//! the Zed-side vi state that `Modes::VI` is synthesized from (SPEC.md
//! §4.2); the functions here are pure point computations, with the
//! scroll-follow (`scroll_to_point`) applied by the backend after each
//! motion, mirroring alacritty's `ViModeCursor::motion` tail.
//!
//! Ported from the pinned zed-industries/alacritty rev `4c129667ce`
//! (`alacritty_terminal/src/vi_mode.rs` plus the `Term` helpers it leans
//! on: `expand_wide`, `bracket_search`, `Row::is_clear`). Alacritty's
//! `Side` is a type alias of `Direction`, so both port to `SelectionSide`.

use std::cmp::min;

use ghostty_vt::screen::CellWide;

use crate::{Point, SelectionSide, ViMotion};

use super::{GridDimensions, PointBoundary, TerminalBackend, point_add, point_clamp, point_sub};

/// Bracket pairs `ViMotion::Bracket` jumps between (alacritty
/// `term::search::BRACKET_PAIRS`).
const BRACKET_PAIRS: [(char, char); 4] = [('(', ')'), ('[', ']'), ('{', '}'), ('<', '>')];

/// Compute the vi cursor's landing point for a motion (alacritty
/// `ViModeCursor::motion`, minus its trailing `scroll_to_point`).
pub(super) fn motion(
    backend: &TerminalBackend,
    mut point: Point,
    motion: ViMotion,
) -> Point {
    let dimensions = GridDimensions::of(backend);

    match motion {
        ViMotion::Up => {
            if point.line > dimensions.topmost_line {
                point.line -= 1;
            }
        }
        ViMotion::Down => {
            if point.line + 1 < dimensions.screen_lines as i32 {
                point.line += 1;
            }
        }
        ViMotion::Left => {
            point = backend.expand_wide(point, SelectionSide::Left);
            let wrap_point = Point::new(point.line - 1, dimensions.last_column());
            if point.column == 0
                && point.line > dimensions.topmost_line
                && is_wrap(backend, dimensions, wrap_point)
            {
                point = wrap_point;
            } else {
                point.column = point.column.saturating_sub(1);
            }
        }
        ViMotion::Right => {
            point = backend.expand_wide(point, SelectionSide::Right);
            if is_wrap(backend, dimensions, point) {
                point = Point::new(point.line + 1, 0);
            } else {
                point.column = min(point.column + 1, dimensions.last_column());
            }
        }
        ViMotion::First => {
            point = backend.expand_wide(point, SelectionSide::Left);
            while point.column == 0
                && point.line > dimensions.topmost_line
                && is_wrap(
                    backend,
                    dimensions,
                    Point::new(point.line - 1, dimensions.last_column()),
                )
            {
                point.line -= 1;
            }
            point.column = 0;
        }
        ViMotion::Last => point = last(backend, dimensions, point),
        ViMotion::FirstOccupied => point = first_occupied(backend, dimensions, point),
        ViMotion::High => {
            let line = -(backend.display_offset() as i32);
            let column =
                first_occupied_in_line(backend, dimensions, line).map_or(0, |point| point.column);
            point = Point::new(line, column);
        }
        ViMotion::Middle => {
            let display_offset = backend.display_offset() as i32;
            let line = -display_offset + dimensions.screen_lines as i32 / 2 - 1;
            let column =
                first_occupied_in_line(backend, dimensions, line).map_or(0, |point| point.column);
            point = Point::new(line, column);
        }
        ViMotion::Low => {
            let display_offset = backend.display_offset() as i32;
            let line = -display_offset + dimensions.screen_lines as i32 - 1;
            let column =
                first_occupied_in_line(backend, dimensions, line).map_or(0, |point| point.column);
            point = Point::new(line, column);
        }
        ViMotion::WordLeft => {
            point = word(
                backend,
                dimensions,
                point,
                SelectionSide::Left,
                SelectionSide::Left,
            );
        }
        ViMotion::WordRight => {
            point = word(
                backend,
                dimensions,
                point,
                SelectionSide::Right,
                SelectionSide::Left,
            );
        }
        ViMotion::WordRightEnd => {
            point = word(
                backend,
                dimensions,
                point,
                SelectionSide::Right,
                SelectionSide::Right,
            );
        }
        ViMotion::Bracket => {
            point = bracket_search(backend, dimensions, point).unwrap_or(point);
        }
        ViMotion::ParagraphUp => {
            // Skip empty lines until we find the next paragraph,
            // then skip over the paragraph until we reach the next empty line.
            let topmost_line = dimensions.topmost_line;
            point.line = (topmost_line..point.line)
                .rev()
                .skip_while(|&line| line_is_clear(backend, dimensions, line))
                .find(|&line| line_is_clear(backend, dimensions, line))
                .unwrap_or(topmost_line);
            point.column = 0;
        }
        ViMotion::ParagraphDown => {
            // Skip empty lines until we find the next paragraph,
            // then skip over the paragraph until we reach the next empty line.
            let bottommost_line = dimensions.bottommost_line();
            point.line = (point.line..bottommost_line)
                .skip_while(|&line| line_is_clear(backend, dimensions, line))
                .find(|&line| line_is_clear(backend, dimensions, line))
                .unwrap_or(bottommost_line);
            point.column = 0;
        }
    }

    point
}

/// Target point for vim-like page movement (alacritty
/// `ViModeCursor::scroll`).
pub(super) fn scroll(backend: &TerminalBackend, point: Point, lines: i32) -> Point {
    let dimensions = GridDimensions::of(backend);

    // Clamp movement to within visible region.
    let line = point_clamp(
        dimensions,
        PointBoundary::Grid,
        Point::new(point.line - lines, point.column),
    )
    .line;

    // Find the first occupied cell after scrolling has been performed.
    let column = first_occupied_in_line(backend, dimensions, line).map_or(0, |point| point.column);

    Point::new(line, column)
}

/// Find next end of line to move to.
fn last(backend: &TerminalBackend, dimensions: GridDimensions, mut point: Point) -> Point {
    // Expand across wide cells.
    point = backend.expand_wide(point, SelectionSide::Right);

    // Find last non-empty cell in the current line.
    let occupied =
        last_occupied_in_line(backend, dimensions, point.line).unwrap_or(Point::new(0, 0));

    if point.column < occupied.column {
        // Jump to last occupied cell when not already at or beyond it.
        occupied
    } else if is_wrap(backend, dimensions, point) {
        // Jump to last occupied cell across linewraps.
        while is_wrap(backend, dimensions, point) {
            point.line += 1;
        }

        last_occupied_in_line(backend, dimensions, point.line).unwrap_or(point)
    } else {
        // Jump to last column when beyond the last occupied cell.
        Point::new(point.line, dimensions.last_column())
    }
}

/// Find next non-empty cell to move to.
fn first_occupied(
    backend: &TerminalBackend,
    dimensions: GridDimensions,
    mut point: Point,
) -> Point {
    let last_column = dimensions.last_column();

    // Expand left across wide chars, since we're searching lines left to right.
    point = backend.expand_wide(point, SelectionSide::Left);

    // Find first non-empty cell in current line.
    let occupied = first_occupied_in_line(backend, dimensions, point.line)
        .unwrap_or(Point::new(point.line, last_column));

    // Jump across wrapped lines if we're already at this line's first occupied cell.
    if point == occupied {
        let mut occupied = None;

        // Search for non-empty cell in previous lines.
        for line in (dimensions.topmost_line..point.line).rev() {
            if !is_wrap(backend, dimensions, Point::new(line, last_column)) {
                break;
            }

            occupied = first_occupied_in_line(backend, dimensions, line).or(occupied);
        }

        // Fallback to the next non-empty cell.
        let mut line = point.line;
        occupied.unwrap_or_else(|| {
            loop {
                if let Some(occupied) = first_occupied_in_line(backend, dimensions, line) {
                    break occupied;
                }

                let last_cell = Point::new(line, last_column);
                if !is_wrap(backend, dimensions, last_cell) {
                    break last_cell;
                }

                line += 1;
            }
        })
    } else {
        occupied
    }
}

/// Move by whitespace separated word, like W/B/E in vi.
fn word(
    backend: &TerminalBackend,
    dimensions: GridDimensions,
    mut point: Point,
    direction: SelectionSide,
    side: SelectionSide,
) -> Point {
    // Make sure we jump above wide chars.
    point = backend.expand_wide(point, direction);

    if direction == side {
        // Skip whitespace until right before a word.
        let mut next_point = advance(dimensions, point, direction);
        while !is_boundary(dimensions, point, direction) && is_space(backend, next_point) {
            point = next_point;
            next_point = advance(dimensions, point, direction);
        }

        // Skip non-whitespace until right inside word boundary.
        let mut next_point = advance(dimensions, point, direction);
        while !is_boundary(dimensions, point, direction) && !is_space(backend, next_point) {
            point = next_point;
            next_point = advance(dimensions, point, direction);
        }
    }

    if direction != side {
        // Skip non-whitespace until just beyond word.
        while !is_boundary(dimensions, point, direction) && !is_space(backend, point) {
            point = advance(dimensions, point, direction);
        }

        // Skip whitespace until right inside word boundary.
        while !is_boundary(dimensions, point, direction) && is_space(backend, point) {
            point = advance(dimensions, point, direction);
        }
    }

    point
}

/// Find next matching bracket (alacritty `Term::bracket_search`, walking
/// the seam point helpers instead of a grid iterator).
fn bracket_search(
    backend: &TerminalBackend,
    dimensions: GridDimensions,
    point: Point,
) -> Option<Point> {
    let start_char = backend.character_at(point);

    // Find the matching bracket we're looking for.
    let (forward, end_char) = BRACKET_PAIRS.iter().find_map(|(open, close)| {
        if open == &start_char {
            Some((true, *close))
        } else if close == &start_char {
            Some((false, *open))
        } else {
            None
        }
    })?;

    // For every character match that equals the starting bracket, we
    // ignore one bracket of the opposite type.
    let mut skip_pairs = 0i32;
    let mut current = point;

    loop {
        // Advance one cell; the point helpers clamp at the grid edges, so
        // no progress means the walk is done.
        let next = if forward {
            point_add(dimensions, PointBoundary::Grid, current, 1)
        } else {
            point_sub(dimensions, PointBoundary::Grid, current, 1)
        };
        if next == current {
            break;
        }
        current = next;

        let character = backend.character_at(current);
        if character == end_char && skip_pairs == 0 {
            return Some(current);
        } else if character == start_char {
            skip_pairs += 1;
        } else if character == end_char {
            skip_pairs -= 1;
        }
    }

    None
}

/// Find first non-empty cell in line.
fn first_occupied_in_line(
    backend: &TerminalBackend,
    dimensions: GridDimensions,
    line: i32,
) -> Option<Point> {
    (0..dimensions.columns)
        .map(|column| Point::new(line, column))
        .find(|&point| !is_space(backend, point))
}

/// Find last non-empty cell in line.
fn last_occupied_in_line(
    backend: &TerminalBackend,
    dimensions: GridDimensions,
    line: i32,
) -> Option<Point> {
    (0..dimensions.columns)
        .map(|column| Point::new(line, column))
        .rfind(|&point| !is_space(backend, point))
}

/// Advance point based on direction.
fn advance(dimensions: GridDimensions, point: Point, direction: SelectionSide) -> Point {
    if direction == SelectionSide::Left {
        point_sub(dimensions, PointBoundary::Grid, point, 1)
    } else {
        point_add(dimensions, PointBoundary::Grid, point, 1)
    }
}

/// Check if cell at point contains whitespace.
fn is_space(backend: &TerminalBackend, point: Point) -> bool {
    !matches!(
        backend.wide_at(point),
        Some(CellWide::SpacerTail) | Some(CellWide::SpacerHead)
    ) && matches!(backend.character_at(point), ' ' | '\t')
}

/// Check if the cell at a point ends a soft-wrapped row (alacritty's
/// WRAPLINE flag lives on the last cell of the wrapping row; ghostty keeps
/// the equivalent bit on the row). Out-of-grid probes report false where
/// alacritty would panic on the grid index.
fn is_wrap(backend: &TerminalBackend, dimensions: GridDimensions, point: Point) -> bool {
    point.line >= dimensions.topmost_line
        && point.line <= dimensions.bottommost_line()
        && point.column == dimensions.last_column()
        && backend.row_wrapped(point.line)
}

/// Check if point is at screen boundary.
fn is_boundary(dimensions: GridDimensions, point: Point, direction: SelectionSide) -> bool {
    (point.line <= dimensions.topmost_line
        && point.column == 0
        && direction == SelectionSide::Left)
        || (point.line == dimensions.bottommost_line()
            && point.column + 1 >= dimensions.columns
            && direction == SelectionSide::Right)
}

/// Check if all cells in the line are empty (alacritty `Row::is_clear`
/// over `Cell::is_empty`): whitespace characters with default colors and
/// none of the emptiness-breaking attributes, on a row that doesn't wrap.
fn line_is_clear(backend: &TerminalBackend, dimensions: GridDimensions, line: i32) -> bool {
    if backend.row_wrapped(line) {
        return false;
    }
    (0..dimensions.columns).all(|column| backend.cell_is_empty(Point::new(line, column)))
}

// The test suite is seeded from alacritty's upstream `vi_mode.rs` tests at
// the pinned rev (SPEC.md §7: gap-fill components seed their suites from
// alacritty's upstream tests), scoped to the 16 motions Zed dispatches:
// the Semantic* and WordLeftEnd cases have no seam surface and are not
// ported. Grid setups feed through the parser as bytes instead of direct
// grid writes; every ported expectation is byte-identical to upstream.
// The alacritty-backend differential tests below the seeded block pin the
// scroll-follow and selection tie-ins against the live oracle.
#[cfg(test)]
mod tests {
    use super::super::tests::{content, test_backend, test_bounds};
    use super::*;
    use crate::terminal_settings::{AlternateScroll, CursorShape as SettingsCursorShape};
    use crate::{Modes, Scroll, Selection, SelectionType};

    fn alacritty_backend(
        columns: usize,
        screen_lines: usize,
        scrollback: usize,
    ) -> crate::alacritty::TerminalBackend {
        let (events_tx, _events_rx) = futures::channel::mpsc::unbounded();
        crate::alacritty::TerminalBackend::new(
            scrollback,
            SettingsCursorShape::Block,
            test_bounds(columns, screen_lines),
            events_tx,
            AlternateScroll::On,
        )
    }

    #[test]
    fn motion_simple() {
        let (backend, _events_rx) = test_backend(20, 20, 0);

        let mut cursor = Point::new(0, 0);

        cursor = motion(&backend, cursor, ViMotion::Right);
        assert_eq!(cursor, Point::new(0, 1));

        cursor = motion(&backend, cursor, ViMotion::Left);
        assert_eq!(cursor, Point::new(0, 0));

        cursor = motion(&backend, cursor, ViMotion::Down);
        assert_eq!(cursor, Point::new(1, 0));

        cursor = motion(&backend, cursor, ViMotion::Up);
        assert_eq!(cursor, Point::new(0, 0));
    }

    #[test]
    fn simple_wide() {
        let (mut backend, _events_rx) = test_backend(20, 20, 0);
        backend.write("a汉a".as_bytes());

        let cursor = motion(&backend, Point::new(0, 1), ViMotion::Right);
        assert_eq!(cursor, Point::new(0, 3));

        let cursor = motion(&backend, Point::new(0, 2), ViMotion::Left);
        assert_eq!(cursor, Point::new(0, 0));
    }

    #[test]
    fn motion_start_end() {
        let (backend, _events_rx) = test_backend(20, 20, 0);

        let mut cursor = Point::new(0, 0);

        cursor = motion(&backend, cursor, ViMotion::Last);
        assert_eq!(cursor, Point::new(0, 19));

        cursor = motion(&backend, cursor, ViMotion::First);
        assert_eq!(cursor, Point::new(0, 0));
    }

    #[test]
    fn motion_first_occupied() {
        let (mut backend, _events_rx) = test_backend(20, 20, 0);
        // Upstream builds: row 0 " x y" + WRAPLINE, row 1 blank + WRAPLINE,
        // row 2 "z ". A 3-row soft-wrapped logical line reproduces it.
        let mut input = String::from(" x y");
        input.push_str(&" ".repeat(36));
        input.push_str("z ");
        backend.write(input.as_bytes());

        let mut cursor = Point::new(2, 1);

        cursor = motion(&backend, cursor, ViMotion::FirstOccupied);
        assert_eq!(cursor, Point::new(2, 0));

        cursor = motion(&backend, cursor, ViMotion::FirstOccupied);
        assert_eq!(cursor, Point::new(0, 1));
    }

    #[test]
    fn motion_high_middle_low() {
        let (backend, _events_rx) = test_backend(20, 20, 0);

        let mut cursor = Point::new(0, 0);

        cursor = motion(&backend, cursor, ViMotion::High);
        assert_eq!(cursor, Point::new(0, 0));

        cursor = motion(&backend, cursor, ViMotion::Middle);
        assert_eq!(cursor, Point::new(9, 0));

        cursor = motion(&backend, cursor, ViMotion::Low);
        assert_eq!(cursor, Point::new(19, 0));
    }

    #[test]
    fn motion_bracket() {
        let (mut backend, _events_rx) = test_backend(20, 20, 0);
        backend.write(b"(x)");

        let mut cursor = Point::new(0, 0);

        cursor = motion(&backend, cursor, ViMotion::Bracket);
        assert_eq!(cursor, Point::new(0, 2));

        cursor = motion(&backend, cursor, ViMotion::Bracket);
        assert_eq!(cursor, Point::new(0, 0));
    }

    #[test]
    fn motion_word() {
        let (mut backend, _events_rx) = test_backend(20, 20, 0);
        backend.write(b"a;  a;");

        let mut cursor = Point::new(0, 0);

        cursor = motion(&backend, cursor, ViMotion::WordRightEnd);
        assert_eq!(cursor, Point::new(0, 1));

        cursor = motion(&backend, cursor, ViMotion::WordRightEnd);
        assert_eq!(cursor, Point::new(0, 5));

        cursor = motion(&backend, cursor, ViMotion::WordLeft);
        assert_eq!(cursor, Point::new(0, 4));

        cursor = motion(&backend, cursor, ViMotion::WordLeft);
        assert_eq!(cursor, Point::new(0, 0));

        cursor = motion(&backend, cursor, ViMotion::WordRight);
        assert_eq!(cursor, Point::new(0, 4));

        // Upstream ends with a WordLeftEnd assertion; that motion is not
        // dispatched by Zed and has no seam surface.
    }

    #[test]
    fn word_wide() {
        let (mut backend, _events_rx) = test_backend(20, 20, 0);
        backend.write("a 汉 a".as_bytes());

        let cursor = motion(&backend, Point::new(0, 2), ViMotion::WordRight);
        assert_eq!(cursor, Point::new(0, 5));

        let cursor = motion(&backend, Point::new(0, 3), ViMotion::WordLeft);
        assert_eq!(cursor, Point::new(0, 0));
    }

    #[test]
    fn scroll_word() {
        let (mut backend, _events_rx) = test_backend(20, 20, 100);
        // Upstream scrolls 5 lines into history; 24 newlines produce the
        // same 5 scrollback lines on a 20-row screen.
        backend.write(&b"\r\n".repeat(24));
        backend.toggle_vi_mode();
        backend.vi_goto_point(Point::new(0, 0));

        backend.vi_motion(ViMotion::WordLeft);
        assert_eq!(content(&mut backend).cursor.point, Point::new(-5, 0));
        assert_eq!(backend.display_offset(), 5);

        backend.vi_motion(ViMotion::WordRight);
        assert_eq!(content(&mut backend).cursor.point, Point::new(19, 19));
        assert_eq!(backend.display_offset(), 0);

        // Upstream's WordLeftEnd leg is not dispatched by Zed; re-approach
        // the top with WordLeft so the WordRightEnd expectation stays
        // byte-identical.
        backend.vi_motion(ViMotion::WordLeft);
        assert_eq!(content(&mut backend).cursor.point, Point::new(-5, 0));
        assert_eq!(backend.display_offset(), 5);

        backend.vi_motion(ViMotion::WordRightEnd);
        assert_eq!(content(&mut backend).cursor.point, Point::new(19, 19));
        assert_eq!(backend.display_offset(), 0);
    }

    #[test]
    fn scroll_simple() {
        let (mut backend, _events_rx) = test_backend(20, 20, 100);
        // Create 1 line of scrollback.
        backend.write(&b"\r\n".repeat(20));

        let mut cursor = Point::new(0, 0);

        cursor = scroll(&backend, cursor, -1);
        assert_eq!(cursor, Point::new(1, 0));

        cursor = scroll(&backend, cursor, 1);
        assert_eq!(cursor, Point::new(0, 0));

        cursor = scroll(&backend, cursor, 1);
        assert_eq!(cursor, Point::new(-1, 0));
    }

    #[test]
    fn scroll_over_top() {
        let (mut backend, _events_rx) = test_backend(20, 20, 100);
        // Create 40 lines of scrollback.
        backend.write(&b"\r\n".repeat(59));

        let mut cursor = Point::new(19, 0);

        cursor = scroll(&backend, cursor, 20);
        assert_eq!(cursor, Point::new(-1, 0));

        cursor = scroll(&backend, cursor, 20);
        assert_eq!(cursor, Point::new(-21, 0));

        cursor = scroll(&backend, cursor, 20);
        assert_eq!(cursor, Point::new(-40, 0));

        cursor = scroll(&backend, cursor, 20);
        assert_eq!(cursor, Point::new(-40, 0));
    }

    #[test]
    fn scroll_over_bottom() {
        let (mut backend, _events_rx) = test_backend(20, 20, 100);
        // Create 40 lines of scrollback.
        backend.write(&b"\r\n".repeat(59));

        let mut cursor = Point::new(-40, 0);

        cursor = scroll(&backend, cursor, -20);
        assert_eq!(cursor, Point::new(-20, 0));

        cursor = scroll(&backend, cursor, -20);
        assert_eq!(cursor, Point::new(0, 0));

        cursor = scroll(&backend, cursor, -20);
        assert_eq!(cursor, Point::new(19, 0));

        cursor = scroll(&backend, cursor, -20);
        assert_eq!(cursor, Point::new(19, 0));
    }

    // Differential tests: the alacritty backend is the live oracle for the
    // backend-level tie-ins (SPEC.md §7) — toggle initialization, the
    // motion scroll-follow, and both selection couplings.

    const DIFFERENTIAL_INPUT: &[u8] = b"alpha beta (gamma delta)\r\n\
        epsilon\r\n\
        \r\n\
        zeta { eta } theta\r\n\
        iota kappa\r\n\
        lambda mu\r\n\
        nu xi omicron\r\n\
        prompt> ";

    /// Both backends fed the differential input with vi mode already on.
    fn differential_pair() -> (
        TerminalBackend,
        futures::channel::mpsc::UnboundedReceiver<crate::PtyEvent>,
        crate::alacritty::TerminalBackend,
    ) {
        let (mut ghostty, ghostty_rx) = test_backend(20, 5, 100);
        let mut alacritty = alacritty_backend(20, 5, 100);
        ghostty.write(DIFFERENTIAL_INPUT);
        alacritty.write(DIFFERENTIAL_INPUT);
        ghostty.toggle_vi_mode();
        alacritty.toggle_vi_mode();
        (ghostty, ghostty_rx, alacritty)
    }

    #[test]
    fn toggle_vi_mode_matches_alacritty() {
        let (mut ghostty, _ghostty_rx) = test_backend(20, 5, 100);
        let mut alacritty = alacritty_backend(20, 5, 100);
        ghostty.write(DIFFERENTIAL_INPUT);
        alacritty.write(DIFFERENTIAL_INPUT);

        assert!(!ghostty.modes().contains(Modes::VI));

        // Toggling at the bottom starts the vi cursor on the primary cursor.
        ghostty.toggle_vi_mode();
        alacritty.toggle_vi_mode();
        let ghostty_content = content(&mut ghostty);
        let alacritty_content = alacritty.make_content(&crate::Content::default());
        assert!(ghostty_content.mode.contains(Modes::VI));
        assert!(alacritty_content.mode.contains(Modes::VI));
        assert_eq!(ghostty_content.cursor.point, alacritty_content.cursor.point);
        assert_eq!(ghostty_content.cursor.shape, alacritty_content.cursor.shape);

        // Toggling off clears the synthesized mode on both.
        ghostty.toggle_vi_mode();
        alacritty.toggle_vi_mode();
        assert!(!ghostty.modes().contains(Modes::VI));
        assert!(
            !alacritty
                .make_content(&crate::Content::default())
                .mode
                .contains(Modes::VI)
        );

        // Toggling while scrolled back (primary cursor off-screen) starts
        // the vi cursor at the viewport's top-left.
        ghostty.scroll_display(Scroll::Top);
        alacritty.scroll_display(Scroll::Top);
        ghostty.toggle_vi_mode();
        alacritty.toggle_vi_mode();
        let ghostty_content = content(&mut ghostty);
        let alacritty_content = alacritty.make_content(&crate::Content::default());
        assert_eq!(ghostty_content.cursor.point, alacritty_content.cursor.point);
        assert!(ghostty_content.cursor.point.line < 0);
    }

    #[test]
    fn vi_motion_script_matches_alacritty() {
        use ViMotion::*;
        let script = [
            Up, Up, WordLeft, WordLeft, First, Bracket, WordRightEnd, Right, Right, Down, Last,
            FirstOccupied, High, ParagraphUp, ParagraphDown, Middle, Low, Left, WordRight,
            Bracket, Up, First, ParagraphUp, ParagraphUp, High, WordLeft,
        ];

        let (mut ghostty, _ghostty_rx, mut alacritty) = differential_pair();

        for (index, vi_motion) in script.into_iter().enumerate() {
            ghostty.vi_motion(vi_motion);
            alacritty.vi_motion(vi_motion);
            let ghostty_content = content(&mut ghostty);
            let alacritty_content = alacritty.make_content(&crate::Content::default());
            assert_eq!(
                ghostty_content.cursor.point, alacritty_content.cursor.point,
                "step {index} ({vi_motion:?}): vi cursor should match alacritty"
            );
            assert_eq!(
                ghostty.display_offset(),
                alacritty.display_offset(),
                "step {index} ({vi_motion:?}): scroll-follow should match alacritty"
            );
        }
    }

    #[test]
    fn vi_motion_drags_selection_matching_alacritty() {
        let (mut ghostty, _ghostty_rx, mut alacritty) = differential_pair();
        ghostty.vi_goto_point(Point::new(1, 3));
        alacritty.vi_goto_point(Point::new(1, 3));

        let mut selection =
            Selection::new(SelectionType::Simple, Point::new(0, 2), SelectionSide::Left);
        selection.update(Point::new(0, 6), SelectionSide::Right);
        ghostty.set_selection(Some(&selection));
        alacritty.set_selection(Some(&selection));

        use ViMotion::*;
        for (index, vi_motion) in [WordRight, Down, Last, WordLeft, Up, Up, First, Low]
            .into_iter()
            .enumerate()
        {
            ghostty.vi_motion(vi_motion);
            alacritty.vi_motion(vi_motion);
            let ghostty_content = content(&mut ghostty);
            let alacritty_content = alacritty.make_content(&crate::Content::default());
            assert_eq!(
                ghostty_content.selection, alacritty_content.selection,
                "step {index} ({vi_motion:?}): dragged selection should match alacritty"
            );
            assert_eq!(
                ghostty_content.selection_text, alacritty_content.selection_text,
                "step {index} ({vi_motion:?}): dragged selection text should match alacritty"
            );
            assert_eq!(
                ghostty_content.cursor.point, alacritty_content.cursor.point,
                "step {index} ({vi_motion:?}): vi cursor should match alacritty"
            );
        }
    }

    #[test]
    fn vi_motion_leaves_empty_selection_alone_matching_alacritty() {
        let (mut ghostty, _ghostty_rx, mut alacritty) = differential_pair();

        // An empty selection (identical anchors) does not follow motions.
        let selection =
            Selection::new(SelectionType::Simple, Point::new(1, 3), SelectionSide::Left);
        ghostty.set_selection(Some(&selection));
        alacritty.set_selection(Some(&selection));

        ghostty.vi_motion(ViMotion::WordRight);
        alacritty.vi_motion(ViMotion::WordRight);
        let ghostty_content = content(&mut ghostty);
        let alacritty_content = alacritty.make_content(&crate::Content::default());
        assert_eq!(ghostty_content.selection, alacritty_content.selection);
        assert_eq!(ghostty_content.selection, None);
    }

    #[test]
    fn scroll_flow_matches_alacritty() {
        let (mut ghostty, _ghostty_rx, mut alacritty) = differential_pair();

        let mut selection =
            Selection::new(SelectionType::Simple, Point::new(2, 0), SelectionSide::Left);
        selection.update(Point::new(2, 4), SelectionSide::Right);
        ghostty.set_selection(Some(&selection));
        alacritty.set_selection(Some(&selection));

        // The InternalEvent::Scroll flow: scroll the display, move the vi
        // cursor with it, then drag the selection to the vi cursor.
        for (index, scroll) in [
            Scroll::Delta(2),
            Scroll::PageDown,
            Scroll::Top,
            Scroll::Delta(-1),
            Scroll::PageUp,
            Scroll::Bottom,
        ]
        .into_iter()
        .enumerate()
        {
            ghostty.scroll_display(scroll);
            alacritty.scroll_display(scroll);
            ghostty.update_vi_cursor_for_scroll(scroll);
            alacritty.update_vi_cursor_for_scroll(scroll);
            let ghostty_head = ghostty.update_selection_to_vi_cursor();
            let alacritty_head = alacritty.update_selection_to_vi_cursor();
            assert_eq!(
                ghostty_head, alacritty_head,
                "step {index} ({scroll:?}): selection head should match alacritty"
            );
            let ghostty_content = content(&mut ghostty);
            let alacritty_content = alacritty.make_content(&crate::Content::default());
            assert_eq!(
                ghostty_content.cursor.point, alacritty_content.cursor.point,
                "step {index} ({scroll:?}): vi cursor should match alacritty"
            );
            assert_eq!(
                ghostty_content.selection, alacritty_content.selection,
                "step {index} ({scroll:?}): selection should match alacritty"
            );
        }
    }

    #[test]
    fn vi_goto_point_scrolls_to_reveal_matching_alacritty() {
        let (mut ghostty, _ghostty_rx, mut alacritty) = differential_pair();

        ghostty.vi_goto_point(Point::new(-2, 3));
        alacritty.vi_goto_point(Point::new(-2, 3));
        assert_eq!(
            content(&mut ghostty).cursor.point,
            alacritty.make_content(&crate::Content::default()).cursor.point
        );
        assert_eq!(ghostty.display_offset(), alacritty.display_offset());
        assert!(ghostty.display_offset() >= 2);

        ghostty.vi_goto_point(Point::new(4, 0));
        alacritty.vi_goto_point(Point::new(4, 0));
        assert_eq!(
            content(&mut ghostty).cursor.point,
            alacritty.make_content(&crate::Content::default()).cursor.point
        );
        assert_eq!(ghostty.display_offset(), alacritty.display_offset());
        assert_eq!(ghostty.display_offset(), 0);
    }
}
