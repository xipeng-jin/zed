#[cfg(target_os = "windows")]
use std::num::NonZeroU32;
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::{borrow::Cow, io, ops::RangeInclusive, path::PathBuf, sync::Arc};

mod hyperlinks;

use alacritty_terminal::{
    event::{Event as AlacTermEvent, EventListener, Notify, WindowSize},
    event_loop::{EventLoop, Msg, Notifier},
    grid::{Dimensions, Grid, GridIterator, Row, Scroll as AlacScroll},
    index::{Boundary, Column, Direction as AlacDirection, Line, Point as AlacPoint},
    selection::{
        Selection as AlacSelection, SelectionRange as AlacSelectionRange,
        SelectionType as AlacSelectionType,
    },
    sync::FairMutex,
    term::{
        Config, Osc52, RenderableCursor, Term, TermMode,
        cell::{Cell as AlacCell, Flags, Hyperlink as AlacHyperlink},
        search::{Match, RegexIter, RegexSearch},
    },
    tty,
    vi_mode::{ViModeCursor, ViMotion as AlacViMotion},
    vte::ansi::{
        ClearMode, CursorShape as AlacCursorShape, CursorStyle as AlacCursorStyle, Handler,
        NamedPrivateMode, PrivateMode, Processor, StdSyncHandler,
    },
};
use anyhow::{Context as _, Result};
use futures::channel::mpsc::UnboundedSender;
use util::paths::PathStyle;
#[cfg(target_os = "windows")]
use windows::Win32::{Foundation::HANDLE, System::Threading::GetProcessId};

use crate::{
    Cell, CellExtra, CellFlags, Color, Content, Cursor, CursorShape, Hyperlink, IndexedCell, Modes,
    Point, PtyEvent, Range, RenderableCells, Rgb, Scroll, Search, Selection, SelectionRange,
    SelectionSide, SelectionType, TerminalBackendEvent, TerminalBounds, ViMotion,
    pty_info::ProcessIdGetter,
    terminal_settings::{AlternateScroll, CursorShape as SettingsCursorShape},
};

pub(super) use hyperlinks::{HyperlinkMatch, RegexSearches};

pub(super) type AlacrittyPty = tty::Pty;
pub(super) type AlacrittyTerm = Term<ZedListener>;
pub(super) type AlacrittyTermConfig = Config;
pub(super) type AlacrittyTermLock = FairMutex<AlacrittyTerm>;
pub(super) type AlacrittyGridIterator<'a> = GridIterator<'a, AlacCell>;

#[derive(Clone)]
pub(super) struct ZedListener(UnboundedSender<PtyEvent>);

#[derive(Clone, Debug)]
pub(super) struct AlacrittySearch {
    search: RegexSearch,
}

#[cfg(unix)]
impl From<&AlacrittyPty> for ProcessIdGetter {
    fn from(pty: &AlacrittyPty) -> Self {
        Self::new(pty.file().as_raw_fd(), pty.child().id())
    }
}

#[cfg(windows)]
impl From<&AlacrittyPty> for ProcessIdGetter {
    fn from(pty: &AlacrittyPty) -> Self {
        let child = pty.child_watcher();
        let handle = child.raw_handle();
        let fallback_pid = child.pid().unwrap_or_else(|| unsafe {
            NonZeroU32::new_unchecked(GetProcessId(HANDLE(handle as _)))
        });

        Self::new(handle as i32, u32::from(fallback_pid))
    }
}

pub(super) struct PtySender {
    notifier: Notifier,
}

impl PtySender {
    pub(super) fn notify(&self, input: impl Into<Cow<'static, [u8]>>) {
        self.notifier.notify(input);
    }

    pub(super) fn resize(&self, bounds: TerminalBounds) {
        if let Err(error) = self
            .notifier
            .0
            .send(Msg::Resize(window_size_from_terminal_bounds(bounds)))
        {
            log::error!("failed to resize alacritty pty: {error}");
        }
    }

    pub(super) fn shutdown(&self) {
        if let Err(error) = self.notifier.0.send(Msg::Shutdown) {
            log::debug!("failed to shut down alacritty pty loop: {error}");
        }
    }
}

fn window_size_from_terminal_bounds(bounds: TerminalBounds) -> WindowSize {
    WindowSize {
        num_lines: bounds.num_lines() as u16,
        num_cols: bounds.num_columns() as u16,
        cell_width: f32::from(bounds.cell_width()) as u16,
        cell_height: f32::from(bounds.line_height()) as u16,
    }
}

/// The alacritty-backed terminal core: the emulator grid, its config, and the
/// parser feeding it. `ghostty::TerminalBackend` must expose this same inherent
/// method surface so the swap only re-points one import and the construction
/// call (docs/ghostty-migration/SPEC.md §4.1); all locking of the shared
/// terminal state stays behind this struct.
pub(super) struct TerminalBackend {
    term: Arc<AlacrittyTermLock>,
    config: AlacrittyTermConfig,
    output_processor: Processor<StdSyncHandler>,
}

