//! THROWAWAY SPIKE — ghostty-vt-backed terminal path (wayfinder ticket
//! xipeng-jin/zed#34). Gated behind `ZED_GHOSTTY_SPIKE=1`. Unix-only, never
//! meant to merge; exists to measure seam friction between the vendored
//! `ghostty_vt` core and Zed's `Content`/`IndexedCell`/`terminal_element`
//! rendering path.
//!
//! Architecture (crude version of docs/ghostty-migration/pty-threading-architecture.md):
//! - PTY spawned via `forkpty`; a reader `std::thread` pumps raw bytes over an
//!   unbounded channel.
//! - The `!Send` ghostty `Terminal` + `RenderState` live inside a GPUI
//!   foreground task (spawned in `TerminalBuilder::subscribe`), NOT inside the
//!   `Terminal` entity: `TerminalBuilder::new` runs on a background thread, so
//!   a `!Send` field in `Terminal` would not compile.
//! - Each batch of bytes is fed to `vt_write`, then a full `Content` snapshot
//!   is built from `RenderState` and pushed into `Terminal::last_content`.

use collections::HashMap;
use std::{
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use alacritty_terminal::term::cell::{Cell as AlacCell, Flags as AlacFlags};
use anyhow::{Context as _, Result, anyhow};
use futures::{
    StreamExt as _,
    channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded},
};
use ghostty_vt::{
    Terminal as GhosttyTerminal, TerminalOptions,
    render::{CellIterator, CursorVisualStyle, Dirty, RenderState, RowIterator},
    screen::{CellWide, Screen},
    style::Underline,
    terminal::{Mode, ScrollViewport},
};
use gpui::{Context, Task};

use crate::{
    Cell, Color, Content, Cursor, CursorShape, Event, IndexedCell, Modes, NamedColor, Point, Rgb,
    Scroll, Terminal, TerminalBounds,
};

pub(crate) fn enabled() -> bool {
    std::env::var("ZED_GHOSTTY_SPIKE").is_ok_and(|value| value == "1")
}

/// Log alacritty `make_content` timings for comparison (set `ZED_TERM_PERF=1`).
pub(crate) fn perf_logging_enabled() -> bool {
    std::env::var("ZED_TERM_PERF").is_ok_and(|value| value == "1")
}

pub(crate) enum SpikeMsg {
    Bytes(Vec<u8>),
    Eof,
}

pub(crate) enum SpikeCmd {
    Resize(TerminalBounds),
    Scroll(Scroll),
}

#[derive(Clone)]
pub(crate) struct SpikePty {
    master: Arc<OwnedFd>,
    child: libc::pid_t,
}

fn winsize_for(bounds: TerminalBounds) -> libc::winsize {
    let cols = bounds.num_columns().max(2) as u16;
    let rows = bounds.num_lines().max(2) as u16;
    libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: (f32::from(bounds.cell_width) * cols as f32) as u16,
        ws_ypixel: (f32::from(bounds.line_height) * rows as f32) as u16,
    }
}

fn write_all_fd(fd: &OwnedFd, data: &[u8]) {
    let mut remaining = data;
    while !remaining.is_empty() {
        let written = unsafe {
            libc::write(
                fd.as_raw_fd(),
                remaining.as_ptr().cast(),
                remaining.len(),
            )
        };
        if written > 0 {
            remaining = &remaining[written as usize..];
        } else {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            log::error!("ghostty spike: pty write failed: {err}");
            break;
        }
    }
}

