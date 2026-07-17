//! The ghostty-backed terminal core (SPEC.md §4, §6 P5), built *dark*:
//! compiled and tested on Linux, unused by production until the P8 swap.
//! It mirrors `alacritty::TerminalBackend`'s inherent method surface so the
//! swap only re-points one import and the construction call. The P6 gap
//! fills (grid search, hover pipeline, vi mode) extend this surface.

mod grid_search;
mod hyperlinks;

pub(super) use hyperlinks::{HyperlinkMatch, RegexSearches};

use std::{
    cell::{Cell as StdCell, RefCell},
    collections::VecDeque,
    rc::Rc,
    sync::Arc,
};

use futures::channel::mpsc::UnboundedSender;
use ghostty_vt::{
    RenderState, Terminal as GhosttyTerminal, TerminalOptions,
    error::Error as GhosttyError,
    fmt::Format,
    render::{CellIterator, CursorVisualStyle, Dirty, RowIterator},
    screen::{CellWide, Screen, TrackedGridRef},
    selection::{
        FormatOptions, SelectLineOptions, SelectWordOptions, Selection as GhosttySelection,
    },
    style::{Palette, PaletteIndex, RgbColor, StyleColor, Underline},
    terminal::{
        ColorScheme, CompressionMode, CompressionResult, ConformanceLevel,
        CursorStyle as GhosttyCursorStyle, DeviceAttributes, DeviceType, Mode,
        Point as GhosttyPoint, PointCoordinate, PointSpace, PrimaryDeviceAttributes,
        ScrollViewport, SecondaryDeviceAttributes, SizeReportSize, TertiaryDeviceAttributes,
    },
};
use util::{ResultExt, paths::PathStyle};

use crate::{
    Cell, CellExtra, CellFlags, Color, Content, Cursor, CursorShape, Hyperlink, IndexedCell,
    Modes, NamedColor, Point, PtyEvent, Range, Rgb, Scroll, Selection, SelectionRange,
    SelectionSide, SelectionType, TerminalBackendEvent, TerminalBounds,
    terminal_settings::{AlternateScroll, CursorShape as SettingsCursorShape},
};

/// Alacritty's default semantic-word boundary set, passed to ghostty's word
/// selection per call (SPEC.md §4.2; parity-matrix row 76). Kept as a local
/// constant so this file survives the alacritty removal at P10.
const SEMANTIC_ESCAPE_CHARS: &[char] = &[
    ',', '│', '`', '|', ':', '"', '\'', ' ', '(', ')', '[', ']', '{', '}', '<', '>', '\t',
];

/// Primary device attributes reported for CSI c: a plain VT102, matching the
/// `\x1b[?6c` response of the alacritty-era core.
const PRIMARY_DEVICE_ATTRIBUTES: PrimaryDeviceAttributes =
    PrimaryDeviceAttributes::new(ConformanceLevel::VT102, &[]);

/// The 15 directly-mapped `Modes` flags rebuilt from typed getters per
/// snapshot (parity matrix §K). `ALT_SCREEN` comes from `active_screen()`;
/// `VI` is synthesized from Zed-side vi state.
const MODE_MAP: [(Mode, Modes); 15] = [
    (Mode::DECCKM, Modes::APP_CURSOR),
    (Mode::KEYPAD_KEYS, Modes::APP_KEYPAD),
    (Mode::CURSOR_VISIBLE, Modes::SHOW_CURSOR),
    (Mode::WRAPAROUND, Modes::LINE_WRAP),
    (Mode::ORIGIN, Modes::ORIGIN),
    (Mode::INSERT, Modes::INSERT),
    (Mode::LINEFEED, Modes::LINE_FEED_NEW_LINE),
    (Mode::FOCUS_EVENT, Modes::FOCUS_IN_OUT),
    (Mode::ALT_SCROLL, Modes::ALTERNATE_SCROLL),
    (Mode::BRACKETED_PASTE, Modes::BRACKETED_PASTE),
    (Mode::SGR_MOUSE, Modes::SGR_MOUSE),
    (Mode::UTF8_MOUSE, Modes::UTF8_MOUSE),
    (Mode::NORMAL_MOUSE, Modes::MOUSE_REPORT_CLICK),
    (Mode::BUTTON_MOUSE, Modes::MOUSE_DRAG),
    (Mode::ANY_MOUSE, Modes::MOUSE_MOTION),
];

/// Ghostty's `max_scrollback` is a byte limit over its page storage
/// (rounded up to whole 512 KiB pages), not the line count its header
/// documents; a standard page holds 215 rows at up to 215 columns. Convert
/// Zed's line-count setting into whole standard pages so at least that many
/// lines are retained in the common case. Zero stays zero: ghostty disables
/// scrollback entirely. The cap is therefore page-granular rather than an
/// exact line count (divergence ledger P5-001).
fn scrollback_bytes_for_lines(lines: usize) -> usize {
    const GHOSTTY_STD_PAGE_BYTES: usize = 512 * 1024;
    const GHOSTTY_STD_PAGE_ROWS: usize = 215;
    lines.div_ceil(GHOSTTY_STD_PAGE_ROWS) * GHOSTTY_STD_PAGE_BYTES
}

/// A selection tracked in the seam: type and cell sides are Zed-side state
/// (ghostty has no side concept), while the two anchor cells ride ghostty
/// tracked grid refs so they follow content through scrolling and reflow the
/// same way alacritty rotates its selection.
struct SeamSelection {
    ty: SelectionType,
    start: TrackedGridRef,
    start_side: SelectionSide,
    end: TrackedGridRef,
    end_side: SelectionSide,
}

/// Iterator over owned viewport cells; the ghostty analogue of the alacritty
/// grid iterator behind `crate::RenderableCells`, which re-points here at P8.
pub(super) struct RenderableCells(std::vec::IntoIter<IndexedCell>);

impl Iterator for RenderableCells {
    type Item = IndexedCell;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

/// The ghostty-backed terminal core: the `!Send` emulator, its render state,
/// and the callback plumbing, owned by the foreground thread with no locks
/// (SPEC.md §3 D2). Mirrors `alacritty::TerminalBackend`'s inherent surface.
pub(super) struct TerminalBackend {
    terminal: GhosttyTerminal<'static, 'static>,
    render_state: RenderState<'static>,
    row_iterator: RowIterator<'static>,
    cell_iterator: CellIterator<'static>,
    /// Callbacks fire synchronously inside `vt_write`; they queue here and
    /// drain FIFO right after each write, preserving PTY-response ordering
    /// (SPEC.md §3 D3).
    events: Rc<RefCell<VecDeque<TerminalBackendEvent>>>,
    events_tx: UnboundedSender<PtyEvent>,
    /// Answers XTWINOPS size queries synchronously from the last known bounds.
    bounds: Rc<StdCell<TerminalBounds>>,
    /// Answers CSI ?996n color-scheme queries synchronously.
    color_scheme: Rc<StdCell<ColorScheme>>,
    /// `scrollbar()` is documented as potentially expensive, so the derived
    /// offset-from-bottom is cached and refreshed only on scroll, resize,
    /// clear, or a dirty-full frame (SPEC.md §4.2 S3).
    display_offset: StdCell<Option<usize>>,
    selection: Option<SeamSelection>,
    /// Zed-side vi-mode state surfaced as `Modes::VI`; driven by the P6 vi
    /// port, always `false` until then.
    vi_mode: bool,
    prev_cursor_blinking: Option<bool>,
    prev_mouse_mode: Option<bool>,
}

impl TerminalBackend {
    pub(super) fn new(
        scrolling_history: usize,
        cursor_shape: SettingsCursorShape,
        bounds: TerminalBounds,
        events_tx: UnboundedSender<PtyEvent>,
        alternate_scroll: AlternateScroll,
    ) -> Self {
        Self::with_options(
            scrolling_history,
            cursor_shape,
            bounds,
            events_tx,
            alternate_scroll,
            false,
        )
    }

    pub(super) fn new_display_only(
        scrolling_history: usize,
        cursor_shape: SettingsCursorShape,
        bounds: TerminalBounds,
        events_tx: UnboundedSender<PtyEvent>,
        alternate_scroll: AlternateScroll,
    ) -> Self {
        Self::with_options(
            scrolling_history,
            cursor_shape,
            bounds,
            events_tx,
            alternate_scroll,
            true,
        )
    }

    fn with_options(
        scrolling_history: usize,
        cursor_shape: SettingsCursorShape,
        bounds: TerminalBounds,
        events_tx: UnboundedSender<PtyEvent>,
        alternate_scroll: AlternateScroll,
        display_only: bool,
    ) -> Self {
        // The `expect`s below fire only on allocation failure or an
        // internally inconsistent library, neither of which the infallible
        // backend construction contract (shared with the alacritty wrapper)
        // can surface.
        let mut terminal = GhosttyTerminal::new(TerminalOptions {
            cols: bounds.num_columns().max(1) as u16,
            rows: bounds.num_lines().max(1) as u16,
            max_scrollback: scrollback_bytes_for_lines(scrolling_history),
        })
        .expect("failed to allocate the ghostty terminal");

        let events = Rc::new(RefCell::new(VecDeque::new()));
        let shared_bounds = Rc::new(StdCell::new(bounds));
        let color_scheme = Rc::new(StdCell::new(ColorScheme::Dark));
        register_callbacks(
            &mut terminal,
            &events,
            &shared_bounds,
            &color_scheme,
            display_only,
        )
        .expect("failed to register ghostty terminal callbacks");

        // Terminal creation contract (SPEC.md §3): scrollback is set above at
        // creation time only; ALT_SCROLL parity with today's
        // `unset_private_mode`; Zed does not render kitty images.
        if let AlternateScroll::Off = alternate_scroll {
            terminal.set_mode(Mode::ALT_SCROLL, false).log_err();
        }
        terminal.set_kitty_image_storage_limit(0).log_err();
        terminal
            .set_default_cursor_style(Some(ghostty_cursor_style(cursor_shape)))
            .log_err();
        terminal.set_default_cursor_blink(Some(false)).log_err();

        Self {
            terminal,
            render_state: RenderState::new().expect("failed to allocate the ghostty render state"),
            row_iterator: RowIterator::new().expect("failed to allocate the ghostty row iterator"),
            cell_iterator: CellIterator::new()
                .expect("failed to allocate the ghostty cell iterator"),
            events,
            events_tx,
            bounds: shared_bounds,
            color_scheme,
            display_offset: StdCell::new(None),
            selection: None,
            vi_mode: false,
            prev_cursor_blinking: None,
            prev_mouse_mode: None,
        }
    }

    pub(super) fn write(&mut self, bytes: &[u8]) {
        self.terminal.vt_write(bytes);
        self.drain_events();
    }