impl TerminalBackend {
    pub(super) fn new(
        scrolling_history: usize,
        cursor_shape: SettingsCursorShape,
        bounds: TerminalBounds,
        events_tx: UnboundedSender<PtyEvent>,
        alternate_scroll: AlternateScroll,
    ) -> Self {
        Self::with_config(
            pty_term_config(scrolling_history, cursor_shape),
            bounds,
            events_tx,
            alternate_scroll,
        )
    }

    pub(super) fn new_display_only(
        scrolling_history: usize,
        cursor_shape: SettingsCursorShape,
        bounds: TerminalBounds,
        events_tx: UnboundedSender<PtyEvent>,
        alternate_scroll: AlternateScroll,
    ) -> Self {
        Self::with_config(
            display_only_term_config(scrolling_history, cursor_shape),
            bounds,
            events_tx,
            alternate_scroll,
        )
    }

    fn with_config(
        config: AlacrittyTermConfig,
        bounds: TerminalBounds,
        events_tx: UnboundedSender<PtyEvent>,
        alternate_scroll: AlternateScroll,
    ) -> Self {
        let mut term = Term::new(config.clone(), &bounds, ZedListener(events_tx));

        if let AlternateScroll::Off = alternate_scroll {
            term.unset_private_mode(PrivateMode::Named(NamedPrivateMode::AlternateScroll));
        }

        Self {
            term: Arc::new(FairMutex::new(term)),
            config,
            output_processor: Processor::new(),
        }
    }

    pub(super) fn spawn_event_loop(
        &self,
        events_tx: UnboundedSender<PtyEvent>,
        pty: AlacrittyPty,
        drain_on_exit: bool,
    ) -> Result<PtySender> {
        let event_loop = EventLoop::new(
            self.term.clone(),
            ZedListener(events_tx),
            pty,
            drain_on_exit,
            false,
        )
        .context("failed to create event loop")?;
        let pty_tx = event_loop.channel();
        let _io_thread = event_loop.spawn();

        Ok(PtySender {
            notifier: Notifier(pty_tx),
        })
    }

    pub(super) fn stream_ingest(&self) -> StreamIngest {
        StreamIngest {
            term: self.term.clone(),
            processor: Processor::new(),
        }
    }

    pub(super) fn write(&mut self, bytes: &[u8]) {
        let mut term = self.term.lock();
        self.output_processor.advance(&mut *term, bytes);
    }

    pub(super) fn set_default_cursor_style(&mut self, cursor_shape: SettingsCursorShape) {
        self.config.default_cursor_style = alacritty_cursor_style(cursor_shape);
        self.term.lock().set_options(self.config.clone());
    }

    pub(super) fn make_content(&mut self, last_content: &Content) -> Content {
        make_content(&self.term.lock_unfair(), last_content)
    }