impl SpikePty {
    pub(crate) fn spawn(
        shell: Option<(String, Vec<String>)>,
        env: &HashMap<String, String>,
        working_directory: Option<&Path>,
    ) -> Result<(Self, UnboundedReceiver<SpikeMsg>)> {
        let winsize = libc::winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let mut master: libc::c_int = -1;
        let pid = unsafe {
            libc::forkpty(
                &mut master,
                std::ptr::null_mut(),
                std::ptr::null(),
                &winsize,
            )
        };
        if pid < 0 {
            return Err(anyhow!(
                "forkpty failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        if pid == 0 {
            // Child: exec the shell. Same crude approach as ghostty's
            // ghostling example; alacritty's tty module does the equivalent.
            let (program, args) = shell.unwrap_or_else(|| {
                (
                    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()),
                    Vec::new(),
                )
            });
            let mut command = std::process::Command::new(&program);
            command
                .args(&args)
                .envs(env)
                .env("TERM", "xterm-256color")
                .env("COLORTERM", "truecolor");
            if let Some(cwd) = working_directory {
                command.current_dir(cwd);
            }
            use std::os::unix::process::CommandExt as _;
            let _ = command.exec();
            std::process::exit(127);
        }

        let master = unsafe { OwnedFd::from_raw_fd(master) };
        let (bytes_tx, bytes_rx) = unbounded();
        let reader_fd = master.try_clone().context("dup pty master")?;
        std::thread::Builder::new()
            .name("ghostty-spike-pty-reader".to_string())
            .spawn(move || reader_loop(reader_fd, bytes_tx))
            .context("spawn pty reader thread")?;

        Ok((
            Self {
                master: Arc::new(master),
                child: pid,
            },
            bytes_rx,
        ))
    }

    pub(crate) fn write(&self, data: &[u8]) {
        write_all_fd(&self.master, data);
    }

    pub(crate) fn resize(&self, bounds: TerminalBounds) {
        let winsize = winsize_for(bounds);
        let result = unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &winsize) };
        if result != 0 {
            log::error!(
                "ghostty spike: TIOCSWINSZ failed: {}",
                std::io::Error::last_os_error()
            );
        }
    }

    pub(crate) fn kill(&self) {
        unsafe {
            libc::kill(self.child, libc::SIGHUP);
        }
        let child = self.child;
        // Reap without blocking the caller.
        if let Err(error) = std::thread::Builder::new()
            .name("ghostty-spike-reaper".to_string())
            .spawn(move || {
                let mut status = 0;
                unsafe { libc::waitpid(child, &mut status, 0) };
            })
        {
            log::error!("ghostty spike: failed to spawn reaper thread: {error}");
        }
    }
}

fn reader_loop(fd: OwnedFd, tx: UnboundedSender<SpikeMsg>) {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let count = unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        if count > 0 {
            if tx
                .unbounded_send(SpikeMsg::Bytes(buf[..count as usize].to_vec()))
                .is_err()
            {
                break;
            }
        } else if count == 0 {
            let _ = tx.unbounded_send(SpikeMsg::Eof);
            break;
        } else {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            // On Linux the slave side closing usually surfaces as EIO.
            let _ = tx.unbounded_send(SpikeMsg::Eof);
            break;
        }
    }
}

fn gerr(error: ghostty_vt::error::Error) -> anyhow::Error {
    anyhow!("ghostty_vt error: {error:?}")
}

struct PerfStats {
    rebuilds: u64,
    bytes: u64,
    vt_write_total: Duration,
    vt_write_max: Duration,
    snapshot_total: Duration,
    snapshot_max: Duration,
    build_total: Duration,
    build_max: Duration,
    cells_last: usize,
    dirty_clean: u64,
    dirty_partial: u64,
    dirty_full: u64,
    last_log: Instant,
}

impl PerfStats {
    fn new() -> Self {
        Self {
            rebuilds: 0,
            bytes: 0,
            vt_write_total: Duration::ZERO,
            vt_write_max: Duration::ZERO,
            snapshot_total: Duration::ZERO,
            snapshot_max: Duration::ZERO,
            build_total: Duration::ZERO,
            build_max: Duration::ZERO,
            cells_last: 0,
            dirty_clean: 0,
            dirty_partial: 0,
            dirty_full: 0,
            last_log: Instant::now(),
        }
    }

    fn maybe_log(&mut self) {
        if self.last_log.elapsed() < Duration::from_secs(5) || self.rebuilds == 0 {
            return;
        }
        let rebuilds = self.rebuilds.max(1) as u32;
        log::info!(
            "ghostty spike perf: rebuilds={} bytes={} cells={} \
             vt_write avg={:?} max={:?} | snapshot avg={:?} max={:?} | \
             content-build avg={:?} max={:?} | dirty clean/partial/full={}/{}/{}",
            self.rebuilds,
            self.bytes,
            self.cells_last,
            self.vt_write_total / rebuilds,
            self.vt_write_max,
            self.snapshot_total / rebuilds,
            self.snapshot_max,
            self.build_total / rebuilds,
            self.build_max,
            self.dirty_clean,
            self.dirty_partial,
            self.dirty_full,
        );
        *self = Self::new();
    }
}