    pub(super) fn set_default_cursor_style(&mut self, cursor_shape: SettingsCursorShape) {
        self.terminal
            .set_default_cursor_style(Some(ghostty_cursor_style(cursor_shape)))
            .log_err();
    }

    /// Push the theme's colors into ghostty as embedder defaults, at creation
    /// and on every theme change (SPEC.md §4.2). Ghostty's setters preserve
    /// OSC overrides, and it answers OSC 4/10/11/12 queries internally.
    pub(super) fn push_theme_colors(
        &mut self,
        foreground: Rgb,
        background: Rgb,
        cursor: Rgb,
        palette: &[Rgb; 256],
    ) {
        self.terminal
            .set_default_fg_color(Some(ghostty_rgb(foreground)))
            .and_then(|terminal| terminal.set_default_bg_color(Some(ghostty_rgb(background))))
            .and_then(|terminal| terminal.set_default_cursor_color(Some(ghostty_rgb(cursor))))
            .and_then(|terminal| {
                terminal.set_default_color_palette(Some(Palette(palette.map(ghostty_rgb))))
            })
            .log_err();
    }

    /// Set the color scheme reported for CSI ?996n queries; wired to the
    /// theme's appearance at the P8 swap.
    pub(super) fn set_color_scheme(&mut self, dark: bool) {
        self.color_scheme.set(if dark {
            ColorScheme::Dark
        } else {
            ColorScheme::Light
        });
    }

    pub(super) fn make_content(&mut self, last_content: &Content) -> Content {
        match self.try_make_content(last_content) {
            Ok(content) => content,
            Err(error) => {
                log::error!("ghostty backend failed to build a content snapshot: {error}");
                last_content.clone()
            }
        }
    }

    pub(super) fn with_renderable_cells<R>(
        &mut self,
        f: impl FnOnce(RenderableCells) -> R,
    ) -> R {
        let cells = match self.collect_viewport_cells() {
            Ok(collected) => {
                let display_offset = self.display_offset() as i32;
                collected
                    .cells
                    .into_iter()
                    .map(|cell| indexed_cell(cell, display_offset))
                    .collect()
            }
            Err(error) => {
                log::error!("ghostty backend failed to collect renderable cells: {error}");
                Vec::new()
            }
        };
        f(RenderableCells(cells.into_iter()))
    }

    pub(super) fn total_lines(&self) -> usize {
        self.terminal.total_rows().log_err().unwrap_or(1)
    }

    pub(super) fn screen_lines(&self) -> usize {
        self.terminal.rows().log_err().unwrap_or(1) as usize
    }

    fn columns(&self) -> usize {
        self.terminal.cols().log_err().unwrap_or(1) as usize
    }

    fn scrollback_lines(&self) -> usize {
        self.terminal.scrollback_rows().log_err().unwrap_or(0)
    }

    pub(super) fn display_offset(&self) -> usize {
        if let Some(display_offset) = self.display_offset.get() {
            return display_offset;
        }
        let display_offset = match self.terminal.scrollbar() {
            // The scrollbar reports the viewport offset from the top of the
            // scrollable area; Zed speaks offset-from-bottom.
            Ok(scrollbar) => scrollbar
                .total
                .saturating_sub(scrollbar.offset + scrollbar.len) as usize,
            Err(error) => {
                log::error!("ghostty backend failed to read the scrollbar: {error}");
                0
            }
        };
        self.display_offset.set(Some(display_offset));
        display_offset
    }

    pub(super) fn content_text(&self) -> String {
        self.format_range_text(GridDimensions::of(self).full_range())
            .unwrap_or_default()
    }

    pub(super) fn full_content_range(&self) -> Range {
        GridDimensions::of(self).full_range()
    }

    pub(super) fn last_n_non_empty_lines(&self, line_count: usize) -> Vec<String> {
        let total_lines = self.total_lines();
        let columns = self.columns();
        let mut lines = Vec::new();

        let mut current_line = total_lines as i64 - 1;
        while current_line >= 0 && lines.len() < line_count {
            let logical_line_start = self.find_logical_line_start(current_line);
            let mut line = String::new();
            for row in logical_line_start..=current_line {
                self.push_row_text(&mut line, row as u32, columns);
            }

            let trimmed = line.trim_end();
            if !trimmed.is_empty() {
                lines.push(trimmed.to_string());
            }

            current_line = logical_line_start - 1;
        }

        lines.reverse();
        lines
    }

    pub(super) fn cursor_blinking(&mut self) -> bool {
        self.render_state
            .update(&self.terminal)
            .and_then(|snapshot| snapshot.cursor_blinking())
            .log_err()
            .unwrap_or(false)
    }

    /// The effective (OSC override or embedder default) color at an index in
    /// alacritty's color-table layout: 0–255 palette, 256 foreground,
    /// 257 background, 258 cursor.
    pub(super) fn color(&self, index: usize) -> Option<Rgb> {
        let color = match index {
            0..=255 => Some(self.terminal.color_palette().log_err()?.0[index]),
            256 => self.terminal.fg_color().log_err()?,
            257 => self.terminal.bg_color().log_err()?,
            258 => self.terminal.cursor_color().log_err()?,
            _ => None,
        };
        color.map(zed_rgb)
    }

    pub(super) fn resize(&mut self, bounds: TerminalBounds) {
        self.bounds.set(bounds);
        self.terminal
            .resize(
                bounds.num_columns().max(1) as u16,
                bounds.num_lines().max(1) as u16,
                f32::from(bounds.cell_width()) as u32,
                f32::from(bounds.line_height()) as u32,
            )
            .log_err();
        // Parity: alacritty's resize drops the active selection.
        self.selection = None;
        self.terminal.set_selection(None).log_err();
        self.display_offset.set(None);
    }

    pub(super) fn scroll_display(&mut self, scroll: Scroll) {
        // Scroll delta signs invert in the seam (SPEC.md §4.2 S3): positive
        // alacritty deltas scroll toward history, ghostty's "up" is negative.
        let scroll_viewport = match scroll {
            Scroll::Delta(delta) => ScrollViewport::Delta(-(delta as isize)),
            Scroll::PageUp => ScrollViewport::Delta(-(self.screen_lines() as isize)),
            Scroll::PageDown => ScrollViewport::Delta(self.screen_lines() as isize),
            Scroll::Top => ScrollViewport::Top,
            Scroll::Bottom => ScrollViewport::Bottom,
        };
        self.terminal.scroll_viewport(scroll_viewport);
        self.display_offset.set(None);
    }

    pub(super) fn scroll_to_point(&mut self, point: Point) {
        let display_offset = self.display_offset() as i32;
        let screen_lines = self.screen_lines() as i32;

        if point.line < -display_offset {
            let lines = point.line + display_offset;
            self.scroll_display(Scroll::Delta(-lines));
        } else if point.line >= screen_lines - display_offset {
            let lines = point.line + display_offset - screen_lines + 1;
            self.scroll_display(Scroll::Delta(-lines));
        }
    }

    /// The G5 clear (SPEC.md §4.5): VT-sequence emulation preserving the
    /// prompt line, written seam-locally (never to the PTY). Scroll the
    /// cursor's row to the top, re-home the cursor on it, erase below, and
    /// erase scrollback last so the scrolled-in rows die too.
    pub(super) fn clear(&mut self) {
        let cursor_row = self.terminal.cursor_y().log_err().unwrap_or(0);
        let cursor_column = self.terminal.cursor_x().log_err().unwrap_or(0);

        let mut sequence = Vec::new();
        // SU treats a 0 parameter as 1, so it must be skipped entirely when
        // the cursor is already on the top row.
        if cursor_row > 0 {
            sequence.extend_from_slice(format!("\x1b[{cursor_row}S").as_bytes());
        }
        sequence.extend_from_slice(format!("\x1b[1;{}H", cursor_column + 1).as_bytes());
        sequence.extend_from_slice(b"\x1b[0J\x1b[3J");

        self.write(&sequence);
        self.display_offset.set(None);
    }

    /// Reclaim over-allocated scrollback storage: the compress-until-done
    /// loop over ghostty's incremental `compress()` (SPEC.md §4.5,
    /// ex-`shrink_to_used`).
    pub(super) fn shrink_to_used(&mut self) {
        loop {
            match self.terminal.compress(CompressionMode::Incremental) {
                Ok(CompressionResult::Pending) => {}
                Ok(_) => break,
                Err(error) => {
                    log::error!("ghostty backend failed to compress scrollback: {error}");
                    break;
                }
            }
        }
    }

    /// Appends a stringified task summary to the terminal, after its output.
    /// A plain `vt_write` through the single ingest point; unlike the
    /// alacritty version this leaves the emulator state fully consistent
    /// (SPEC.md §4.5, ex-`append_text_to_term`).
    pub(super) fn append_lines(&mut self, text_lines: &[&str]) {
        let mut bytes = b"\r\n".to_vec();
        for line in text_lines {
            bytes.extend_from_slice(line.as_bytes());
            bytes.extend_from_slice(b"\r\n");
        }
        self.write(&bytes);
    }

    pub(super) fn set_selection(&mut self, selection: Option<&Selection>) {
        self.selection = selection.and_then(|selection| {
            let start = self.track_grid_point(selection.start.point)?;
            let end = self.track_grid_point(selection.end.point)?;
            Some(SeamSelection {
                ty: selection.ty,
                start,
                start_side: selection.start.side,
                end,
                end_side: selection.end.side,
            })
        });
        self.sync_selection_to_terminal();
    }

    pub(super) fn update_selection(&mut self, point: Point, side: SelectionSide) -> bool {
        let point = self.ghostty_screen_point(point);
        let Some(selection) = self.selection.as_mut() else {
            return false;
        };
        selection.end.set(&mut self.terminal, point).log_err();
        selection.end_side = side;
        self.sync_selection_to_terminal();
        true
    }

    pub(super) fn selection_text(&self) -> Option<String> {
        self.sync_selection_to_terminal()?;
        self.active_selection_text()
    }

    /// Format the (already synced) active selection, mirroring alacritty's
    /// `selection_to_string`, including its trailing newline for Lines
    /// selections.
    fn active_selection_text(&self) -> Option<String> {
        let mut text = self.format_active_selection()?;
        if self
            .selection
            .as_ref()
            .is_some_and(|selection| selection.ty == SelectionType::Lines)
        {
            text.push('\n');
        }
        Some(text)
    }

    /// The per-snapshot `Modes` bitfield rebuilt from typed getters
    /// (parity matrix §K).
    pub(super) fn modes(&self) -> Modes {
        let mut modes = Modes::empty();
        for (ghostty_mode, mode) in MODE_MAP {
            if self
                .terminal
                .mode(ghostty_mode)
                .log_err()
                .unwrap_or(false)
            {
                modes.insert(mode);
            }
        }
        if self.terminal.active_screen().log_err() == Some(Screen::Alternate) {
            modes.insert(Modes::ALT_SCREEN);
        }
        if self.vi_mode {
            modes.insert(Modes::VI);
        }
        modes
    }