    pub(super) fn with_renderable_cells<R>(
        &self,
        f: impl for<'a> FnOnce(RenderableCells<'a>) -> R,
    ) -> R {
        let term = self.term.lock_unfair();
        let content = term.renderable_content();
        f(RenderableCells::new(content.display_iter))
    }

    pub(super) fn total_lines(&self) -> usize {
        self.term.lock_unfair().total_lines()
    }

    pub(super) fn screen_lines(&self) -> usize {
        self.term.lock_unfair().screen_lines()
    }

    pub(super) fn display_offset(&self) -> usize {
        self.term.lock_unfair().grid().display_offset()
    }

    pub(super) fn content_text(&self) -> String {
        let term = self.term.lock_unfair();
        let start = AlacPoint::new(term.topmost_line(), Column(0));
        let end = AlacPoint::new(term.bottommost_line(), term.last_column());
        term.bounds_to_string(start, end)
    }

    pub(super) fn full_content_range(&self) -> Range {
        let term = self.term.lock();
        let start = AlacPoint::new(term.topmost_line(), Column(0));
        let end = AlacPoint::new(term.bottommost_line(), term.last_column());
        Range::from_alacritty(start..=end)
    }

    pub(super) fn last_n_non_empty_lines(&self, line_count: usize) -> Vec<String> {
        last_non_empty_lines(&self.term.lock_unfair(), line_count)
    }

    pub(super) fn cursor_blinking(&self) -> bool {
        self.term.lock().cursor_style().blinking
    }

    pub(super) fn color(&self, index: usize) -> Option<Rgb> {
        self.term.lock().colors()[index].map(Rgb::from_vte)
    }

    pub(super) fn resize(&mut self, bounds: TerminalBounds) {
        self.term.lock_unfair().resize(bounds);
    }

    pub(super) fn scroll_display(&mut self, scroll: Scroll) {
        self.term
            .lock_unfair()
            .scroll_display(scroll.to_alacritty());
    }

    pub(super) fn scroll_to_point(&mut self, point: Point) {
        self.term
            .lock_unfair()
            .scroll_to_point(point.to_alacritty());
    }

    pub(super) fn clear(&mut self) {
        clear_saved_screen(&mut self.term.lock_unfair());
    }

    pub(super) fn shrink_to_used(&mut self) {
        self.term.lock().grid_mut().truncate();
    }

    /// Appends a stringified task summary to the terminal, after its output.
    ///
    /// This must only be called after the terminal's PTY is no longer alive.
    /// New text being added to the terminal here uses "less public" APIs,
    /// which do not maintain the entire terminal state intact.
    ///
    ///
    /// The library
    ///
    /// * does not increment inner grid cursor's _lines_ on `input` calls
    ///   (but displaying the lines correctly and incrementing cursor's columns)
    ///
    /// * ignores `\n` and \r` character input, requiring the `newline` call instead
    ///
    /// * does not alter grid state after `newline` call
    ///   so its `bottommost_line` is always the same additions, and
    ///   the cursor's `point` is not updated to the new line and column values
    ///
    /// * ??? there could be more consequences, and any further "proper" streaming from the PTY might bug and/or panic.
    ///   Still, subsequent `append_lines` invocations are possible and display the contents correctly.
    ///
    /// Despite the quirks, this is the simplest approach to appending text to the terminal: its alternative, `grid_mut` manipulations,
    /// do not properly set the scrolling state and display odd text after appending; also those manipulations are more tedious and error-prone.
    /// The function achieves proper display and scrolling capabilities, at a cost of grid state not properly synchronized.
    /// This is enough for printing moderately-sized texts like task summaries, but might break or perform poorly for larger texts.
    pub(super) fn append_lines(&mut self, text_lines: &[&str]) {
        let mut term = self.term.lock();
        term.newline();
        term.grid_mut().cursor.point.column = Column(0);
        for line in text_lines {
            for character in line.chars() {
                term.input(character);
            }
            term.newline();
            term.grid_mut().cursor.point.column = Column(0);
        }
    }

    pub(super) fn set_selection(&mut self, selection: Option<&Selection>) {
        self.term.lock_unfair().selection = selection.map(Selection::to_alacritty);
    }

    pub(super) fn update_selection(&mut self, point: Point, side: SelectionSide) -> bool {
        let mut term = self.term.lock_unfair();
        let Some(mut selection) = term.selection.take() else {
            return false;
        };
        selection.update(point.to_alacritty(), side.to_alacritty());
        term.selection = Some(selection);
        true
    }

    pub(super) fn selection_text(&self) -> Option<String> {
        self.term.lock_unfair().selection_to_string()
    }

    pub(super) fn toggle_vi_mode(&mut self) {
        self.term.lock_unfair().toggle_vi_mode();
    }

    pub(super) fn vi_motion(&mut self, motion: ViMotion) {
        self.term.lock_unfair().vi_motion(motion.to_alacritty());
    }

    pub(super) fn vi_goto_point(&mut self, point: Point) {
        self.term.lock_unfair().vi_goto_point(point.to_alacritty());
    }

    pub(super) fn update_vi_cursor_for_scroll(&mut self, scroll: Scroll) {
        update_vi_cursor_for_scroll(&mut self.term.lock_unfair(), scroll);
    }

    pub(super) fn update_selection_to_vi_cursor(&mut self) -> Option<Point> {
        update_selection_to_vi_cursor(&mut self.term.lock_unfair())
    }

    pub(super) fn prepare_search(&self, searcher: Search) -> PreparedSearch {
        PreparedSearch {
            term: self.term.clone(),
            searcher,
        }
    }

    pub(super) fn find_from_terminal_point(
        &self,
        point: Point,
        regex_searches: &mut RegexSearches,
        path_style: PathStyle,
    ) -> Option<HyperlinkMatch> {
        let term = self.term.lock();
        let point = point.to_alacritty().grid_clamp(&*term, Boundary::Grid);
        hyperlinks::find_from_grid_point(&term, point, regex_searches, path_style)
    }
}

/// Feeds raw bytes from a background reader (the headless subprocess path)
/// into the shared emulator; each output stream gets its own parser while the
/// terminal mutex serializes grid mutation.
pub(super) struct StreamIngest {
    term: Arc<AlacrittyTermLock>,
    processor: Processor<StdSyncHandler>,
}

impl StreamIngest {
    pub(super) fn advance(&mut self, bytes: &[u8]) {
        let mut term = self.term.lock();
        self.processor.advance(&mut *term, bytes);
    }
}

/// A grid search prepared on the foreground and run to completion on a
/// background thread, locking the shared emulator for the duration.
pub(super) struct PreparedSearch {
    term: Arc<AlacrittyTermLock>,
    searcher: Search,
}

impl PreparedSearch {
    pub(super) fn run(self) -> Vec<Range> {
        let term = self.term.lock();
        let mut searcher = self.searcher.into_alacritty();
        all_search_matches(&term, &mut searcher)
            .map(Range::from_alacritty)
            .collect()
    }
}