/// Owns the `!Send` ghostty terminal state on the GPUI foreground thread and
/// drives it from the PTY byte channel plus entity-side commands.
pub(crate) fn spawn_driver(
    pty: SpikePty,
    mut msg_rx: UnboundedReceiver<SpikeMsg>,
    mut cmd_rx: UnboundedReceiver<SpikeCmd>,
    cx: &Context<Terminal>,
) -> Task<Result<()>> {
    cx.spawn(async move |terminal, cx| {
        let mut vt = GhosttyTerminal::new(TerminalOptions {
            cols: 80,
            rows: 24,
            max_scrollback: 10_000,
        })
        .map_err(gerr)?;
        vt.on_pty_write({
            let master = pty.master.clone();
            move |_term, data| write_all_fd(&master, data)
        })
        .map_err(gerr)?;

        let mut snapshotter = Snapshotter {
            render_state: RenderState::new().map_err(gerr)?,
            row_iterator: RowIterator::new().map_err(gerr)?,
            cell_iterator: CellIterator::new().map_err(gerr)?,
        };
        let mut stats = PerfStats::new();
        let mut exited = false;

        loop {
            let mut pending_bytes: Vec<Vec<u8>> = Vec::new();
            let mut pending_cmds: Vec<SpikeCmd> = Vec::new();

            futures::select_biased! {
                cmd = cmd_rx.next() => match cmd {
                    Some(cmd) => pending_cmds.push(cmd),
                    None => break,
                },
                msg = msg_rx.next() => match msg {
                    Some(SpikeMsg::Bytes(bytes)) => pending_bytes.push(bytes),
                    Some(SpikeMsg::Eof) | None => exited = true,
                },
            }

            // Coalesce whatever else is already queued (bounded so a
            // firehose like `yes` still yields to rendering).
            let mut batched = pending_bytes.iter().map(|b| b.len()).sum::<usize>();
            while batched < 2 * 1024 * 1024 {
                match msg_rx.try_recv() {
                    Ok(SpikeMsg::Bytes(bytes)) => {
                        batched += bytes.len();
                        pending_bytes.push(bytes);
                    }
                    Ok(SpikeMsg::Eof) => {
                        exited = true;
                        break;
                    }
                    Err(_) => break,
                }
            }
            while let Ok(cmd) = cmd_rx.try_recv() {
                pending_cmds.push(cmd);
            }

            for cmd in pending_cmds {
                match cmd {
                    SpikeCmd::Resize(bounds) => {
                        let winsize = winsize_for(bounds);
                        vt.resize(
                            winsize.ws_col,
                            winsize.ws_row,
                            (f32::from(bounds.cell_width) as u32).max(1),
                            (f32::from(bounds.line_height) as u32).max(1),
                        )
                        .map_err(gerr)?;
                    }
                    SpikeCmd::Scroll(scroll) => {
                        let rows = vt.rows().map_err(gerr)? as isize;
                        let scroll = match scroll {
                            Scroll::Delta(delta) => ScrollViewport::Delta(-(delta as isize)),
                            Scroll::PageUp => ScrollViewport::Delta(-rows),
                            Scroll::PageDown => ScrollViewport::Delta(rows),
                            Scroll::Top => ScrollViewport::Top,
                            Scroll::Bottom => ScrollViewport::Bottom,
                        };
                        vt.scroll_viewport(scroll);
                    }
                }
            }

            if !pending_bytes.is_empty() {
                let started = Instant::now();
                for bytes in &pending_bytes {
                    stats.bytes += bytes.len() as u64;
                    vt.vt_write(bytes);
                }
                let elapsed = started.elapsed();
                stats.vt_write_total += elapsed;
                stats.vt_write_max = stats.vt_write_max.max(elapsed);
            }

            let content = snapshotter.build_content(&vt, &mut stats)?;
            stats.rebuilds += 1;
            stats.maybe_log();

            terminal.update(cx, |terminal, cx| {
                let mut content = content;
                content.terminal_bounds = terminal.last_content.terminal_bounds;
                content.last_hovered_word = terminal.last_content.last_hovered_word.clone();
                terminal.last_content = content;
                if exited {
                    cx.emit(Event::CloseTerminal);
                }
                cx.emit(Event::Wakeup);
                cx.notify();
            })?;

            if exited {
                break;
            }
        }
        anyhow::Ok(())
    })
}

struct Snapshotter {
    render_state: RenderState<'static>,
    row_iterator: RowIterator<'static>,
    cell_iterator: CellIterator<'static>,
}