    /// The foreground extract stage of the `find_matches` sandwich
    /// (SPEC.md §4.4): one bulk buffer extract plus the wrap-flag line map,
    /// packaged with the query so the regex stage can leave the thread.
    pub(super) fn prepare_search(&self, query: grid_search::SearchQuery) -> grid_search::PreparedSearch {
        grid_search::PreparedSearch::new(query, grid_search::extract_logical_lines(&self.terminal))
    }

    /// The foreground map stage of the `find_matches` sandwich: byte↔cell
    /// walks over match rows only, with matches leaving Screen space for the
    /// grid convention at this exit (S3).
    pub(super) fn search_matches(&self, found: grid_search::FoundMatches) -> Vec<Range> {
        grid_search::resolve_matches(&self.terminal, found)
    }

    /// Hover-to-detect links (SPEC.md §4.4): the ported hyperlinks pipeline
    /// over this backend's grid, mirroring
    /// `alacritty::TerminalBackend::find_from_terminal_point`.
    pub(super) fn find_from_terminal_point(
        &self,
        point: Point,
        regex_searches: &mut RegexSearches,
        path_style: PathStyle,
    ) -> Option<HyperlinkMatch> {
        let point = point_clamp(GridDimensions::of(self), PointBoundary::Grid, point);
        hyperlinks::find_from_grid_point(self, point, regex_searches, path_style)
    }

    /// Jump to the far cell of a wide character, mirroring alacritty's
    /// `Term::expand_wide` over ghostty's `wide()` cell property (G6).
    pub(super) fn expand_wide(&self, mut point: Point, side: SelectionSide) -> Point {
        let dimensions = GridDimensions::of(self);
        let wide = self.wide_at(point);

        match side {
            SelectionSide::Right if wide == Some(CellWide::SpacerHead) => {
                point.column = 1;
                point.line += 1;
            }
            SelectionSide::Right if wide == Some(CellWide::Wide) => {
                point.column = point.column.saturating_add(1).min(dimensions.last_column());
            }
            SelectionSide::Left
                if matches!(wide, Some(CellWide::Wide) | Some(CellWide::SpacerTail)) =>
            {
                if wide == Some(CellWide::SpacerTail) {
                    point.column = point.column.saturating_sub(1);
                }

                let previous = point_sub(dimensions, PointBoundary::Grid, point, 1);
                if self.wide_at(previous) == Some(CellWide::SpacerHead) {
                    point = previous;
                }
            }
            _ => {}
        }

        point
    }

    fn wide_at(&self, point: Point) -> Option<CellWide> {
        self.terminal
            .grid_ref(self.ghostty_screen_point(point))
            .and_then(|grid_ref| grid_ref.cell())
            .and_then(|cell| cell.wide())
            .log_err()
    }

    fn try_make_content(&mut self, last_content: &Content) -> Result<Content, GhosttyError> {
        let collected = self.collect_viewport_cells()?;

        let display_offset = self.display_offset();
        let scrollback_lines = self.scrollback_lines();
        let cells: Vec<IndexedCell> = collected
            .cells
            .into_iter()
            .map(|cell| indexed_cell(cell, display_offset as i32))
            .collect();

        let mode = self.modes();

        // Derived events (SPEC.md §4.1): no ghostty callback exists for
        // these, so they are computed frame-over-frame from snapshot diffs.
        if let Some(previous) = self.prev_cursor_blinking
            && previous != collected.cursor_blinking
        {
            self.send_event(TerminalBackendEvent::CursorBlinkingChange);
        }
        self.prev_cursor_blinking = Some(collected.cursor_blinking);

        let mouse_mode = mode.intersects(Modes::MOUSE_MODE);
        if let Some(previous) = self.prev_mouse_mode
            && previous != mouse_mode
        {
            self.send_event(TerminalBackendEvent::MouseCursorDirty);
        }
        self.prev_mouse_mode = Some(mouse_mode);

        let cursor_point = Point::new(
            self.terminal.cursor_y()? as i32,
            self.terminal.cursor_x()? as usize,
        );
        let cursor_char = self
            .terminal
            .grid_ref(GhosttyPoint::Active(PointCoordinate {
                x: cursor_point.column as u16,
                y: cursor_point.line as u32,
            }))
            .and_then(|grid_ref| grid_ref.cell())
            .and_then(|cell| cell.codepoint())
            .ok()
            .and_then(char::from_u32)
            .filter(|&character| character != '\0')
            .unwrap_or(' ');

        let selection = self.sync_selection_to_terminal();
        let selection_text = if selection.is_some() {
            self.active_selection_text()
        } else {
            None
        };

        let bottom_line = collected.screen_lines as i32 - 1 - display_offset as i32;
        let bottom_row_occupied = cursor_point.line >= bottom_line
            || cells
                .iter()
                .rev()
                .take_while(|cell| cell.point.line >= bottom_line)
                .any(|cell| cell.cell.character() != ' ');

        Ok(Content {
            cells,
            mode,
            display_offset,
            selection_text,
            selection,
            cursor: Cursor {
                shape: collected.cursor_shape,
                point: cursor_point,
            },
            cursor_char,
            terminal_bounds: last_content.terminal_bounds,
            last_hovered_word: last_content.last_hovered_word.clone(),
            scrolled_to_top: display_offset == scrollback_lines,
            scrolled_to_bottom: display_offset == 0,
            bottom_row_occupied,
        })
    }

    fn collect_viewport_cells(&mut self) -> Result<CollectedCells, GhosttyError> {
        let snapshot = self.render_state.update(&self.terminal)?;
        if snapshot.dirty()? == Dirty::Full {
            self.display_offset.set(None);
        }
        // Dirty state is consumed here so it keeps meaning "changed since the
        // last snapshot"; both layers must be unset independently.
        snapshot.set_dirty(Dirty::Clean)?;

        let columns = snapshot.cols()? as usize;
        let screen_lines = snapshot.rows()? as usize;
        let cursor_blinking = snapshot.cursor_blinking()?;
        let cursor_shape = if snapshot.cursor_visible()? {
            cursor_shape_from_ghostty(snapshot.cursor_visual_style()?)
        } else {
            CursorShape::Hidden
        };

        let mut cells = Vec::with_capacity(columns * screen_lines);
        let mut row_iteration = self.row_iterator.update(&snapshot)?;
        let mut viewport_line = 0;
        while let Some(row) = row_iteration.next() {
            let mut cell_iteration = self.cell_iterator.update(row)?;
            for column in 0..columns {
                cell_iteration.select(column as u16)?;

                let raw_cell = cell_iteration.raw_cell()?;
                let style = cell_iteration.style()?;

                let graphemes_len = cell_iteration.graphemes_len()?;
                let mut graphemes = vec!['\0'; graphemes_len];
                if graphemes_len > 0 {
                    cell_iteration.graphemes_buf(&mut graphemes)?;
                }
                let character = graphemes.first().copied().unwrap_or(' ');
                let zerowidth = graphemes.get(1..).unwrap_or_default().to_vec();

                let hyperlink = if raw_cell.has_hyperlink()? {
                    hyperlink_at(&self.terminal, column as u16, viewport_line as u32)
                } else {
                    None
                };

                let mut flags = cell_flags_from_style(&style);
                if raw_cell.wide()? == CellWide::SpacerTail {
                    flags.insert(CellFlags::WIDE_CHAR_SPACER);
                }

                let extra = (!zerowidth.is_empty() || hyperlink.is_some()).then(|| {
                    Arc::new(CellExtra {
                        zerowidth,
                        hyperlink,
                    })
                });

                cells.push(ViewportCell {
                    column,
                    viewport_line,
                    cell: Cell {
                        c: if character == '\0' { ' ' } else { character },
                        fg: color_from_style(style.fg_color, NamedColor::Foreground),
                        bg: color_from_style(style.bg_color, NamedColor::Background),
                        flags,
                        extra,
                    },
                });
            }
            row.set_dirty(false)?;
            viewport_line += 1;
        }

        Ok(CollectedCells {
            cells,
            screen_lines,
            cursor_blinking,
            cursor_shape,
        })
    }

    fn send_event(&self, event: TerminalBackendEvent) {
        self.events_tx.unbounded_send(PtyEvent::Event(event)).ok();
    }

    fn drain_events(&self) {
        loop {
            let Some(event) = self.events.borrow_mut().pop_front() else {
                break;
            };
            self.events_tx.unbounded_send(PtyEvent::Event(event)).ok();
        }
    }

    /// Convert a grid-convention point (line 0 = top of the active screen,
    /// scrollback negative — SPEC.md §4.2 S3) into ghostty Screen space.
    fn ghostty_screen_point(&self, point: Point) -> GhosttyPoint {
        let dimensions = GridDimensions::of(self);
        let point = point_clamp(dimensions, PointBoundary::Grid, point);
        GhosttyPoint::Screen(PointCoordinate {
            x: point.column as u16,
            y: (point.line - dimensions.topmost_line) as u32,
        })
    }

    fn grid_point_from_screen(&self, coordinate: PointCoordinate) -> Point {
        Point::new(
            coordinate.y as i32 - self.scrollback_lines() as i32,
            coordinate.x as usize,
        )
    }

    fn track_grid_point(&self, point: Point) -> Option<TrackedGridRef> {
        self.terminal
            .track_grid_ref(self.ghostty_screen_point(point))
            .log_err()
    }

    fn tracked_grid_point(&self, tracked: &TrackedGridRef) -> Option<Point> {
        let coordinate = tracked.point(PointSpace::Screen).log_err()??;
        Some(self.grid_point_from_screen(coordinate))
    }

    /// Compute the effective selected range from the seam selection state,
    /// porting alacritty's `Selection::to_range` semantics, and install it as
    /// ghostty's active selection so formatting and rendering agree.
    /// Returns the range, or `None` when there is no (non-empty) selection.
    fn sync_selection_to_terminal(&self) -> Option<SelectionRange> {
        let range = self.selection_range();
        match range {
            Some(range) => {
                let installed = self
                    .terminal
                    .grid_ref(self.ghostty_screen_point(range.start))
                    .and_then(|start| {
                        let end = self.terminal.grid_ref(self.ghostty_screen_point(range.end))?;
                        self.terminal
                            .set_selection(Some(&GhosttySelection::new(start, end, false)))?;
                        Ok(())
                    });
                installed.log_err();
            }
            None => {
                self.terminal.set_selection(None).log_err();
            }
        }
        range
    }