fn display_only_term_config(
    scrolling_history: usize,
    cursor_shape: SettingsCursorShape,
) -> AlacrittyTermConfig {
    Config {
        scrolling_history,
        default_cursor_style: alacritty_cursor_style(cursor_shape),
        osc52: Osc52::Disabled,
        ..Config::default()
    }
}

fn pty_term_config(
    scrolling_history: usize,
    cursor_shape: SettingsCursorShape,
) -> AlacrittyTermConfig {
    Config {
        scrolling_history,
        default_cursor_style: alacritty_cursor_style(cursor_shape),
        ..Config::default()
    }
}

#[cfg(not(windows))]
pub(super) fn current_child_signal_mask() -> io::Result<tty::SignalMask> {
    tty::SignalMask::current()
}

pub(super) fn pty_options(
    shell: Option<(String, Vec<String>)>,
    working_directory: Option<PathBuf>,
    env: impl IntoIterator<Item = (String, String)>,
    #[cfg(not(windows))] child_signal_mask: Option<tty::SignalMask>,
    #[cfg(windows)] escape_args: bool,
) -> tty::Options {
    tty::Options {
        shell: shell.map(|(program, args)| tty::Shell::new(program, args)),
        working_directory,
        drain_on_exit: true,
        env: env.into_iter().collect(),
        #[cfg(not(windows))]
        child_signal_mask,
        #[cfg(windows)]
        escape_args,
    }
}

pub(super) fn open_pty(
    options: &tty::Options,
    bounds: TerminalBounds,
    window_id: u64,
) -> io::Result<AlacrittyPty> {
    tty::new(options, window_size_from_terminal_bounds(bounds), window_id)
}

fn alacritty_cursor_style(cursor_shape: SettingsCursorShape) -> AlacCursorStyle {
    AlacCursorStyle {
        shape: alacritty_cursor_shape(cursor_shape),
        blinking: false,
    }
}

fn alacritty_cursor_shape(cursor_shape: SettingsCursorShape) -> AlacCursorShape {
    match cursor_shape {
        SettingsCursorShape::Block => AlacCursorShape::Block,
        SettingsCursorShape::Underline => AlacCursorShape::Underline,
        SettingsCursorShape::Bar => AlacCursorShape::Beam,
        SettingsCursorShape::Hollow => AlacCursorShape::HollowBlock,
    }
}

impl Dimensions for TerminalBounds {
    /// Note: this is supposed to be for the back buffer's length,
    /// but we exclusively use it to resize the terminal, which does not
    /// use this method. We still have to implement it for the trait though,
    /// hence, this comment.
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }

    fn screen_lines(&self) -> usize {
        self.num_lines()
    }

    fn columns(&self) -> usize {
        self.num_columns()
    }
}

impl From<AlacTermEvent> for TerminalBackendEvent {
    fn from(event: AlacTermEvent) -> Self {
        match event {
            AlacTermEvent::MouseCursorDirty => Self::MouseCursorDirty,
            AlacTermEvent::Title(title) => Self::Title(title),
            AlacTermEvent::ResetTitle => Self::ResetTitle,
            AlacTermEvent::ClipboardStore(_, data) => Self::ClipboardStore(data),
            AlacTermEvent::ClipboardLoad(_, format) => Self::ClipboardLoad(format),
            AlacTermEvent::ColorRequest(index, format) => {
                Self::ColorRequest(index, Arc::new(move |color: Rgb| format(color.to_vte())))
            }
            AlacTermEvent::PtyWrite(output) => Self::PtyWrite(output),
            AlacTermEvent::TextAreaSizeRequest(format) => {
                Self::TextAreaSizeRequest(Arc::new(move |bounds| {
                    format(window_size_from_terminal_bounds(bounds))
                }))
            }
            AlacTermEvent::CursorBlinkingChange => Self::CursorBlinkingChange,
            AlacTermEvent::Wakeup => Self::Wakeup,
            AlacTermEvent::Bell => Self::Bell,
            AlacTermEvent::Exit => Self::Exit,
            AlacTermEvent::ChildExit(status) => Self::ChildExit(status),
        }
    }
}

impl EventListener for ZedListener {
    fn send_event(&self, event: AlacTermEvent) {
        self.0.unbounded_send(PtyEvent::Event(event.into())).ok();
    }
}

impl Scroll {
    fn to_alacritty(self) -> AlacScroll {
        match self {
            Self::Delta(delta) => AlacScroll::Delta(delta),
            Self::PageUp => AlacScroll::PageUp,
            Self::PageDown => AlacScroll::PageDown,
            Self::Top => AlacScroll::Top,
            Self::Bottom => AlacScroll::Bottom,
        }
    }
}