impl Snapshotter {
    /// Build a full Zed `Content` snapshot from the ghostty render state.
    ///
    /// This is the seam under test: ghostty rows/cells/styles flattened into
    /// the alacritty-shaped `IndexedCell` grid that `terminal_element`
    /// consumes today.
    fn build_content(&mut self, vt: &GhosttyTerminal<'static, '_>, stats: &mut PerfStats) -> Result<Content> {
        let scrollbar = vt.scrollbar().map_err(gerr)?;
        let display_offset =
            scrollbar.total.saturating_sub(scrollbar.offset + scrollbar.len) as usize;

        let started = Instant::now();
        let snapshot = self.render_state.update(vt).map_err(gerr)?;
        let snapshot_elapsed = started.elapsed();
        stats.snapshot_total += snapshot_elapsed;
        stats.snapshot_max = stats.snapshot_max.max(snapshot_elapsed);

        match snapshot.dirty().map_err(gerr)? {
            Dirty::Clean => stats.dirty_clean += 1,
            Dirty::Partial => stats.dirty_partial += 1,
            Dirty::Full => stats.dirty_full += 1,
        }

        let started = Instant::now();
        let rows = snapshot.rows().map_err(gerr)? as usize;
        let cols = snapshot.cols().map_err(gerr)? as usize;

        let cursor_visible = snapshot.cursor_visible().map_err(gerr)?;
        let cursor_viewport = snapshot.cursor_viewport().map_err(gerr)?;
        let cursor_shape = if cursor_visible {
            match snapshot.cursor_visual_style().map_err(gerr)? {
                CursorVisualStyle::Bar => CursorShape::Bar,
                CursorVisualStyle::Underline => CursorShape::Underline,
                CursorVisualStyle::BlockHollow => CursorShape::HollowBlock,
                _ => CursorShape::Block,
            }
        } else {
            CursorShape::Hidden
        };
        let (cursor_x, cursor_y) = cursor_viewport
            .as_ref()
            .map_or((0, 0), |vp| (vp.x as usize, vp.y as usize));

        let mut cells = Vec::with_capacity(rows * cols);
        let mut cursor_char = ' ';
        let mut bottom_row_occupied = false;
        let mut grapheme_buffer: Vec<char> = Vec::with_capacity(4);

        let mut row_iteration = self.row_iterator.update(&snapshot).map_err(gerr)?;
        let mut row_index = 0usize;
        while let Some(row) = row_iteration.next() {
            let grid_line = row_index as i32 - display_offset as i32;
            let mut cell_iteration = self.cell_iterator.update(row).map_err(gerr)?;
            let mut column = 0usize;
            while let Some(cell) = cell_iteration.next() {
                let mut alacritty_cell = AlacCell::default();

                let raw = cell.raw_cell().map_err(gerr)?;
                match raw.wide().map_err(gerr)? {
                    CellWide::Narrow => {}
                    CellWide::Wide => alacritty_cell.flags.insert(AlacFlags::WIDE_CHAR),
                    CellWide::SpacerTail => {
                        alacritty_cell.flags.insert(AlacFlags::WIDE_CHAR_SPACER)
                    }
                    CellWide::SpacerHead => {
                        alacritty_cell
                            .flags
                            .insert(AlacFlags::LEADING_WIDE_CHAR_SPACER)
                    }
                }

                let grapheme_count = cell.graphemes_len().map_err(gerr)?;
                if grapheme_count > 0
                    && !alacritty_cell.flags.intersects(
                        AlacFlags::WIDE_CHAR_SPACER | AlacFlags::LEADING_WIDE_CHAR_SPACER,
                    )
                {
                    grapheme_buffer.clear();
                    grapheme_buffer.resize(grapheme_count, '\0');
                    cell.graphemes_buf(&mut grapheme_buffer).map_err(gerr)?;
                    alacritty_cell.c = grapheme_buffer.first().copied().unwrap_or(' ');
                    for combining in grapheme_buffer.iter().skip(1) {
                        alacritty_cell.push_zerowidth(*combining);
                    }
                }

                // Flattened-RGB color bridge: ghostty resolves palette and
                // default colors to concrete RGB; only "no explicit color"
                // maps back onto the theme-aware Named defaults. Palette
                // themes therefore bypass Zed's theme mapping — a known
                // spike shortcut, resolved for real by the color-contract
                // decision (ticket #31).
                alacritty_cell.fg = cell
                    .fg_color()
                    .map_err(gerr)?
                    .map_or(Color::Named(NamedColor::Foreground), |color| {
                        Color::Spec(Rgb {
                            r: color.r,
                            g: color.g,
                            b: color.b,
                        })
                    });
                alacritty_cell.bg = cell
                    .bg_color()
                    .map_err(gerr)?
                    .map_or(Color::Named(NamedColor::Background), |color| {
                        Color::Spec(Rgb {
                            r: color.r,
                            g: color.g,
                            b: color.b,
                        })
                    });

                if cell.has_styling().map_err(gerr)? {
                    let style = cell.style().map_err(gerr)?;
                    let flags = &mut alacritty_cell.flags;
                    flags.set(AlacFlags::BOLD, style.bold);
                    flags.set(AlacFlags::ITALIC, style.italic);
                    flags.set(AlacFlags::INVERSE, style.inverse);
                    flags.set(AlacFlags::DIM, style.faint);
                    flags.set(AlacFlags::STRIKEOUT, style.strikethrough);
                    flags.set(AlacFlags::HIDDEN, style.invisible);
                    match style.underline {
                        Underline::Single => flags.insert(AlacFlags::UNDERLINE),
                        Underline::Double => flags.insert(AlacFlags::DOUBLE_UNDERLINE),
                        Underline::Curly => flags.insert(AlacFlags::UNDERCURL),
                        Underline::Dotted => flags.insert(AlacFlags::DOTTED_UNDERLINE),
                        Underline::Dashed => flags.insert(AlacFlags::DASHED_UNDERLINE),
                        _ => {}
                    }
                }

                if row_index == cursor_y && column == cursor_x {
                    cursor_char = alacritty_cell.c;
                }
                if row_index == rows.saturating_sub(1) && alacritty_cell.c != ' ' {
                    bottom_row_occupied = true;
                }

                cells.push(IndexedCell {
                    point: Point::new(grid_line, column),
                    cell: Cell {
                        cell: alacritty_cell,
                    },
                });
                column += 1;
            }
            row.set_dirty(false).map_err(gerr)?;
            row_index += 1;
        }
        snapshot.set_dirty(Dirty::Clean).map_err(gerr)?;

        let build_elapsed = started.elapsed();
        stats.build_total += build_elapsed;
        stats.build_max = stats.build_max.max(build_elapsed);
        stats.cells_last = cells.len();

        let cursor_grid_line = cursor_y as i32 - display_offset as i32;
        if cursor_grid_line >= rows as i32 - 1 - display_offset as i32 {
            bottom_row_occupied = true;
        }

        Ok(Content {
            cells,
            mode: modes_from_ghostty(vt),
            display_offset,
            selection_text: None,
            selection: None,
            cursor: Cursor {
                shape: cursor_shape,
                point: Point::new(cursor_grid_line, cursor_x),
            },
            cursor_char,
            terminal_bounds: TerminalBounds::default(),
            last_hovered_word: None,
            scrolled_to_top: scrollbar.offset == 0,
            scrolled_to_bottom: display_offset == 0,
            bottom_row_occupied,
        })
    }
}