    fn selection_range(&self) -> Option<SelectionRange> {
        let selection = self.selection.as_ref()?;
        let start_point = self.tracked_grid_point(&selection.start)?;
        let end_point = self.tracked_grid_point(&selection.end)?;

        let mut start = (start_point, selection.start_side);
        let mut end = (end_point, selection.end_side);
        if start.0 > end.0 {
            std::mem::swap(&mut start, &mut end);
        }

        let dimensions = GridDimensions::of(self);
        start.0 = point_clamp(dimensions, PointBoundary::Grid, start.0);
        end.0 = point_clamp(dimensions, PointBoundary::Grid, end.0);

        match selection.ty {
            SelectionType::Simple => range_simple(dimensions, start, end),
            SelectionType::Semantic => self.range_semantic(start.0, end.0),
            SelectionType::Lines => self.range_lines(start.0, end.0),
        }
    }

    /// Semantic word selection via ghostty's word gesture, passing
    /// alacritty's escape-char set per call (parity matrix row 76).
    fn range_semantic(&self, start: Point, end: Point) -> Option<SelectionRange> {
        let start = self
            .word_selection_at(start)
            .map(|range| range.start)
            .unwrap_or(start);
        let end = self
            .word_selection_at(end)
            .map(|range| range.end)
            .unwrap_or(end);
        Some(SelectionRange {
            start,
            end,
            is_block: false,
        })
    }

    fn word_selection_at(&self, point: Point) -> Option<SelectionRange> {
        let grid_ref = self
            .terminal
            .grid_ref(self.ghostty_screen_point(point))
            .log_err()?;
        let selection = self
            .terminal
            .select_word(
                SelectWordOptions::new(grid_ref).with_boundary_codepoints(SEMANTIC_ESCAPE_CHARS),
            )
            .log_err()??;
        self.selection_endpoints(&selection)
    }

    fn range_lines(&self, start: Point, end: Point) -> Option<SelectionRange> {
        let start = self
            .line_selection_at(start)
            .map(|range| range.start)
            .unwrap_or_else(|| Point::new(start.line, 0));
        let end = self
            .line_selection_at(end)
            .map(|range| range.end)
            .unwrap_or_else(|| {
                Point::new(end.line, GridDimensions::of(self).last_column())
            });
        Some(SelectionRange {
            start,
            end,
            is_block: false,
        })
    }

    fn line_selection_at(&self, point: Point) -> Option<SelectionRange> {
        let grid_ref = self
            .terminal
            .grid_ref(self.ghostty_screen_point(point))
            .log_err()?;
        // An empty whitespace set keeps the full logical line, matching
        // alacritty's untrimmed line selection.
        let selection = self
            .terminal
            .select_line(SelectLineOptions::new(grid_ref).with_whitespace(&[]))
            .log_err()??;
        self.selection_endpoints(&selection)
    }

    fn selection_endpoints(&self, selection: &GhosttySelection<'_>) -> Option<SelectionRange> {
        let start = self
            .terminal
            .point_from_grid_ref(&selection.start(), PointSpace::Screen)
            .log_err()??;
        let end = self
            .terminal
            .point_from_grid_ref(&selection.end(), PointSpace::Screen)
            .log_err()??;
        Some(SelectionRange {
            start: self.grid_point_from_screen(start),
            end: self.grid_point_from_screen(end),
            is_block: false,
        })
    }

    fn format_active_selection(&self) -> Option<String> {
        self.format_selection_text(|options| options)
    }

    fn format_range_text(&self, range: Range) -> Option<String> {
        let start = self
            .terminal
            .grid_ref(self.ghostty_screen_point(range.start()))
            .log_err()?;
        let end = self
            .terminal
            .grid_ref(self.ghostty_screen_point(range.end()))
            .log_err()?;
        let selection = GhosttySelection::new(start, end, false);
        self.format_selection_text(|options| options.with_selection(&selection))
    }

    /// Format a selection as plain text with unwrap and trim, matching
    /// alacritty's `selection_to_string` semantics (soft-wrapped lines join
    /// without a newline; trailing whitespace trims per line).
    fn format_selection_text<'t, 's>(
        &'t self,
        configure: impl Fn(FormatOptions<'t, 's>) -> FormatOptions<'t, 's>,
    ) -> Option<String>
    where
        't: 's,
    {
        let mut buffer = vec![0; 1024];
        loop {
            let options = configure(
                FormatOptions::new()
                    .with_emit_format(Format::Plain)
                    .with_unwrap(true)
                    .with_trim(true),
            );
            match self.terminal.format_selection_buf(options, &mut buffer) {
                Ok(Some(written)) => {
                    return String::from_utf8(buffer[..written].to_vec()).log_err();
                }
                Ok(None) => return None,
                Err(GhosttyError::OutOfSpace { required }) => buffer.resize(required, 0),
                Err(error) => {
                    log::error!("ghostty backend failed to format a selection: {error}");
                    return None;
                }
            }
        }
    }

    fn find_logical_line_start(&self, current_line: i64) -> i64 {
        let mut line_start = current_line;
        while line_start > 0 {
            let wrapped = self
                .terminal
                .grid_ref(GhosttyPoint::Screen(PointCoordinate {
                    x: 0,
                    y: (line_start - 1) as u32,
                }))
                .and_then(|grid_ref| grid_ref.row())
                .and_then(|row| row.is_wrapped())
                .log_err()
                .unwrap_or(false);
            if !wrapped {
                break;
            }
            line_start -= 1;
        }
        line_start
    }

    fn push_row_text(&self, line: &mut String, row: u32, columns: usize) {
        for column in 0..columns {
            let character = self
                .terminal
                .grid_ref(GhosttyPoint::Screen(PointCoordinate {
                    x: column as u16,
                    y: row,
                }))
                .and_then(|grid_ref| grid_ref.cell())
                .and_then(|cell| cell.codepoint())
                .ok()
                .and_then(char::from_u32)
                .filter(|&character| character != '\0')
                .unwrap_or(' ');
            line.push(character);
        }
    }
}

struct ViewportCell {
    column: usize,
    viewport_line: usize,
    cell: Cell,
}

struct CollectedCells {
    cells: Vec<ViewportCell>,
    screen_lines: usize,
    cursor_blinking: bool,
    cursor_shape: CursorShape,
}

fn indexed_cell(cell: ViewportCell, display_offset: i32) -> IndexedCell {
    IndexedCell {
        // S3 coordinate conversion: the render state iterates the viewport;
        // Zed's Content speaks alacritty grid lines.
        point: Point::new(cell.viewport_line as i32 - display_offset, cell.column),
        cell: cell.cell,
    }
}

fn hyperlink_at(terminal: &GhosttyTerminal<'_, '_>, x: u16, y: u32) -> Option<Hyperlink> {
    let grid_ref = terminal
        .grid_ref(GhosttyPoint::Viewport(PointCoordinate { x, y }))
        .log_err()?;
    let mut buffer = vec![0; 256];
    loop {
        match grid_ref.hyperlink_uri(&mut buffer) {
            Ok(0) => return None,
            Ok(written) => {
                let uri = String::from_utf8(buffer[..written].to_vec()).log_err()?;
                // OSC 8 ids are not exposed by ghostty (G7); hover-extent
                // equality compares URIs and `id` stays `None`.
                return Some(Hyperlink::new(None::<&str>, uri));
            }
            Err(GhosttyError::OutOfSpace { required }) => buffer.resize(required, 0),
            Err(error) => {
                log::error!("ghostty backend failed to read a hyperlink uri: {error}");
                return None;
            }
        }
    }
}

fn register_callbacks(
    terminal: &mut GhosttyTerminal<'static, 'static>,
    events: &Rc<RefCell<VecDeque<TerminalBackendEvent>>>,
    bounds: &Rc<StdCell<TerminalBounds>>,
    color_scheme: &Rc<StdCell<ColorScheme>>,
    display_only: bool,
) -> Result<(), GhosttyError> {
    terminal
        .on_pty_write({
            let events = events.clone();
            move |_, data| {
                events
                    .borrow_mut()
                    .push_back(TerminalBackendEvent::PtyWrite(
                        String::from_utf8_lossy(data).into_owned(),
                    ));
            }
        })?
        .on_bell({
            let events = events.clone();
            move |_| events.borrow_mut().push_back(TerminalBackendEvent::Bell)
        })?
        .on_title_changed({
            let events = events.clone();
            move |terminal| {
                let event = match terminal.title() {
                    Ok("") => TerminalBackendEvent::ResetTitle,
                    Ok(title) => TerminalBackendEvent::Title(title.to_string()),
                    Err(error) => {
                        log::error!("ghostty backend failed to read the title: {error}");
                        return;
                    }
                };
                events.borrow_mut().push_back(event);
            }
        })?
        // Parity: the alacritty-era core never answers XTVERSION, so the
        // mandatory registration responds with a silent ignore.
        .on_xtversion(|_| None)?
        .on_size({
            let bounds = bounds.clone();
            move |_| {
                let bounds = bounds.get();
                Some(SizeReportSize {
                    rows: bounds.num_lines() as u16,
                    columns: bounds.num_columns() as u16,
                    cell_width: f32::from(bounds.cell_width()) as u32,
                    cell_height: f32::from(bounds.line_height()) as u32,
                })
            }
        })?
        .on_color_scheme({
            let color_scheme = color_scheme.clone();
            move |_| Some(color_scheme.get())
        })?
        .on_device_attributes(|_| {
            Some(DeviceAttributes {
                primary: PRIMARY_DEVICE_ATTRIBUTES,
                secondary: SecondaryDeviceAttributes {
                    device_type: DeviceType::VT100,
                    firmware_version: 0,
                    rom_cartridge: 0,
                },
                tertiary: TertiaryDeviceAttributes { unit_id: 0 },
            })
        })?;

    if !display_only {
        terminal.on_clipboard_write({
            let events = events.clone();
            move |_, write| {
                if let Some(content) = write.contents().next() {
                    events
                        .borrow_mut()
                        .push_back(TerminalBackendEvent::ClipboardStore(
                            content.data.to_string(),
                        ));
                }
                Ok(())
            }
        })?;
    }

    Ok(())
}

/// The grid dimensions the point helpers operate over, in the alacritty grid
/// convention: line 0 is the top of the active screen and scrollback lines
/// are negative.
#[derive(Clone, Copy, Debug)]
pub(super) struct GridDimensions {
    pub topmost_line: i32,
    pub screen_lines: usize,
    pub columns: usize,
}

impl GridDimensions {
    fn of(backend: &TerminalBackend) -> Self {
        Self {
            topmost_line: -(backend.scrollback_lines() as i32),
            screen_lines: backend.screen_lines(),
            columns: backend.columns(),
        }
    }

    fn bottommost_line(&self) -> i32 {
        self.screen_lines as i32 - 1
    }

    fn last_column(&self) -> usize {
        self.columns.saturating_sub(1)
    }