impl ViMotion {
    fn to_alacritty(self) -> AlacViMotion {
        match self {
            Self::Up => AlacViMotion::Up,
            Self::Down => AlacViMotion::Down,
            Self::Left => AlacViMotion::Left,
            Self::Right => AlacViMotion::Right,
            Self::First => AlacViMotion::First,
            Self::Last => AlacViMotion::Last,
            Self::FirstOccupied => AlacViMotion::FirstOccupied,
            Self::High => AlacViMotion::High,
            Self::Middle => AlacViMotion::Middle,
            Self::Low => AlacViMotion::Low,
            Self::WordLeft => AlacViMotion::WordLeft,
            Self::WordRight => AlacViMotion::WordRight,
            Self::WordRightEnd => AlacViMotion::WordRightEnd,
            Self::Bracket => AlacViMotion::Bracket,
            Self::ParagraphUp => AlacViMotion::ParagraphUp,
            Self::ParagraphDown => AlacViMotion::ParagraphDown,
        }
    }
}

impl Search {
    pub fn new(search: &str) -> Option<Self> {
        Some(Self {
            search: AlacrittySearch {
                search: RegexSearch::new(search).ok()?,
            },
        })
    }

    fn into_alacritty(self) -> RegexSearch {
        self.search.search
    }
}

impl SelectionSide {
    fn to_alacritty(self) -> AlacDirection {
        match self {
            Self::Left => AlacDirection::Left,
            Self::Right => AlacDirection::Right,
        }
    }
}

impl SelectionType {
    fn to_alacritty(self) -> AlacSelectionType {
        match self {
            Self::Simple => AlacSelectionType::Simple,
            Self::Semantic => AlacSelectionType::Semantic,
            Self::Lines => AlacSelectionType::Lines,
        }
    }
}

impl Selection {
    fn to_alacritty(&self) -> AlacSelection {
        let mut selection = AlacSelection::new(
            self.ty.to_alacritty(),
            self.start.point.to_alacritty(),
            self.start.side.to_alacritty(),
        );
        if self.start.point != self.end.point || self.start.side != self.end.side {
            selection.update(self.end.point.to_alacritty(), self.end.side.to_alacritty());
        }
        selection
    }
}

fn terminal_hyperlink_from_alacritty(hyperlink: AlacHyperlink) -> Hyperlink {
    Hyperlink::new(Some(hyperlink.id()), hyperlink.uri().to_string())
}

fn terminal_cell_from_alacritty(cell: &AlacCell) -> Cell {
    let zerowidth = cell.zerowidth().unwrap_or_default();
    let hyperlink = cell.hyperlink().map(terminal_hyperlink_from_alacritty);
    let extra = (!zerowidth.is_empty() || hyperlink.is_some()).then(|| {
        Arc::new(CellExtra {
            zerowidth: zerowidth.to_vec(),
            hyperlink,
        })
    });

    Cell {
        c: cell.c,
        fg: Color::from_vte(cell.fg),
        bg: Color::from_vte(cell.bg),
        flags: terminal_cell_flags_from_alacritty(cell.flags),
        extra,
    }
}

fn terminal_cell_flags_from_alacritty(flags: Flags) -> CellFlags {
    let mut cell_flags = CellFlags::empty();
    for (alacritty_flag, cell_flag) in [
        (Flags::INVERSE, CellFlags::INVERSE),
        (Flags::BOLD, CellFlags::BOLD),
        (Flags::ITALIC, CellFlags::ITALIC),
        (Flags::DIM, CellFlags::DIM),
        (Flags::STRIKEOUT, CellFlags::STRIKEOUT),
        (Flags::WIDE_CHAR_SPACER, CellFlags::WIDE_CHAR_SPACER),
        (Flags::UNDERLINE, CellFlags::UNDERLINE),
        (Flags::DOUBLE_UNDERLINE, CellFlags::DOUBLE_UNDERLINE),
        (Flags::UNDERCURL, CellFlags::UNDERCURL),
        (Flags::DOTTED_UNDERLINE, CellFlags::DOTTED_UNDERLINE),
        (Flags::DASHED_UNDERLINE, CellFlags::DASHED_UNDERLINE),
    ] {
        if flags.contains(alacritty_flag) {
            cell_flags.insert(cell_flag);
        }
    }
    cell_flags
}

impl<'a> RenderableCells<'a> {
    pub(super) fn new(cells: GridIterator<'a, AlacCell>) -> Self {
        Self { cells }
    }
}