fn modes_from_ghostty(vt: &GhosttyTerminal<'static, '_>) -> Modes {
    let mut modes = Modes::empty();
    let mut apply = |mode: Mode, flag: Modes| {
        if vt.mode(mode).unwrap_or(false) {
            modes.insert(flag);
        }
    };
    apply(Mode::DECCKM, Modes::APP_CURSOR);
    apply(Mode::KEYPAD_KEYS, Modes::APP_KEYPAD);
    apply(Mode::CURSOR_VISIBLE, Modes::SHOW_CURSOR);
    apply(Mode::WRAPAROUND, Modes::LINE_WRAP);
    apply(Mode::ORIGIN, Modes::ORIGIN);
    apply(Mode::INSERT, Modes::INSERT);
    apply(Mode::LINEFEED, Modes::LINE_FEED_NEW_LINE);
    apply(Mode::FOCUS_EVENT, Modes::FOCUS_IN_OUT);
    apply(Mode::ALT_SCROLL, Modes::ALTERNATE_SCROLL);
    apply(Mode::BRACKETED_PASTE, Modes::BRACKETED_PASTE);
    apply(Mode::SGR_MOUSE, Modes::SGR_MOUSE);
    apply(Mode::UTF8_MOUSE, Modes::UTF8_MOUSE);
    apply(Mode::NORMAL_MOUSE, Modes::MOUSE_REPORT_CLICK);
    apply(Mode::BUTTON_MOUSE, Modes::MOUSE_DRAG);
    apply(Mode::ANY_MOUSE, Modes::MOUSE_MOTION);
    if matches!(vt.active_screen(), Ok(Screen::Alternate)) {
        modes.insert(Modes::ALT_SCREEN);
    }
    modes
}