    fn full_range(&self) -> Range {
        Range::new(
            Point::new(self.topmost_line, 0),
            Point::new(self.bottommost_line(), self.last_column()),
        )
    }
}

/// Grid boundaries for the point helpers, mirroring alacritty's `Boundary`:
/// `Grid` spans scrollback top to screen bottom; `Cursor` is the cursor's
/// range of motion (the active screen only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PointBoundary {
    Grid,
    Cursor,
}

/// Add a number of columns to a point, wrapping across lines, then clamp
/// (alacritty `Point::add` semantics).
pub(super) fn point_add(
    dimensions: GridDimensions,
    boundary: PointBoundary,
    mut point: Point,
    rhs: usize,
) -> Point {
    let columns = dimensions.columns.max(1);
    point.line += ((rhs + point.column) / columns) as i32;
    point.column = (point.column + rhs) % columns;
    point_clamp(dimensions, boundary, point)
}

/// Subtract a number of columns from a point, wrapping across lines, then
/// clamp (alacritty `Point::sub` semantics).
pub(super) fn point_sub(
    dimensions: GridDimensions,
    boundary: PointBoundary,
    mut point: Point,
    rhs: usize,
) -> Point {
    let columns = dimensions.columns.max(1);
    let line_changes = (rhs + columns - 1).saturating_sub(point.column) / columns;
    point.line -= line_changes as i32;
    point.column = (columns + point.column - rhs % columns) % columns;
    point_clamp(dimensions, boundary, point)
}

/// Clamp a point to a grid boundary (alacritty `Point::grid_clamp`
/// semantics for the `Grid` and `Cursor` boundaries).
pub(super) fn point_clamp(
    dimensions: GridDimensions,
    boundary: PointBoundary,
    mut point: Point,
) -> Point {
    point.column = point.column.min(dimensions.last_column());

    let topmost_line = match boundary {
        PointBoundary::Grid => dimensions.topmost_line,
        PointBoundary::Cursor => 0,
    };
    let bottommost_line = dimensions.bottommost_line();

    if point.line < topmost_line {
        Point::new(topmost_line, 0)
    } else if point.line > bottommost_line {
        Point::new(bottommost_line, dimensions.last_column())
    } else {
        point
    }
}

/// Simple-selection side arithmetic ported from alacritty's
/// `Selection::{is_empty, range_simple}`: a cell is included only when
/// grabbed from its covering side.
fn range_simple(
    dimensions: GridDimensions,
    start: (Point, SelectionSide),
    end: (Point, SelectionSide),
) -> Option<SelectionRange> {
    let (start_point, start_side) = start;
    let (end_point, end_side) = end;

    // Empty when the anchors are identical or two adjacent cells have the
    // sides right -> left.
    if start_point == end_point && start_side == end_side {
        return None;
    }
    if start_side == SelectionSide::Right
        && end_side == SelectionSide::Left
        && start_point.line == end_point.line
        && start_point.column + 1 == end_point.column
    {
        return None;
    }

    let columns = dimensions.columns.max(1);
    let mut start = start_point;
    let mut end = end_point;

    // Remove the last cell if the selection ends to the left of a cell.
    if end_side == SelectionSide::Left && start_point != end_point {
        if end.column == 0 {
            end.column = columns - 1;
            end.line -= 1;
        } else {
            end.column -= 1;
        }
    }

    // Remove the first cell if the selection starts at the right of a cell.
    if start_side == SelectionSide::Right && start_point != end_point {
        start.column += 1;
        if start.column == columns {
            start.column = 0;
            start.line += 1;
        }
    }

    Some(SelectionRange {
        start,
        end,
        is_block: false,
    })
}

fn ghostty_cursor_style(cursor_shape: SettingsCursorShape) -> GhosttyCursorStyle {
    match cursor_shape {
        SettingsCursorShape::Block => GhosttyCursorStyle::Block,
        SettingsCursorShape::Underline => GhosttyCursorStyle::Underline,
        SettingsCursorShape::Bar => GhosttyCursorStyle::Bar,
        SettingsCursorShape::Hollow => GhosttyCursorStyle::BlockHollow,
    }
}

fn cursor_shape_from_ghostty(style: CursorVisualStyle) -> CursorShape {
    match style {
        CursorVisualStyle::Block => CursorShape::Block,
        CursorVisualStyle::Underline => CursorShape::Underline,
        CursorVisualStyle::Bar => CursorShape::Bar,
        CursorVisualStyle::BlockHollow => CursorShape::HollowBlock,
        // The enum is non-exhaustive upstream; fall back to the default
        // block shape for any future variant.
        _ => CursorShape::Block,
    }
}

fn ghostty_rgb(rgb: Rgb) -> RgbColor {
    RgbColor {
        r: rgb.r,
        g: rgb.g,
        b: rgb.b,
    }
}

fn zed_rgb(rgb: RgbColor) -> Rgb {
    Rgb {
        r: rgb.r,
        g: rgb.g,
        b: rgb.b,
    }
}

/// The semantic `StyleColor` mapping (SPEC.md §4.2, #31), read from the
/// style layer, never the flattened per-cell RGB: `None` maps to the given
/// default named color, palette 0–15 to named ANSI colors, palette 16–255 to
/// `Indexed`, and direct RGB to `Spec`. Dim stays a cell flag.
fn color_from_style(color: StyleColor, default: NamedColor) -> Color {
    match color {
        StyleColor::None => Color::Named(default),
        StyleColor::Palette(PaletteIndex(index)) => match index {
            0..=15 => Color::Named(named_ansi_color(index)),
            _ => Color::Indexed(index),
        },
        StyleColor::Rgb(rgb) => Color::Spec(zed_rgb(rgb)),
    }
}

fn named_ansi_color(index: u8) -> NamedColor {
    match index {
        0 => NamedColor::Black,
        1 => NamedColor::Red,
        2 => NamedColor::Green,
        3 => NamedColor::Yellow,
        4 => NamedColor::Blue,
        5 => NamedColor::Magenta,
        6 => NamedColor::Cyan,
        7 => NamedColor::White,
        8 => NamedColor::BrightBlack,
        9 => NamedColor::BrightRed,
        10 => NamedColor::BrightGreen,
        11 => NamedColor::BrightYellow,
        12 => NamedColor::BrightBlue,
        13 => NamedColor::BrightMagenta,
        14 => NamedColor::BrightCyan,
        _ => NamedColor::BrightWhite,
    }
}