impl Iterator for RenderableCells<'_> {
    type Item = IndexedCell;

    fn next(&mut self) -> Option<Self::Item> {
        self.cells.next().map(|cell| IndexedCell {
            point: terminal_point_from_alacritty(cell.point),
            cell: terminal_cell_from_alacritty(cell.cell),
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.cells.size_hint()
    }
}

impl Modes {
    #[cfg(test)]
    fn to_alacritty(self) -> TermMode {
        let mut mode = TermMode::empty();
        add_alacritty_mode(&mut mode, self, Self::APP_CURSOR, TermMode::APP_CURSOR);
        add_alacritty_mode(&mut mode, self, Self::APP_KEYPAD, TermMode::APP_KEYPAD);
        add_alacritty_mode(&mut mode, self, Self::SHOW_CURSOR, TermMode::SHOW_CURSOR);
        add_alacritty_mode(&mut mode, self, Self::LINE_WRAP, TermMode::LINE_WRAP);
        add_alacritty_mode(&mut mode, self, Self::ORIGIN, TermMode::ORIGIN);
        add_alacritty_mode(&mut mode, self, Self::INSERT, TermMode::INSERT);
        add_alacritty_mode(
            &mut mode,
            self,
            Self::LINE_FEED_NEW_LINE,
            TermMode::LINE_FEED_NEW_LINE,
        );
        add_alacritty_mode(&mut mode, self, Self::FOCUS_IN_OUT, TermMode::FOCUS_IN_OUT);
        add_alacritty_mode(
            &mut mode,
            self,
            Self::ALTERNATE_SCROLL,
            TermMode::ALTERNATE_SCROLL,
        );
        add_alacritty_mode(
            &mut mode,
            self,
            Self::BRACKETED_PASTE,
            TermMode::BRACKETED_PASTE,
        );
        add_alacritty_mode(&mut mode, self, Self::SGR_MOUSE, TermMode::SGR_MOUSE);
        add_alacritty_mode(&mut mode, self, Self::UTF8_MOUSE, TermMode::UTF8_MOUSE);
        add_alacritty_mode(&mut mode, self, Self::ALT_SCREEN, TermMode::ALT_SCREEN);
        add_alacritty_mode(
            &mut mode,
            self,
            Self::MOUSE_REPORT_CLICK,
            TermMode::MOUSE_REPORT_CLICK,
        );
        add_alacritty_mode(&mut mode, self, Self::MOUSE_DRAG, TermMode::MOUSE_DRAG);
        add_alacritty_mode(&mut mode, self, Self::MOUSE_MOTION, TermMode::MOUSE_MOTION);
        add_alacritty_mode(&mut mode, self, Self::VI, TermMode::VI);
        mode
    }
}

fn terminal_modes_from_alacritty(mode: TermMode) -> Modes {
    let mut terminal_modes = Modes::empty();
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::APP_CURSOR,
        Modes::APP_CURSOR,
    );
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::APP_KEYPAD,
        Modes::APP_KEYPAD,
    );
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::SHOW_CURSOR,
        Modes::SHOW_CURSOR,
    );
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::LINE_WRAP,
        Modes::LINE_WRAP,
    );
    add_terminal_mode(&mut terminal_modes, mode, TermMode::ORIGIN, Modes::ORIGIN);
    add_terminal_mode(&mut terminal_modes, mode, TermMode::INSERT, Modes::INSERT);
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::LINE_FEED_NEW_LINE,
        Modes::LINE_FEED_NEW_LINE,
    );
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::FOCUS_IN_OUT,
        Modes::FOCUS_IN_OUT,
    );
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::ALTERNATE_SCROLL,
        Modes::ALTERNATE_SCROLL,
    );
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::BRACKETED_PASTE,
        Modes::BRACKETED_PASTE,
    );
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::SGR_MOUSE,
        Modes::SGR_MOUSE,
    );
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::UTF8_MOUSE,
        Modes::UTF8_MOUSE,
    );
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::ALT_SCREEN,
        Modes::ALT_SCREEN,
    );
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::MOUSE_REPORT_CLICK,
        Modes::MOUSE_REPORT_CLICK,
    );
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::MOUSE_DRAG,
        Modes::MOUSE_DRAG,
    );
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::MOUSE_MOTION,
        Modes::MOUSE_MOTION,
    );
    add_terminal_mode(&mut terminal_modes, mode, TermMode::VI, Modes::VI);
    terminal_modes
}

fn add_terminal_mode(
    terminal_modes: &mut Modes,
    alacritty_modes: TermMode,
    alacritty_mode: TermMode,
    terminal_mode: Modes,
) {
    if alacritty_modes.contains(alacritty_mode) {
        terminal_modes.insert(terminal_mode);
    }
}

#[cfg(test)]
fn add_alacritty_mode(
    alacritty_modes: &mut TermMode,
    terminal_modes: Modes,
    terminal_mode: Modes,
    alacritty_mode: TermMode,
) {
    if terminal_modes.contains(terminal_mode) {
        alacritty_modes.insert(alacritty_mode);
    }
}

impl Cursor {
    fn from_alacritty(cursor: RenderableCursor) -> Self {
        Self {
            shape: terminal_cursor_shape_from_alacritty(cursor.shape),
            point: terminal_point_from_alacritty(cursor.point),
        }
    }
}

fn terminal_cursor_shape_from_alacritty(shape: AlacCursorShape) -> CursorShape {
    match shape {
        AlacCursorShape::Block => CursorShape::Block,
        AlacCursorShape::Underline => CursorShape::Underline,
        AlacCursorShape::Beam => CursorShape::Bar,
        AlacCursorShape::HollowBlock => CursorShape::HollowBlock,
        AlacCursorShape::Hidden => CursorShape::Hidden,
    }
}

impl Point {
    fn to_alacritty(self) -> AlacPoint {
        AlacPoint::new(Line(self.line), Column(self.column))
    }
}

fn terminal_point_from_alacritty(point: AlacPoint) -> Point {
    Point {
        line: point.line.0,
        column: point.column.0,
    }
}

impl Range {
    #[cfg(test)]
    fn to_alacritty(self) -> RangeInclusive<AlacPoint> {
        self.start.to_alacritty()..=self.end.to_alacritty()
    }

    fn from_alacritty(range: RangeInclusive<AlacPoint>) -> Self {
        Self {
            start: terminal_point_from_alacritty(*range.start()),
            end: terminal_point_from_alacritty(*range.end()),
        }
    }
}

fn terminal_selection_range_from_alacritty(range: AlacSelectionRange) -> SelectionRange {
    SelectionRange {
        start: terminal_point_from_alacritty(range.start),
        end: terminal_point_from_alacritty(range.end),
        is_block: range.is_block,
    }
}

fn clear_saved_screen(term: &mut Term<ZedListener>) {
    term.clear_screen(ClearMode::Saved);

    let cursor = term.grid().cursor.point;

    term.grid_mut().reset_region(..cursor.line);

    let line = term.grid()[cursor.line][..Column(term.grid().columns())]
        .iter()
        .cloned()
        .enumerate()
        .collect::<Vec<(usize, AlacCell)>>();

    for (index, cell) in line {
        term.grid_mut()[Line(0)][Column(index)] = cell;
    }

    term.grid_mut().cursor.point = AlacPoint::new(Line(0), term.grid_mut().cursor.point.column);
    let new_cursor = term.grid().cursor.point;

    if (new_cursor.line.0 as usize) < term.screen_lines() - 1 {
        term.grid_mut().reset_region((new_cursor.line + 1)..);
    }
}

fn make_content(term: &Term<ZedListener>, last_content: &Content) -> Content {
    let content = term.renderable_content();

    let estimated_size = content.display_iter.size_hint().0;
    let mut cells = Vec::with_capacity(estimated_size);

    cells.extend(content.display_iter.map(|ic| IndexedCell {
        point: terminal_point_from_alacritty(ic.point),
        cell: terminal_cell_from_alacritty(ic.cell),
    }));

    let selection_text = if content.selection.is_some() {
        term.selection_to_string()
    } else {
        None
    };

    let bottom_line = term.screen_lines() as i32 - 1 - content.display_offset as i32;
    let bottom_row_occupied = content.cursor.point.line.0 >= bottom_line
        || cells
            .iter()
            .rev()
            .take_while(|cell| cell.point.line >= bottom_line)
            .any(|cell| cell.cell.character() != ' ');

    Content {
        cells,
        mode: terminal_modes_from_alacritty(content.mode),
        display_offset: content.display_offset,
        selection_text,
        selection: content
            .selection
            .map(terminal_selection_range_from_alacritty),
        cursor: Cursor::from_alacritty(content.cursor),
        cursor_char: term.grid()[content.cursor.point].c,
        terminal_bounds: last_content.terminal_bounds,
        last_hovered_word: last_content.last_hovered_word.clone(),
        scrolled_to_top: content.display_offset == term.history_size(),
        scrolled_to_bottom: content.display_offset == 0,
        bottom_row_occupied,
    }
}

fn last_non_empty_lines(term: &Term<ZedListener>, line_count: usize) -> Vec<String> {
    let grid = term.grid();
    let mut lines = Vec::new();

    let mut current_line = grid.bottommost_line().0;
    let topmost_line = grid.topmost_line().0;

    while current_line >= topmost_line && lines.len() < line_count {
        let (logical_line_start, logical_line) =
            logical_line_for_row(grid, current_line, topmost_line);

        if let Some(line) = process_line(logical_line) {
            lines.push(line);
        }

        current_line = logical_line_start - 1;
    }

    lines.reverse();
    lines
}

fn update_vi_cursor_for_scroll(term: &mut Term<ZedListener>, scroll: Scroll) {
    match scroll {
        Scroll::Delta(delta) => {
            term.vi_mode_cursor = term.vi_mode_cursor.scroll(term, delta);
        }
        Scroll::PageUp => {
            let lines = term.screen_lines() as i32;
            term.vi_mode_cursor = term.vi_mode_cursor.scroll(term, lines);
        }
        Scroll::PageDown => {
            let lines = -(term.screen_lines() as i32);
            term.vi_mode_cursor = term.vi_mode_cursor.scroll(term, lines);
        }
        Scroll::Top => {
            let point = AlacPoint::new(term.topmost_line(), Column(0));
            term.vi_mode_cursor = ViModeCursor::new(point);
        }
        Scroll::Bottom => {
            let point = AlacPoint::new(term.bottommost_line(), Column(0));
            term.vi_mode_cursor = ViModeCursor::new(point);
        }
    }
}

fn update_selection_to_vi_cursor(term: &mut Term<ZedListener>) -> Option<Point> {
    let mut selection = term.selection.take()?;
    let point = term.vi_mode_cursor.point;
    selection.update(point, AlacDirection::Right);
    term.selection = Some(selection);
    Some(terminal_point_from_alacritty(point))
}