fn cell_flags_from_style(style: &ghostty_vt::style::Style) -> CellFlags {
    let mut flags = CellFlags::empty();
    if style.inverse {
        flags.insert(CellFlags::INVERSE);
    }
    if style.bold {
        flags.insert(CellFlags::BOLD);
    }
    if style.italic {
        flags.insert(CellFlags::ITALIC);
    }
    if style.faint {
        flags.insert(CellFlags::DIM);
    }
    if style.strikethrough {
        flags.insert(CellFlags::STRIKEOUT);
    }
    match style.underline {
        Underline::None => {}
        Underline::Single => flags.insert(CellFlags::UNDERLINE),
        Underline::Double => flags.insert(CellFlags::DOUBLE_UNDERLINE),
        Underline::Curly => flags.insert(CellFlags::UNDERCURL),
        Underline::Dotted => flags.insert(CellFlags::DOTTED_UNDERLINE),
        Underline::Dashed => flags.insert(CellFlags::DASHED_UNDERLINE),
        // The enum is non-exhaustive upstream; render any future underline
        // variant as a plain underline.
        _ => flags.insert(CellFlags::UNDERLINE),
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_settings::CursorShape as SettingsCursorShape;
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

    fn test_backend(
        columns: usize,
        screen_lines: usize,
        scrollback: usize,
    ) -> (TerminalBackend, UnboundedReceiver<PtyEvent>) {
        let (events_tx, events_rx) = futures::channel::mpsc::unbounded();
        let backend = TerminalBackend::new(
            scrollback,
            SettingsCursorShape::Block,
            test_bounds(columns, screen_lines),
            events_tx,
            AlternateScroll::On,
        );
        (backend, events_rx)
    }

    fn content(backend: &mut TerminalBackend) -> Content {
        backend.make_content(&Content::default())
    }

    fn screen_rows(content: &Content, columns: usize, screen_lines: usize) -> Vec<String> {
        let mut rows = vec![vec![' '; columns]; screen_lines];
        for cell in &content.cells {
            if (0..screen_lines as i32).contains(&cell.point.line) && cell.point.column < columns {
                rows[cell.point.line as usize][cell.point.column] = cell.cell.character();
            }
        }
        rows.into_iter()
            .map(|row| row.into_iter().collect::<String>().trim_end().to_string())
            .collect()
    }

    fn cell_at(content: &Content, line: i32, column: usize) -> &Cell {
        &content
            .cells
            .iter()
            .find(|cell| cell.point.line == line && cell.point.column == column)
            .unwrap_or_else(|| panic!("no cell at line {line} column {column}"))
            .cell
    }

    fn drained_events(events_rx: &mut UnboundedReceiver<PtyEvent>) -> Vec<TerminalBackendEvent> {
        let mut events = Vec::new();
        while let Ok(PtyEvent::Event(event)) = events_rx.try_recv() {
            events.push(event);
        }
        events
    }

    fn pty_writes(events: Vec<TerminalBackendEvent>) -> String {
        events
            .into_iter()
            .filter_map(|event| match event {
                TerminalBackendEvent::PtyWrite(output) => Some(output),
                _ => None,
            })
            .collect()
    }

    /// Twin of the alacritty-backed G5 clear seam test
    /// (crates/terminal/src/alacritty.rs): byte-identical input and
    /// expectations, required to pass identically on both backends
    /// (SPEC.md §4.5, §6 P5).
    #[test]
    fn clear_preserves_prompt_line_and_erases_scrollback() {
        let (mut backend, _events_rx) = test_backend(20, 5, 100);
        backend.write(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\nprompt> ");

        // 8 lines on a 5-row screen: 3 lines are in scrollback.
        assert_eq!(backend.total_lines(), 8);
        assert_eq!(backend.screen_lines(), 5);

        backend.clear();

        assert_eq!(backend.total_lines(), 5, "scrollback should be erased");
        assert_eq!(backend.display_offset(), 0);

        let content = content(&mut backend);
        assert_eq!(content.cursor.point, Point::new(0, 8));
        assert_eq!(
            screen_rows(&content, 20, 5),
            vec![
                "prompt>".to_string(),
                String::new(),
                String::new(),
                String::new(),
                String::new()
            ],
        );
        assert!(content.scrolled_to_bottom);
    }

    /// Twin of the alacritty-backed companion clear test: content below the
    /// cursor is erased too (the cursor line alone survives).
    #[test]
    fn clear_erases_content_below_cursor() {
        let (mut backend, _events_rx) = test_backend(20, 5, 100);
        backend.write(b"one> \r\nbelow\x1b[1;6H");

        backend.clear();

        assert_eq!(backend.total_lines(), 5);
        let content = content(&mut backend);
        assert_eq!(content.cursor.point, Point::new(0, 5));
        assert_eq!(
            screen_rows(&content, 20, 5),
            vec![
                "one>".to_string(),
                String::new(),
                String::new(),
                String::new(),
                String::new()
            ],
        );
    }

    #[test]
    fn snapshot_maps_semantic_style_colors() {
        let (mut backend, _events_rx) = test_backend(40, 3, 0);
        backend.write(b"\x1b[31mr\x1b[0m\x1b[38;5;100mi\x1b[0m\x1b[38;2;1;2;3ms\x1b[0mp");

        let content = content(&mut backend);
        assert_eq!(cell_at(&content, 0, 0).fg, Color::Named(NamedColor::Red));
        assert_eq!(cell_at(&content, 0, 1).fg, Color::Indexed(100));
        assert_eq!(
            cell_at(&content, 0, 2).fg,
            Color::Spec(Rgb { r: 1, g: 2, b: 3 })
        );
        assert_eq!(
            cell_at(&content, 0, 3).fg,
            Color::Named(NamedColor::Foreground)
        );
        assert_eq!(
            cell_at(&content, 0, 3).bg,
            Color::Named(NamedColor::Background)
        );
    }

    #[test]
    fn snapshot_maps_cell_flags() {
        let (mut backend, _events_rx) = test_backend(40, 3, 0);
        backend.write(b"\x1b[1mb\x1b[0m\x1b[3mi\x1b[0m\x1b[2md\x1b[0m\x1b[9ms\x1b[0m\x1b[7mv\x1b[0m\x1b[4mu\x1b[0m\x1b[4:3mc\x1b[0m");

        let content = content(&mut backend);
        assert!(cell_at(&content, 0, 0).is_bold());
        assert!(cell_at(&content, 0, 1).is_italic());
        assert!(cell_at(&content, 0, 2).is_dim());
        assert!(cell_at(&content, 0, 3).has_strikeout());
        assert!(cell_at(&content, 0, 4).is_inverse());
        assert!(cell_at(&content, 0, 5).flags.contains(CellFlags::UNDERLINE));
        assert!(cell_at(&content, 0, 6).has_undercurl());
    }

    #[test]
    fn snapshot_rebuilds_modes_from_typed_getters() {
        let (mut backend, _events_rx) = test_backend(40, 3, 0);

        let baseline = backend.modes();
        assert!(baseline.contains(Modes::SHOW_CURSOR));
        assert!(baseline.contains(Modes::LINE_WRAP));
        assert!(!baseline.contains(Modes::BRACKETED_PASTE));
        assert!(!baseline.contains(Modes::ALT_SCREEN));
        assert!(!baseline.intersects(Modes::MOUSE_MODE));

        backend.write(b"\x1b[?2004h\x1b[?1000h\x1b[?1006h\x1b[?1h");
        let modes = backend.modes();
        assert!(modes.contains(Modes::BRACKETED_PASTE));
        assert!(modes.contains(Modes::MOUSE_REPORT_CLICK));
        assert!(modes.contains(Modes::SGR_MOUSE));
        assert!(modes.contains(Modes::APP_CURSOR));

        backend.write(b"\x1b[?1049h");
        assert!(backend.modes().contains(Modes::ALT_SCREEN));
        backend.write(b"\x1b[?1049l");
        assert!(!backend.modes().contains(Modes::ALT_SCREEN));
    }

    #[test]
    fn creation_contract_unsets_alt_scroll_when_off() {
        let (events_tx, _events_rx) = futures::channel::mpsc::unbounded();
        let backend = TerminalBackend::new(
            100,
            SettingsCursorShape::Block,
            test_bounds(20, 5),
            events_tx,
            AlternateScroll::Off,
        );
        assert!(!backend.modes().contains(Modes::ALTERNATE_SCROLL));

        let (mut on_backend, _events_rx) = test_backend(20, 5, 100);
        assert!(on_backend.modes().contains(Modes::ALTERNATE_SCROLL));
        on_backend.write(b"x");
    }

    /// Ghostty's scrollback limit is byte/page-granular, not an exact line
    /// count (divergence ledger P5-001): zero disables history exactly, and
    /// non-zero values bound history at page granularity.
    #[test]
    fn creation_contract_bounds_scrollback() {
        let (mut zero, _events_rx) = test_backend(20, 3, 0);
        zero.write(b"1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n8");
        assert_eq!(zero.total_lines(), 3, "zero scrollback keeps no history");

        let (mut bounded, _events_rx) = test_backend(20, 3, 2);
        let mut flood = Vec::new();
        for index in 0..20_000 {
            flood.extend_from_slice(format!("line {index}\r\n").as_bytes());
        }
        bounded.write(&flood);
        assert!(
            bounded.total_lines() < 20_000,
            "a small line cap must bound history (got {})",
            bounded.total_lines()
        );
    }

    #[test]
    fn scrollback_coordinates_and_display_offset() {
        let (mut backend, _events_rx) = test_backend(20, 5, 100);
        backend.write(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\nprompt> ");

        let at_bottom = content(&mut backend);
        assert_eq!(at_bottom.display_offset, 0);
        assert!(at_bottom.scrolled_to_bottom);
        assert!(!at_bottom.scrolled_to_top);
        assert_eq!(cell_at(&at_bottom, 0, 0).character(), 'f');
        assert_eq!(at_bottom.cursor.point, Point::new(4, 8));

        backend.scroll_display(Scroll::Delta(2));
        assert_eq!(backend.display_offset(), 2);
        let scrolled = content(&mut backend);
        assert_eq!(scrolled.display_offset, 2);
        assert!(!scrolled.scrolled_to_bottom);
        // The viewport now starts two lines into scrollback: grid line -2.
        assert_eq!(cell_at(&scrolled, -2, 0).character(), 't');
        assert_eq!(cell_at(&scrolled, 2, 0).character(), 's');

        backend.scroll_display(Scroll::Top);
        let top = content(&mut backend);
        assert_eq!(top.display_offset, 3);
        assert!(top.scrolled_to_top);
        assert_eq!(cell_at(&top, -3, 0).character(), 'o');

        backend.scroll_display(Scroll::Bottom);
        assert_eq!(backend.display_offset(), 0);

        backend.scroll_to_point(Point::new(-3, 0));
        assert_eq!(backend.display_offset(), 3);
        backend.scroll_to_point(Point::new(4, 0));
        assert_eq!(backend.display_offset(), 0);
    }

    #[test]
    fn wide_chars_and_expand_wide() {
        let (mut backend, _events_rx) = test_backend(4, 3, 100);
        backend.write("a世b".as_bytes());

        let content = content(&mut backend);
        assert_eq!(cell_at(&content, 0, 1).character(), '世');
        assert!(cell_at(&content, 0, 2).is_wide_char_spacer());
        assert_eq!(cell_at(&content, 0, 3).character(), 'b');

        assert_eq!(
            backend.expand_wide(Point::new(0, 1), SelectionSide::Right),
            Point::new(0, 2)
        );
        assert_eq!(
            backend.expand_wide(Point::new(0, 2), SelectionSide::Left),
            Point::new(0, 1)
        );
        assert_eq!(
            backend.expand_wide(Point::new(0, 0), SelectionSide::Right),
            Point::new(0, 0)
        );
    }

    #[test]
    fn expand_wide_follows_spacer_head_across_lines() {
        let (mut backend, _events_rx) = test_backend(4, 3, 100);
        // The wide char doesn't fit in the last column, leaving a spacer
        // head there and wrapping the wide char to the next line.
        backend.write("abc世".as_bytes());

        let content = content(&mut backend);
        assert_eq!(cell_at(&content, 1, 0).character(), '世');

        assert_eq!(
            backend.expand_wide(Point::new(0, 3), SelectionSide::Right),
            Point::new(1, 1)
        );
        assert_eq!(
            backend.expand_wide(Point::new(1, 1), SelectionSide::Left),
            Point::new(0, 3)
        );
    }

    #[test]
    fn zerowidth_graphemes_split_into_base_and_tail() {
        let (mut backend, _events_rx) = test_backend(20, 3, 0);
        backend.write("e\u{0301}x".as_bytes());

        let content = content(&mut backend);
        let cell = cell_at(&content, 0, 0);
        assert_eq!(cell.character(), 'e');
        assert_eq!(cell.zerowidth(), Some(&['\u{0301}'][..]));
        assert_eq!(cell_at(&content, 0, 1).character(), 'x');
    }

    #[test]
    fn hyperlink_cells_carry_uri_without_id() {
        let (mut backend, _events_rx) = test_backend(20, 3, 0);
        backend.write(b"\x1b]8;;https://example.com\x1b\\hi\x1b]8;;\x1b\\ no");

        let content = content(&mut backend);
        let hyperlink = cell_at(&content, 0, 0).hyperlink().expect("hyperlink");
        assert_eq!(hyperlink.uri(), "https://example.com");
        assert_eq!(hyperlink.id(), None);
        assert!(cell_at(&content, 0, 3).hyperlink().is_none());
    }

    #[test]
    fn cursor_shape_and_char() {
        let (mut backend, _events_rx) = test_backend(20, 3, 0);
        backend.write(b"ab\x1b[1;1H");

        let visible = content(&mut backend);
        assert_eq!(visible.cursor.shape, CursorShape::Block);
        assert_eq!(visible.cursor.point, Point::new(0, 0));
        assert_eq!(visible.cursor_char, 'a');

        backend.write(b"\x1b[?25l");
        let hidden = content(&mut backend);
        assert_eq!(hidden.cursor.shape, CursorShape::Hidden);
    }

    #[test]
    fn simple_selection_matches_alacritty_side_semantics() {
        let inputs: &[u8] = b"alpha beta\r\ngamma delta";
        let cases = [
            // (start point, start side, end point, end side)
            (Point::new(0, 0), SelectionSide::Left, Point::new(0, 4), SelectionSide::Right),
            (Point::new(0, 2), SelectionSide::Right, Point::new(1, 4), SelectionSide::Left),
            (Point::new(0, 3), SelectionSide::Left, Point::new(0, 3), SelectionSide::Right),
            // Backwards drag: anchors swap.
            (Point::new(1, 4), SelectionSide::Right, Point::new(0, 2), SelectionSide::Left),
            // Empty: same cell, same side.
            (Point::new(0, 3), SelectionSide::Left, Point::new(0, 3), SelectionSide::Left),
            // Empty: adjacent cells grabbed right -> left.
            (Point::new(0, 3), SelectionSide::Right, Point::new(0, 4), SelectionSide::Left),
        ];

        for (index, (start, start_side, end, end_side)) in cases.into_iter().enumerate() {
            let (mut ghostty, _ghostty_rx) = test_backend(20, 5, 100);
            ghostty.write(inputs);
            let mut selection = Selection::new(SelectionType::Simple, start, start_side);
            selection.update(end, end_side);
            ghostty.set_selection(Some(&selection));
            let ghostty_content = content(&mut ghostty);

            let (events_tx, _alacritty_rx) = futures::channel::mpsc::unbounded();
            let mut alacritty = crate::alacritty::TerminalBackend::new(
                100,
                SettingsCursorShape::Block,
                test_bounds(20, 5),
                events_tx,
                AlternateScroll::On,
            );
            alacritty.write(inputs);
            alacritty.set_selection(Some(&selection));
            let alacritty_content = alacritty.make_content(&Content::default());

            assert_eq!(
                ghostty_content.selection, alacritty_content.selection,
                "case {index}: selection range should match alacritty"
            );
            assert_eq!(
                ghostty_content.selection_text, alacritty_content.selection_text,
                "case {index}: selection text should match alacritty"
            );
            assert_eq!(
                ghostty.selection_text(),
                alacritty.selection_text(),
                "case {index}: selection_text() should match alacritty"
            );
        }
    }

    /// Wide-char endpoints: the pinned alacritty `Selection::to_range` does
    /// no fullwidth expansion (`expand_wide` is a vi/search helper there),
    /// so the seam must not add any either — both backends must agree on
    /// selections that start or end on a wide char or its spacer.
    ///
    /// Selection *text* has one adjudicated exception (divergence ledger
    /// P5-002): when the selection cut leaves trailing spaces that are
    /// interior to the row, ghostty's formatter trims them and alacritty
    /// keeps them; `expected_text` pins the ghostty behavior and
    /// `matches_alacritty_text` marks which cases the oracle comparison
    /// still covers.
    #[test]
    fn simple_selection_over_wide_chars_matches_alacritty() {
        let inputs = "a世b 世世x".as_bytes();
        let cases = [
            (
                (Point::new(0, 0), SelectionSide::Left, Point::new(0, 1), SelectionSide::Right),
                "a世",
                true,
            ),
            (
                (Point::new(0, 1), SelectionSide::Left, Point::new(0, 2), SelectionSide::Right),
                "世",
                true,
            ),
            // Endpoints on the wide char's trailing spacer cell.
            (
                (Point::new(0, 2), SelectionSide::Left, Point::new(0, 6), SelectionSide::Right),
                "世b 世",
                true,
            ),
            // The selection ends on a row-interior space: ghostty trims it,
            // alacritty keeps it (ledger P5-002).
            (
                (Point::new(0, 2), SelectionSide::Right, Point::new(0, 5), SelectionSide::Left),
                "b",
                false,
            ),
        ];

        for (index, ((start, start_side, end, end_side), expected_text, matches_alacritty_text)) in
            cases.into_iter().enumerate()
        {
            let (mut ghostty, _ghostty_rx) = test_backend(20, 5, 100);
            ghostty.write(inputs);
            let mut selection = Selection::new(SelectionType::Simple, start, start_side);
            selection.update(end, end_side);
            ghostty.set_selection(Some(&selection));
            let ghostty_content = content(&mut ghostty);

            let (events_tx, _alacritty_rx) = futures::channel::mpsc::unbounded();
            let mut alacritty = crate::alacritty::TerminalBackend::new(
                100,
                SettingsCursorShape::Block,
                test_bounds(20, 5),
                events_tx,
                AlternateScroll::On,
            );
            alacritty.write(inputs);
            alacritty.set_selection(Some(&selection));
            let alacritty_content = alacritty.make_content(&Content::default());

            assert_eq!(
                ghostty_content.selection, alacritty_content.selection,
                "case {index}: wide-char selection range should match alacritty"
            );
            assert_eq!(
                ghostty_content.selection_text.as_deref(),
                Some(expected_text),
                "case {index}: ghostty selection text"
            );
            if matches_alacritty_text {
                assert_eq!(
                    ghostty_content.selection_text, alacritty_content.selection_text,
                    "case {index}: wide-char selection text should match alacritty"
                );
            } else {
                assert_eq!(
                    alacritty_content.selection_text.as_deref(),
                    Some("b "),
                    "case {index}: alacritty keeps the row-interior space (P5-002)"
                );
            }
        }
    }

    #[test]
    fn update_selection_extends_the_end() {
        let (mut backend, _events_rx) = test_backend(20, 5, 0);
        backend.write(b"alpha beta");

        assert!(!backend.update_selection(Point::new(0, 4), SelectionSide::Right));

        let selection = Selection::new(SelectionType::Simple, Point::new(0, 0), SelectionSide::Left);
        backend.set_selection(Some(&selection));
        assert!(backend.update_selection(Point::new(0, 4), SelectionSide::Right));
        assert_eq!(backend.selection_text().as_deref(), Some("alpha"));

        backend.set_selection(None);
        assert_eq!(backend.selection_text(), None);
    }

    #[test]
    fn semantic_selection_expands_to_word_boundaries() {
        let (mut backend, _events_rx) = test_backend(30, 5, 0);
        backend.write(b"alpha beta-two gamma");

        let mut selection =
            Selection::new(SelectionType::Semantic, Point::new(0, 8), SelectionSide::Left);
        selection.update(Point::new(0, 8), SelectionSide::Right);
        backend.set_selection(Some(&selection));

        assert_eq!(backend.selection_text().as_deref(), Some("beta-two"));
    }

    #[test]
    fn lines_selection_expands_to_full_lines() {
        let (mut backend, _events_rx) = test_backend(30, 5, 0);
        backend.write(b"first line\r\nsecond line\r\nthird");

        let mut selection =
            Selection::new(SelectionType::Lines, Point::new(0, 3), SelectionSide::Left);
        selection.update(Point::new(1, 3), SelectionSide::Right);
        backend.set_selection(Some(&selection));

        // The trailing newline is alacritty `selection_to_string` parity for
        // Lines selections.
        assert_eq!(
            backend.selection_text().as_deref(),
            Some("first line\nsecond line\n")
        );
    }

    #[test]
    fn selection_tracks_content_through_scrolling() {
        let (mut backend, _events_rx) = test_backend(20, 3, 100);
        backend.write(b"target\r\n");

        let mut selection =
            Selection::new(SelectionType::Simple, Point::new(0, 0), SelectionSide::Left);
        selection.update(Point::new(0, 5), SelectionSide::Right);
        backend.set_selection(Some(&selection));
        assert_eq!(backend.selection_text().as_deref(), Some("target"));

        // Push the selected line into scrollback; the tracked anchors follow.
        backend.write(b"a\r\nb\r\nc\r\nd\r\n");
        assert_eq!(backend.selection_text().as_deref(), Some("target"));
        let content = content(&mut backend);
        let range = content.selection.expect("selection should survive scrolling");
        assert!(range.start.line < 0, "selection should sit in scrollback");
    }

    // format_selection_buf characterization tests (SPEC.md §6 P5): pin the
    // formatter semantics the seam relies on for selection_text and
    // content_text.

    #[test]
    fn format_selection_buf_trims_trailing_whitespace() {
        let (mut backend, _events_rx) = test_backend(20, 5, 0);
        backend.write(b"hello   \r\nworld");

        let mut selection =
            Selection::new(SelectionType::Simple, Point::new(0, 0), SelectionSide::Left);
        selection.update(Point::new(1, 4), SelectionSide::Right);
        backend.set_selection(Some(&selection));

        assert_eq!(backend.selection_text().as_deref(), Some("hello\nworld"));
    }

    #[test]
    fn format_selection_buf_unwraps_soft_wrapped_lines() {
        let (mut backend, _events_rx) = test_backend(6, 5, 0);
        // 10 chars on a 6-column screen soft-wrap onto a second row.
        backend.write(b"abcdefghij");

        let mut selection =
            Selection::new(SelectionType::Simple, Point::new(0, 0), SelectionSide::Left);
        selection.update(Point::new(1, 3), SelectionSide::Right);
        backend.set_selection(Some(&selection));

        assert_eq!(backend.selection_text().as_deref(), Some("abcdefghij"));
    }

    #[test]
    fn format_selection_buf_keeps_wide_chars_whole() {
        let (mut backend, _events_rx) = test_backend(20, 5, 0);
        backend.write("a世b".as_bytes());

        let mut selection =
            Selection::new(SelectionType::Simple, Point::new(0, 0), SelectionSide::Left);
        selection.update(Point::new(0, 3), SelectionSide::Right);
        backend.set_selection(Some(&selection));

        assert_eq!(backend.selection_text().as_deref(), Some("a世b"));
    }

    #[test]
    fn content_text_covers_scrollback_and_screen() {
        let (mut backend, _events_rx) = test_backend(20, 3, 100);
        backend.write(b"one\r\ntwo\r\nthree\r\nfour");
        assert_eq!(backend.content_text(), "one\ntwo\nthree\nfour");
    }

    #[test]
    fn full_content_range_spans_the_grid() {
        let (mut backend, _events_rx) = test_backend(20, 3, 100);
        backend.write(b"one\r\ntwo\r\nthree\r\nfour");
        let range = backend.full_content_range();
        assert_eq!(range.start(), Point::new(-1, 0));
        assert_eq!(range.end(), Point::new(2, 19));
    }

    #[test]
    fn last_n_non_empty_lines_walks_logical_lines() {
        let (mut backend, _events_rx) = test_backend(6, 3, 100);
        backend.write(b"first\r\n\r\nabcdefghij\r\nlast");

        assert_eq!(
            backend.last_n_non_empty_lines(2),
            vec!["abcdefghij".to_string(), "last".to_string()]
        );
        assert_eq!(
            backend.last_n_non_empty_lines(10),
            vec![
                "first".to_string(),
                "abcdefghij".to_string(),
                "last".to_string()
            ]
        );
    }

    #[test]
    fn append_lines_appends_after_output() {
        let (mut backend, _events_rx) = test_backend(20, 5, 100);
        backend.write(b"output");
        backend.append_lines(&["Task finished", "exit code 0"]);

        assert_eq!(
            backend.last_n_non_empty_lines(3),
            vec![
                "output".to_string(),
                "Task finished".to_string(),
                "exit code 0".to_string()
            ]
        );
    }

    #[test]
    fn shrink_to_used_completes_and_preserves_content() {
        let (mut backend, _events_rx) = test_backend(20, 3, 100);
        backend.write(b"one\r\ntwo\r\nthree\r\nfour\r\nfive");
        backend.shrink_to_used();
        assert_eq!(backend.content_text(), "one\ntwo\nthree\nfour\nfive");
    }

    #[test]
    fn resize_updates_dimensions_and_clears_selection() {
        let (mut backend, _events_rx) = test_backend(20, 5, 100);
        backend.write(b"some text");
        let selection =
            Selection::new(SelectionType::Simple, Point::new(0, 0), SelectionSide::Left);
        backend.set_selection(Some(&selection));

        backend.resize(test_bounds(30, 8));

        assert_eq!(backend.screen_lines(), 8);
        let content = content(&mut backend);
        assert!(content.selection.is_none(), "resize drops the selection");
        assert_eq!(content.cells.iter().map(|c| c.point.column).max(), Some(29));
    }

    #[test]
    fn theme_colors_push_and_osc_overrides_survive() {
        let (mut backend, _events_rx) = test_backend(20, 5, 0);

        let mut palette = [Rgb { r: 0, g: 0, b: 0 }; 256];
        palette[1] = Rgb { r: 10, g: 20, b: 30 };
        backend.push_theme_colors(
            Rgb { r: 1, g: 1, b: 1 },
            Rgb { r: 2, g: 2, b: 2 },
            Rgb { r: 3, g: 3, b: 3 },
            &palette,
        );

        assert_eq!(backend.color(1), Some(Rgb { r: 10, g: 20, b: 30 }));
        assert_eq!(backend.color(256), Some(Rgb { r: 1, g: 1, b: 1 }));
        assert_eq!(backend.color(257), Some(Rgb { r: 2, g: 2, b: 2 }));
        assert_eq!(backend.color(258), Some(Rgb { r: 3, g: 3, b: 3 }));

        // A program-set OSC 4 override wins over the pushed default...
        backend.write(b"\x1b]4;1;rgb:aa/bb/cc\x1b\\");
        assert_eq!(
            backend.color(1),
            Some(Rgb { r: 0xaa, g: 0xbb, b: 0xcc })
        );

        // ...and survives a re-push on theme change.
        backend.push_theme_colors(
            Rgb { r: 1, g: 1, b: 1 },
            Rgb { r: 2, g: 2, b: 2 },
            Rgb { r: 3, g: 3, b: 3 },
            &palette,
        );
        assert_eq!(
            backend.color(1),
            Some(Rgb { r: 0xaa, g: 0xbb, b: 0xcc })
        );
    }

    #[test]
    fn callbacks_emit_backend_events_in_pty_order() {
        let (mut backend, mut events_rx) = test_backend(20, 5, 0);

        backend.write(b"\x07");
        assert!(matches!(
            drained_events(&mut events_rx).as_slice(),
            [TerminalBackendEvent::Bell]
        ));

        backend.write(b"\x1b]2;my title\x1b\\");
        assert!(matches!(
            drained_events(&mut events_rx).as_slice(),
            [TerminalBackendEvent::Title(title)] if title == "my title"
        ));

        backend.write(b"\x1b]2;\x1b\\");
        assert!(matches!(
            drained_events(&mut events_rx).as_slice(),
            [TerminalBackendEvent::ResetTitle]
        ));

        // OSC 52 clipboard write: "hello" base64-encoded.
        backend.write(b"\x1b]52;c;aGVsbG8=\x1b\\");
        assert!(matches!(
            drained_events(&mut events_rx).as_slice(),
            [TerminalBackendEvent::ClipboardStore(data)] if data == "hello"
        ));
    }

    #[test]
    fn display_only_backend_ignores_clipboard_writes() {
        let (events_tx, mut events_rx) = futures::channel::mpsc::unbounded();
        let mut backend = TerminalBackend::new_display_only(
            100,
            SettingsCursorShape::Block,
            test_bounds(20, 5),
            events_tx,
            AlternateScroll::On,
        );
        backend.write(b"\x1b]52;c;aGVsbG8=\x1b\\");
        assert!(drained_events(&mut events_rx).is_empty());
    }

    #[test]
    fn device_attributes_response_matches_alacritty_primary() {
        let (mut ghostty, mut ghostty_rx) = test_backend(20, 5, 0);
        ghostty.write(b"\x1b[c");
        let ghostty_response = pty_writes(drained_events(&mut ghostty_rx));

        let (events_tx, mut alacritty_rx) = futures::channel::mpsc::unbounded();
        let mut alacritty = crate::alacritty::TerminalBackend::new(
            100,
            SettingsCursorShape::Block,
            test_bounds(20, 5),
            events_tx,
            AlternateScroll::On,
        );
        alacritty.write(b"\x1b[c");
        let alacritty_response = pty_writes(drained_events(&mut alacritty_rx));

        assert_eq!(ghostty_response, alacritty_response);
        assert_eq!(ghostty_response, "\x1b[?6c");
    }

    #[test]
    fn size_report_answers_from_bounds() {
        let (mut backend, mut events_rx) = test_backend(20, 5, 0);
        backend.write(b"\x1b[18t");
        assert_eq!(pty_writes(drained_events(&mut events_rx)), "\x1b[8;5;20t");
    }

    #[test]
    fn with_renderable_cells_yields_owned_indexed_cells() {
        let (mut backend, _events_rx) = test_backend(20, 3, 0);
        backend.write(b"ab");

        let (count, first_two) = backend.with_renderable_cells(|cells| {
            let cells: Vec<_> = cells.collect();
            let first_two: String = cells.iter().take(2).map(|cell| cell.character()).collect();
            (cells.len(), first_two)
        });
        assert_eq!(count, 20 * 3);
        assert_eq!(first_two, "ab");
    }

    #[test]
    fn color_scheme_report_reflects_setting() {
        let (mut backend, mut events_rx) = test_backend(20, 3, 0);

        // Dark by default: CSI ?996n reports dark (997;1).
        backend.write(b"\x1b[?996n");
        assert_eq!(pty_writes(drained_events(&mut events_rx)), "\x1b[?997;1n");

        backend.set_color_scheme(false);
        backend.write(b"\x1b[?996n");
        assert_eq!(pty_writes(drained_events(&mut events_rx)), "\x1b[?997;2n");
    }

    #[test]
    fn default_cursor_style_applies_on_decscusr_reset() {
        let (mut backend, _events_rx) = test_backend(20, 3, 0);
        backend.set_default_cursor_style(SettingsCursorShape::Underline);

        // DECSCUSR 0 resets to the configured default style.
        backend.write(b"\x1b[0 q");
        let content = content(&mut backend);
        assert_eq!(content.cursor.shape, CursorShape::Underline);
    }

    #[test]
    fn cursor_blinking_change_is_derived_from_snapshot_diffs() {
        let (mut backend, mut events_rx) = test_backend(20, 5, 0);
        content(&mut backend);
        drained_events(&mut events_rx);

        // DECSCUSR 1: blinking block.
        backend.write(b"\x1b[1 q");
        content(&mut backend);
        let events = drained_events(&mut events_rx);
        assert!(
            events
                .iter()
                .any(|event| matches!(event, TerminalBackendEvent::CursorBlinkingChange)),
            "expected CursorBlinkingChange, got {events:?}"
        );
        assert!(backend.cursor_blinking());
    }

    // G6 point-arithmetic tests (SPEC.md §4.5): the alacritty implementation
    // is the oracle — every helper must agree with `Point::{add,sub}` and
    // `grid_clamp` over an exhaustive sweep of small grids. These oracle
    // tests retire with the alacritty dependency at P10.

    struct OracleDimensions {
        history: usize,
        screen_lines: usize,
        columns: usize,
    }

    impl alacritty_terminal::grid::Dimensions for OracleDimensions {
        fn total_lines(&self) -> usize {
            self.history + self.screen_lines
        }

        fn screen_lines(&self) -> usize {
            self.screen_lines
        }

        fn columns(&self) -> usize {
            self.columns
        }
    }

    fn oracle_cases() -> impl Iterator<Item = (GridDimensions, OracleDimensions, Point)> {
        (1..=3usize).flat_map(move |columns| {
            (1..=3usize).flat_map(move |screen_lines| {
                (0..=2usize).flat_map(move |history| {
                    let topmost = -(history as i32);
                    let bottommost = screen_lines as i32 - 1;
                    (topmost - 2..=bottommost + 2).flat_map(move |line| {
                        (0..=columns).map(move |column| {
                            (
                                GridDimensions {
                                    topmost_line: topmost,
                                    screen_lines,
                                    columns,
                                },
                                OracleDimensions {
                                    history,
                                    screen_lines,
                                    columns,
                                },
                                Point::new(line, column),
                            )
                        })
                    })
                })
            })
        })
    }

    fn from_alacritty_point(point: alacritty_terminal::index::Point) -> Point {
        Point::new(point.line.0, point.column.0)
    }

    fn to_alacritty_point(point: Point) -> alacritty_terminal::index::Point {
        alacritty_terminal::index::Point::new(
            alacritty_terminal::index::Line(point.line),
            alacritty_terminal::index::Column(point.column),
        )
    }

    fn oracle_boundary(boundary: PointBoundary) -> alacritty_terminal::index::Boundary {
        match boundary {
            PointBoundary::Grid => alacritty_terminal::index::Boundary::Grid,
            PointBoundary::Cursor => alacritty_terminal::index::Boundary::Cursor,
        }
    }

    #[test]
    fn point_clamp_matches_alacritty_oracle() {
        for (dimensions, oracle, point) in oracle_cases() {
            for boundary in [PointBoundary::Grid, PointBoundary::Cursor] {
                assert_eq!(
                    point_clamp(dimensions, boundary, point),
                    from_alacritty_point(
                        to_alacritty_point(point).grid_clamp(&oracle, oracle_boundary(boundary))
                    ),
                    "clamp mismatch at {point:?} {boundary:?} {dimensions:?}"
                );
            }
        }
    }

    #[test]
    fn point_add_matches_alacritty_oracle() {
        for (dimensions, oracle, point) in oracle_cases() {
            for boundary in [PointBoundary::Grid, PointBoundary::Cursor] {
                for rhs in 0..=7usize {
                    assert_eq!(
                        point_add(dimensions, boundary, point, rhs),
                        from_alacritty_point(to_alacritty_point(point).add(
                            &oracle,
                            oracle_boundary(boundary),
                            rhs
                        )),
                        "add mismatch at {point:?} + {rhs} {boundary:?} {dimensions:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn point_sub_matches_alacritty_oracle() {
        for (dimensions, oracle, point) in oracle_cases() {
            for boundary in [PointBoundary::Grid, PointBoundary::Cursor] {
                for rhs in 0..=7usize {
                    assert_eq!(
                        point_sub(dimensions, boundary, point, rhs),
                        from_alacritty_point(to_alacritty_point(point).sub(
                            &oracle,
                            oracle_boundary(boundary),
                            rhs
                        )),
                        "sub mismatch at {point:?} - {rhs} {boundary:?} {dimensions:?}"
                    );
                }
            }
        }
    }
}