fn logical_line_for_row(grid: &Grid<AlacCell>, current: i32, topmost: i32) -> (i32, String) {
    let start = find_logical_line_start(grid, current, topmost);
    let mut line = String::new();
    for row in start..=current {
        line.push_str(&row_to_string(&grid[Line(row)]));
    }
    (start, line)
}

fn find_logical_line_start(grid: &Grid<AlacCell>, current: i32, topmost: i32) -> i32 {
    let mut line_start = current;
    while line_start > topmost {
        let previous_line = Line(line_start - 1);
        let last_cell = &grid[previous_line][Column(grid.columns() - 1)];
        if !last_cell.flags.contains(Flags::WRAPLINE) {
            break;
        }
        line_start -= 1;
    }
    line_start
}

fn row_to_string(row: &Row<AlacCell>) -> String {
    row[..Column(row.len())]
        .iter()
        .map(|cell| cell.c)
        .collect::<String>()
}

fn process_line(line: String) -> Option<String> {
    let trimmed = line.trim_end().to_string();
    if !trimmed.is_empty() {
        Some(trimmed)
    } else {
        None
    }
}

fn all_search_matches<'a, T>(
    term: &'a Term<T>,
    regex: &'a mut RegexSearch,
) -> impl Iterator<Item = Match> + 'a {
    let start = AlacPoint::new(term.grid().topmost_line(), Column(0));
    let end = AlacPoint::new(term.grid().bottommost_line(), term.grid().last_column());
    RegexIter::new(start, end, AlacDirection::Right, term, regex)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The former `terminal_hyperlink_from_alacritty_keeps_alacritty_storage` and
    // `terminal_cell_from_alacritty_shares_extra_storage` tests asserted that
    // conversion shared alacritty's Arc storage; owned domain types copy at
    // snapshot build instead. Retired via
    // docs/ghostty-migration/divergence-ledger.md (P2-001, P2-002); the
    // semantic halves of those round-trips continue below.
    #[test]
    fn terminal_hyperlink_from_alacritty_preserves_id_and_uri() {
        let hyperlink = AlacHyperlink::new(Some("id"), "https://example.com".to_string());
        let hyperlink = terminal_hyperlink_from_alacritty(hyperlink);

        assert_eq!(hyperlink.id(), Some("id"));
        assert_eq!(hyperlink.uri(), "https://example.com");
    }

    #[test]
    fn terminal_cell_from_alacritty_preserves_zerowidth() {
        let mut cell = AlacCell::default();
        cell.push_zerowidth('a');

        let converted = terminal_cell_from_alacritty(&cell);

        assert_eq!(converted.zerowidth(), Some(&['a'][..]));
    }

    #[test]
    fn terminal_modes_round_trip_alacritty_flags() {
        let alacritty_modes = TermMode::APP_CURSOR
            | TermMode::BRACKETED_PASTE
            | TermMode::ALT_SCREEN
            | TermMode::MOUSE_DRAG
            | TermMode::SGR_MOUSE
            | TermMode::VI;

        let terminal_modes = terminal_modes_from_alacritty(alacritty_modes);
        assert!(terminal_modes.contains(Modes::APP_CURSOR));
        assert!(terminal_modes.contains(Modes::BRACKETED_PASTE));
        assert!(terminal_modes.contains(Modes::ALT_SCREEN));
        assert!(terminal_modes.contains(Modes::MOUSE_DRAG));
        assert!(terminal_modes.intersects(Modes::MOUSE_MODE));
        assert!(terminal_modes.contains(Modes::SGR_MOUSE));
        assert!(terminal_modes.contains(Modes::VI));
        assert!(!terminal_modes.contains(Modes::MOUSE_REPORT_CLICK));

        let alacritty_modes = terminal_modes.to_alacritty();
        assert!(alacritty_modes.contains(TermMode::APP_CURSOR));
        assert!(alacritty_modes.contains(TermMode::BRACKETED_PASTE));
        assert!(alacritty_modes.contains(TermMode::ALT_SCREEN));
        assert!(alacritty_modes.contains(TermMode::MOUSE_DRAG));
        assert!(alacritty_modes.contains(TermMode::SGR_MOUSE));
        assert!(alacritty_modes.contains(TermMode::VI));
        assert!(!alacritty_modes.contains(TermMode::MOUSE_REPORT_CLICK));
    }

    #[test]
    fn terminal_selection_range_round_trip_alacritty_range() {
        let alacritty_range = AlacSelectionRange {
            start: AlacPoint::new(Line(-2), Column(3)),
            end: AlacPoint::new(Line(4), Column(8)),
            is_block: true,
        };

        let terminal_range = terminal_selection_range_from_alacritty(alacritty_range);
        assert_eq!(
            terminal_range,
            SelectionRange {
                start: Point {
                    line: -2,
                    column: 3
                },
                end: Point { line: 4, column: 8 },
                is_block: true,
            }
        );
    }
}
