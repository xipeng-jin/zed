//! The shared PTY seam (SPEC.md §3 D1/D2, §4.1 S7): portable-pty behind a thin
//! Zed-owned module, used identically by both terminal backends. A reader
//! `std::thread` moves raw byte batches over a bounded channel to the
//! foreground pump in `terminal.rs`; a writer `std::thread` drains an input
//! channel; the reader reaps the child on EOF — no event loop, no SIGCHLD
//! machinery. portable-pty's `pre_exec` clears the child's signal mask,
//! subsuming the `SignalMask` fork fix (zed#42234).

use std::{
    borrow::Cow,
    io::{Read, Write},
    path::PathBuf,
    process::ExitStatus,
    sync::Arc,
    thread::JoinHandle,
};

use anyhow::{Context as _, Result};
use collections::HashMap;
use gpui::BackgroundExecutor;
#[cfg(windows)]
use gpui::Task;
use parking_lot::Mutex;
use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::{TerminalBackendEvent, TerminalBounds, pty_info::ProcessIdGetter};

/// One `read()` batch — ghostty's own number, bounding both gather latency and
/// per-turn parse work (SPEC.md §3 D2).
pub(super) const READ_BATCH_SIZE: usize = 64 * 1024;

/// Depth of the bounded output channel. When the foreground falls behind, the
/// reader blocks, the kernel PTY queue fills, and the child's writes stall —
/// the same flow control ghostty documents for its 4-buffer pipeline.
pub(super) const OUTPUT_CHANNEL_BATCHES: usize = 4;

/// Batches the foreground pump ingests per turn before yielding, keeping
/// worst-case per-turn parse work at `MAX_BATCHES_PER_TURN × READ_BATCH_SIZE`
/// — the same bound as alacritty's `MAX_LOCKED_READ` hold that the foreground
/// already waited out via the FairMutex.
pub(super) const MAX_BATCHES_PER_TURN: usize = 4;

/// Cadence for the Windows child-exit poll (`try_wait`; SPEC.md §8.2). ConPTY
/// delivers reader EOF only once the pseudoconsole is dropped, so exit must be
/// observed independently of the reader. §8.2 words this "on the `pty_info`
/// cadence" — a fixed timer of the same order is used instead of piggybacking
/// `pty_info`'s refresh, because that refresh is wakeup-driven and a child
/// exiting silently produces no wakeups; still no new thread.
#[cfg(windows)]
const EXIT_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// What flows from the PTY reader thread (and the headless subprocess pumps)
/// to the foreground pump. Exit-sequence events travel on this channel — not a
/// side channel — so they cannot overtake still-queued output bytes.
pub(super) enum PtyOutput {
    Bytes(Vec<u8>),
    Event(TerminalBackendEvent),
}

pub(super) fn output_channel() -> (
    async_channel::Sender<PtyOutput>,
    async_channel::Receiver<PtyOutput>,
) {
    async_channel::bounded(OUTPUT_CHANNEL_BATCHES)
}

pub(super) struct PtyOptions {
    pub shell: Option<(String, Vec<String>)>,
    pub working_directory: Option<PathBuf>,
    pub env: HashMap<String, String>,
    /// Exposed to the child as `WINDOWID` (and `ALACRITTY_WINDOW_ID`, kept for
    /// byte-parity with the alacritty tty layer until P10 adjudicates it).
    /// Unix-only, exactly like the alacritty tty layer.
    #[cfg_attr(windows, allow(dead_code))]
    pub window_id: u64,
}

type SharedChild = Arc<Mutex<Option<Box<dyn Child + Send + Sync>>>>;

pub(super) struct SpawnedPty {
    pub handle: PtyHandle,
    pub process_id_getter: ProcessIdGetter,
    /// Both threads exit on their own once the PTY closes; production drops
    /// (detaches) these. Only tests read them, via `is_finished`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub reader_thread: JoinHandle<()>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub writer_thread: JoinHandle<()>,
}

/// The foreground's handle to a live PTY: owns the master (closing it is what
/// delivers reader EOF on Windows), the writer-thread input channel, and a
/// killer for the child.
pub(super) struct PtyHandle {
    /// `Some` until drop takes it (see the `Drop` impl).
    master: Option<Box<dyn MasterPty + Send>>,
    input_tx: async_channel::Sender<Cow<'static, [u8]>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    #[cfg(windows)]
    _exit_poller: Task<()>,
}

impl PtyHandle {
    /// Queues bytes for the writer thread. Never blocks: the input channel is
    /// unbounded, so a stalled child blocks the writer thread, not the UI.
    pub(super) fn notify(&self, input: impl Into<Cow<'static, [u8]>>) {
        if self.input_tx.send_blocking(input.into()).is_err() {
            log::debug!("dropping write to a shut-down terminal PTY");
        }
    }

    /// Resizes the PTY directly (TIOCSWINSZ / ConPTY resize) — cheap, and
    /// ordered before the emulator-side resize exactly like ghostty places its
    /// ioctl outside the terminal update.
    pub(super) fn resize(&self, bounds: TerminalBounds) {
        let Some(master) = &self.master else { return };
        if let Err(error) = master.resize(pty_size_from_bounds(bounds)) {
            log::error!("failed to resize terminal PTY: {error:#}");
        }
    }

    #[cfg(test)]
    pub(super) fn master(&self) -> Option<&(dyn MasterPty + Send)> {
        self.master.as_deref()
    }

    /// Stops the writer thread and signals the child (SIGHUP on unix — the
    /// same signal the alacritty `Pty` drop sent; `TerminateProcess` on
    /// Windows, via the wezterm#7709 fix). The reader thread drains remaining
    /// output, reaps, and exits on its own.
    pub(super) fn shutdown(&self) {
        self.input_tx.close();
        if let Err(error) = self.killer.lock().kill() {
            log::debug!("failed to signal terminal child on shutdown: {error}");
        }
    }
}

impl Drop for PtyHandle {
    fn drop(&mut self) {
        let Some(master) = self.master.take() else {
            return;
        };
        // On Windows, dropping the master runs ClosePseudoConsole, which can
        // block until pending output is drained — while the reader thread may
        // itself be blocked sending to the bounded channel that only the
        // dropping thread's side consumes. Never let that pair deadlock the
        // dropping thread (the UI foreground in production): close on a
        // detached thread; the reader drains and exits in parallel. On unix
        // the master drop is a plain close(2).
        #[cfg(windows)]
        {
            if let Err(error) = std::thread::Builder::new()
                .name("terminal-pty-closer".to_string())
                .spawn(move || drop(master))
            {
                // The closure — and with it the master — was dropped inline.
                log::error!("failed to spawn pty closer thread: {error}");
            }
        }
        #[cfg(not(windows))]
        drop(master);
    }
}

/// Opens a PTY, spawns the shell into it, and starts the reader/writer
/// threads. Output batches (and the final exit-event sequence) arrive on
/// `output_tx`; input flows through [`PtyHandle::notify`].
#[cfg_attr(not(windows), allow(unused_variables))]
pub(super) fn spawn_pty(
    options: PtyOptions,
    bounds: TerminalBounds,
    output_tx: async_channel::Sender<PtyOutput>,
    executor: &BackgroundExecutor,
) -> Result<SpawnedPty> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(pty_size_from_bounds(bounds))
        .context("failed to open pty")?;

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    set_iutf8(pair.master.as_ref());

    let command = command_builder(options);
    let child = pair
        .slave
        .spawn_command(command)
        .context("failed to spawn terminal child")?;
    // The parent must not keep slave fds open, or the master would never see
    // EOF after the child exits.
    drop(pair.slave);

    let killer = child.clone_killer();
    let process_id_getter = process_id_getter(pair.master.as_ref(), child.as_ref());

    let reader = pair
        .master
        .try_clone_reader()
        .context("failed to clone pty reader")?;
    let writer = pair
        .master
        .take_writer()
        .context("failed to take pty writer")?;

    let child: SharedChild = Arc::new(Mutex::new(Some(child)));
    let (input_tx, input_rx) = async_channel::unbounded();

    #[cfg(windows)]
    let exit_poller = spawn_exit_poller(executor, child.clone(), output_tx.clone());

    let reader_thread = spawn_reader_thread(reader, child, output_tx)
        .context("failed to spawn pty reader thread")?;
    let writer_thread =
        spawn_writer_thread(writer, input_rx).context("failed to spawn pty writer thread")?;

    Ok(SpawnedPty {
        handle: PtyHandle {
            master: Some(pair.master),
            input_tx,
            killer: Mutex::new(killer),
            #[cfg(windows)]
            _exit_poller: exit_poller,
        },
        process_id_getter,
        reader_thread,
        writer_thread,
    })
}

fn pty_size_from_bounds(bounds: TerminalBounds) -> PtySize {
    let rows = bounds.num_lines() as u16;
    let cols = bounds.num_columns() as u16;
    // Winsize pixel fields carry the total text-area size, matching
    // alacritty's `to_winsize` (cols × cell_width, rows × cell_height).
    PtySize {
        rows,
        cols,
        pixel_width: cols.saturating_mul(f32::from(bounds.cell_width()) as u16),
        pixel_height: rows.saturating_mul(f32::from(bounds.line_height()) as u16),
    }
}

fn command_builder(options: PtyOptions) -> CommandBuilder {
    let mut command = match options.shell {
        Some((program, args)) => {
            let mut command = CommandBuilder::new(program);
            command.args(args);
            command
        }
        None => default_shell_command(),
    };

    #[cfg(unix)]
    {
        let window_id = options.window_id.to_string();
        command.env("ALACRITTY_WINDOW_ID", &window_id);
        // Window ID for clients relying on X11 hacks.
        command.env("WINDOWID", window_id);
    }

    for (key, value) in options.env {
        command.env(key, value);
    }

    // Prevent child processes from inheriting linux-specific startup
    // notification env.
    command.env_remove("XDG_ACTIVATION_TOKEN");
    command.env_remove("DESKTOP_STARTUP_ID");

    if let Some(working_directory) = options.working_directory {
        command.cwd(working_directory);
    }

    command
}

/// The no-explicit-shell command, matching the alacritty tty layer: resolve
/// `$SHELL` (passwd fallback), plain invocation on Linux, `/usr/bin/login`
/// wrapper on macOS. The macOS wrapper is re-derived from ghostty's exec layer
/// at the macOS gate (SPEC.md §8.1); until then this preserves today's
/// behavior. On Windows Zed always passes an explicit shell.
fn default_shell_command() -> CommandBuilder {
    #[cfg(unix)]
    {
        let shell = CommandBuilder::new_default_prog().get_shell();

        #[cfg(target_os = "macos")]
        if let Ok(user) = std::env::var("USER") {
            let shell_name = shell.rsplit('/').next().unwrap_or(&shell);

            // Use the `login` command so the shell will appear as a tty
            // session; exec the shell with argv[0] prepended by '-' so it
            // becomes a login shell (`-l` disables `login` doing it itself).
            let exec = format!("exec -a -{} {}", shell_name, shell);

            // With -l, `login` won't chdir to $HOME, so it also misses a
            // ~/.hushlogin; check ourselves and pass -q when one exists.
            let has_home_hushlogin = std::env::var("HOME")
                .is_ok_and(|home| std::path::Path::new(&home).join(".hushlogin").exists());

            // -f: no authentication (already logged in)
            // -l: skip changing directory to $HOME and argv[0] mangling
            // -p: preserve the environment
            // zsh over sh because of `exec -a`.
            let flags = if has_home_hushlogin { "-qflp" } else { "-flp" };
            let mut command = CommandBuilder::new("/usr/bin/login");
            command.args([flags, user.as_str(), "/bin/zsh", "-fc", exec.as_str()]);
            return command;
        }

        // Deliberately not `new_default_prog()`: portable-pty spawns that as a
        // login shell (argv[0] = "-shell"), which alacritty does not do here.
        CommandBuilder::new(shell)
    }
    #[cfg(windows)]
    {
        CommandBuilder::new_default_prog()
    }
}

/// Parity with alacritty's tty setup: mark the PTY as UTF-8 so canonical-mode
/// line editing erases multibyte characters correctly.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn set_iutf8(master: &dyn MasterPty) {
    let Some(fd) = master.as_raw_fd() else {
        return;
    };
    unsafe {
        let mut termios = std::mem::zeroed::<libc::termios>();
        if libc::tcgetattr(fd, &mut termios) != 0 {
            log::debug!("failed to read pty termios for IUTF8 setup");
            return;
        }
        termios.c_iflag |= libc::IUTF8;
        if libc::tcsetattr(fd, libc::TCSANOW, &termios) != 0 {
            log::debug!("failed to set IUTF8 on the pty");
        }
    }
}

#[cfg(unix)]
fn process_id_getter(master: &dyn MasterPty, child: &(dyn Child + Send + Sync)) -> ProcessIdGetter {
    // The master fd feeds `tcgetpgrp` for foreground-process discovery; the
    // child pid is the fallback, exactly as with the alacritty pty.
    ProcessIdGetter::new(
        master.as_raw_fd().unwrap_or(-1),
        child.process_id().unwrap_or(0),
    )
}

#[cfg(windows)]
fn process_id_getter(
    _master: &dyn MasterPty,
    child: &(dyn Child + Send + Sync),
) -> ProcessIdGetter {
    let handle = child
        .as_raw_handle()
        .map(|handle| handle as i32)
        .unwrap_or(0);
    ProcessIdGetter::new(handle, child.process_id().unwrap_or(0))
}

/// Reads the PTY in [`READ_BATCH_SIZE`] batches until EOF, then reaps the
/// child and reports the same event sequence the alacritty `EventLoop`
/// produced on child exit: `ChildExit(status)` → `Exit` → `Wakeup`. A blocking
/// reader drains every byte before reporting exit by construction, subsuming
/// alacritty's `drain_on_exit`.
fn spawn_reader_thread(
    mut reader: Box<dyn Read + Send>,
    child: SharedChild,
    output_tx: async_channel::Sender<PtyOutput>,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("terminal-pty-reader".to_string())
        .spawn(move || {
            let mut buffer = vec![0u8; READ_BATCH_SIZE];
            let mut receiver_alive = true;
            let mut reads = 0u32;
            loop {
                let read_result = reader.read(&mut buffer);
                if cfg!(test) && reads < 4 {
                    reads += 1;
                    eprintln!(
                        "[pty-reader] read #{reads}: {:?}",
                        read_result.as_ref().map(|count| *count)
                    );
                }
                match read_result {
                    // portable-pty maps the Linux slave-hangup EIO to a clean
                    // EOF already.
                    Ok(0) => break,
                    Ok(count) => {
                        if receiver_alive
                            && output_tx
                                .send_blocking(PtyOutput::Bytes(buffer[..count].to_vec()))
                                .is_err()
                        {
                            // The terminal is gone. Keep draining so a child
                            // blocked on a full kernel queue can make progress
                            // toward exit, and so it still gets reaped below.
                            receiver_alive = false;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(error) => {
                        log::debug!("terminal pty read failed: {error}");
                        break;
                    }
                }
            }

            // `None` means the Windows exit poller observed and reported the
            // exit first; nothing left to do.
            let status = reap_child(&child);
            if cfg!(test) {
                eprintln!("[pty-reader] eof; reaped: {status:?}");
            }
            if let Some(status) = status
                && receiver_alive
            {
                for event in exit_event_sequence(status) {
                    if output_tx.send_blocking(PtyOutput::Event(event)).is_err() {
                        break;
                    }
                }
            }
        })
}

fn spawn_writer_thread(
    mut writer: Box<dyn Write + Send>,
    input_rx: async_channel::Receiver<Cow<'static, [u8]>>,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("terminal-pty-writer".to_string())
        .spawn(move || {
            while let Ok(input) = input_rx.recv_blocking() {
                if let Err(error) = writer.write_all(&input).and_then(|()| writer.flush()) {
                    log::debug!("terminal pty write failed: {error}");
                    break;
                }
            }
        })
}

/// Mirrors the event order of the alacritty `EventLoop` on child exit.
fn exit_event_sequence(status: ExitStatus) -> [TerminalBackendEvent; 3] {
    [
        TerminalBackendEvent::ChildExit(status),
        TerminalBackendEvent::Exit,
        TerminalBackendEvent::Wakeup,
    ]
}

/// Takes the child out of the shared slot and waits for it. Returns `None`
/// when someone else already reaped it.
fn reap_child(child: &SharedChild) -> Option<ExitStatus> {
    let mut boxed = child.lock().take()?;
    Some(wait_for_exit_status(boxed.as_mut()))
}

fn wait_for_exit_status(child: &mut (dyn Child + Send + Sync)) -> ExitStatus {
    // On unix the portable-pty child is a `std::process::Child`; waiting on it
    // directly preserves the raw wait status (exit code *and* signal), which
    // `register_task_finished` reports to the user.
    #[cfg(unix)]
    {
        let child: &mut dyn Child = child;
        if let Some(std_child) = child.downcast_mut::<std::process::Child>() {
            return match std_child.wait() {
                Ok(status) => status,
                Err(error) => {
                    log::warn!("failed to wait for terminal child: {error}");
                    ExitStatus::default()
                }
            };
        }
    }

    match child.wait() {
        Ok(status) => std_exit_status(status),
        Err(error) => {
            log::warn!("failed to wait for terminal child: {error}");
            ExitStatus::default()
        }
    }
}

#[cfg(unix)]
fn std_exit_status(status: portable_pty::ExitStatus) -> ExitStatus {
    use std::os::unix::process::ExitStatusExt as _;
    ExitStatus::from_raw((status.exit_code() as i32) << 8)
}

#[cfg(windows)]
fn std_exit_status(status: portable_pty::ExitStatus) -> ExitStatus {
    use std::os::windows::process::ExitStatusExt as _;
    ExitStatus::from_raw(status.exit_code())
}

/// Windows-only child-exit watcher: ConPTY delivers reader EOF only after the
/// pseudoconsole is dropped, so exit is observed by polling `try_wait` —
/// while the master stays open — and reported through the output channel
/// (SPEC.md §8.2; `WaitForSingleObject` on `as_raw_handle` is the documented
/// fallback if polling latency ever bites).
#[cfg(windows)]
fn spawn_exit_poller(
    executor: &BackgroundExecutor,
    child: SharedChild,
    output_tx: async_channel::Sender<PtyOutput>,
) -> Task<()> {
    let timer_executor = executor.clone();
    executor.spawn(async move {
        if cfg!(test) {
            eprintln!("[exit-poller] started");
        }
        let mut ticks = 0u32;
        loop {
            timer_executor.timer(EXIT_POLL_INTERVAL).await;
            ticks += 1;
            let status = {
                let mut guard = child.lock();
                let Some(live_child) = guard.as_mut() else {
                    // The reader thread reaped first (master already dropped).
                    if cfg!(test) {
                        eprintln!("[exit-poller] tick {ticks}: child already reaped");
                    }
                    return;
                };
                let polled = live_child.try_wait();
                if cfg!(test) && (ticks <= 3 || ticks % 100 == 0 || !matches!(polled, Ok(None))) {
                    eprintln!("[exit-poller] tick {ticks}: {polled:?}");
                }
                match polled {
                    Ok(None) => continue,
                    Ok(Some(status)) => {
                        guard.take();
                        std_exit_status(status)
                    }
                    Err(error) => {
                        log::warn!("failed to poll terminal child for exit: {error}");
                        return;
                    }
                }
            };
            for event in exit_event_sequence(status) {
                if output_tx.send(PtyOutput::Event(event)).await.is_err() {
                    return;
                }
            }
            if cfg!(test) {
                eprintln!("[exit-poller] exit reported after {ticks} ticks");
            }
            return;
        }
    })
}

/// The G3 PTY integration suite (SPEC.md §7; verification-strategy.md §5):
/// spawn, resize, kill, and exit-status propagation exercised end-to-end
/// through the seam, plus the §8.2 ConPTY shutdown/exit/kill hazards on
/// Windows.
#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt as _;
    use gpui::TestAppContext;
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt as _;
    use std::time::{Duration, Instant};

    // Generous because first-use ConPTY sessions on cold Windows CI runners
    // can take tens of seconds to produce their first output. While a PTY is
    // live its exit poller ticks every 100 ms, which keeps individual parks
    // under the test scheduler's 15 s hard parking limit.
    const TEST_TIMEOUT: Duration = Duration::from_secs(60);

    /// Control experiment, deliberately below the seam: portable-pty driven
    /// directly from a plain `#[test]` with std threads — no gpui, no async
    /// channels, no `spawn_pty`. Separates "portable-pty cannot run ConPTY on
    /// this host" from "something in the seam or the gpui test harness breaks
    /// it": if this passes while the `conpty_*` tests fail, the fault is
    /// above portable-pty; if it fails too, the substrate itself is unfit.
    #[cfg(windows)]
    #[test]
    fn conpty_direct_portable_pty_control() {
        use std::io::Read as _;
        use std::sync::mpsc;

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty failed");

        let command = CommandBuilder::new("cmd.exe");
        let mut child = pair
            .slave
            .spawn_command(command)
            .expect("spawn cmd.exe failed");
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().expect("clone reader failed");
        let (bytes_tx, bytes_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        eprintln!("[direct-control] read: {count}");
                        if bytes_tx.send(buffer[..count].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        eprintln!("[direct-control] read error: {error}");
                        break;
                    }
                }
            }
        });

        // The banner (or any output beyond the ~20-byte ConPTY preamble)
        // proves the child executed.
        let mut total = Vec::new();
        let deadline = Instant::now() + TEST_TIMEOUT;
        while total.len() < 40 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match bytes_rx.recv_timeout(remaining) {
                Ok(batch) => total.extend_from_slice(&batch),
                Err(error) => panic!(
                    "no output beyond {} bytes from a direct portable-pty cmd.exe session: {error}; \
                     output so far: {:?}",
                    total.len(),
                    String::from_utf8_lossy(&total)
                ),
            }
        }
        eprintln!(
            "[direct-control] session produced output: {:?}",
            String::from_utf8_lossy(&total)
        );

        if let Err(error) = child.kill() {
            eprintln!("[direct-control] kill failed: {error}");
        }
        drop(pair.master);
        let status = child.wait().expect("wait failed");
        eprintln!("[direct-control] child exited: {status:?}");
    }

    #[cfg(unix)]
    fn shell_options(command: &str) -> PtyOptions {
        PtyOptions {
            shell: Some((
                "/bin/sh".to_string(),
                vec!["-c".to_string(), command.to_string()],
            )),
            working_directory: None,
            env: HashMap::default(),
            window_id: 0,
        }
    }

    #[cfg(windows)]
    fn cmd_options(args: &[&str]) -> PtyOptions {
        PtyOptions {
            shell: Some((
                "cmd.exe".to_string(),
                args.iter().map(|arg| arg.to_string()).collect(),
            )),
            working_directory: None,
            env: HashMap::default(),
            window_id: 0,
        }
    }

    async fn recv_output(
        output_rx: &async_channel::Receiver<PtyOutput>,
        executor: &BackgroundExecutor,
    ) -> PtyOutput {
        futures::select_biased! {
            output = output_rx.recv().fuse() => output.expect("pty output channel closed unexpectedly"),
            _ = executor.timer(TEST_TIMEOUT).fuse() => panic!("timed out waiting for pty output"),
        }
    }

    /// Collects output until the child-exit sequence completes. Asserts that
    /// `ChildExit` is followed by `Exit` and `Wakeup` (bytes may interleave on
    /// Windows, where the reader and the exit poller are separate producers).
    async fn drain_until_exit(
        output_rx: &async_channel::Receiver<PtyOutput>,
        executor: &BackgroundExecutor,
    ) -> (Vec<u8>, ExitStatus) {
        let mut bytes = Vec::new();
        let status = loop {
            match recv_output(output_rx, executor).await {
                PtyOutput::Bytes(batch) => bytes.extend_from_slice(&batch),
                PtyOutput::Event(TerminalBackendEvent::ChildExit(status)) => break status,
                PtyOutput::Event(event) => {
                    panic!("expected ChildExit before other exit events, got {event:?}")
                }
            }
        };
        let mut saw_exit = false;
        loop {
            match recv_output(output_rx, executor).await {
                PtyOutput::Bytes(batch) => bytes.extend_from_slice(&batch),
                PtyOutput::Event(TerminalBackendEvent::Exit) => saw_exit = true,
                PtyOutput::Event(TerminalBackendEvent::Wakeup) if saw_exit => {
                    return (bytes, status);
                }
                PtyOutput::Event(event) => panic!("unexpected event in exit sequence: {event:?}"),
            }
        }
    }

    /// Waits until the session has demonstrably produced `min_total` bytes of
    /// output — on Windows, acting on a ConPTY session before conhost is
    /// fully up races its startup, so the §8.2 tests establish liveness
    /// first. Sized in bytes rather than matched on content: ConPTY output is
    /// a VT stream whose text framing is not guaranteed, and its startup
    /// preamble (~20 bytes across two reads on the CI runner) precedes any
    /// child text such as the cmd banner.
    #[cfg(windows)]
    async fn wait_for_output_bytes(
        min_total: usize,
        output_rx: &async_channel::Receiver<PtyOutput>,
        executor: &BackgroundExecutor,
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        while bytes.len() < min_total {
            match recv_output(output_rx, executor).await {
                PtyOutput::Bytes(batch) => bytes.extend_from_slice(&batch),
                // An exit event first means the session ended early; surface
                // that instead of timing out opaquely.
                PtyOutput::Event(event) => panic!(
                    "session ended after {} bytes, before reaching {min_total}: got {event:?}",
                    bytes.len()
                ),
            }
        }
        bytes
    }

    /// The ConPTY startup preamble alone is ~20 bytes; the cmd banner pushes
    /// a session past this.
    #[cfg(windows)]
    const SESSION_LIVE_BYTES: usize = 40;

    /// Waits for a thread to finish while draining the output channel, so a
    /// producer blocked on the bounded channel can always make progress.
    async fn assert_thread_finishes(
        thread: &JoinHandle<()>,
        output_rx: &async_channel::Receiver<PtyOutput>,
        executor: &BackgroundExecutor,
    ) {
        let deadline = Instant::now() + TEST_TIMEOUT;
        while !thread.is_finished() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for pty thread {:?} to finish",
                thread.thread().name()
            );
            while output_rx.try_recv().is_ok() {}
            executor.timer(Duration::from_millis(10)).await;
        }
    }

    #[cfg(unix)]
    #[gpui::test]
    async fn pty_spawn_propagates_exit_status(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let executor = cx.background_executor.clone();

        let (output_tx, output_rx) = output_channel();
        let spawned = spawn_pty(
            shell_options("printf ready; exit 7"),
            TerminalBounds::default(),
            output_tx,
            &executor,
        )
        .expect("failed to spawn pty");

        assert!(spawned.process_id_getter.fallback_pid().as_u32() > 0);

        let (bytes, status) = drain_until_exit(&output_rx, &executor).await;
        assert!(
            String::from_utf8_lossy(&bytes).contains("ready"),
            "child output should reach the byte channel, got: {:?}",
            String::from_utf8_lossy(&bytes)
        );
        assert_eq!(status.code(), Some(7), "exit code must propagate exactly");

        assert_thread_finishes(&spawned.reader_thread, &output_rx, &executor).await;
        spawned.handle.shutdown();
        assert_thread_finishes(&spawned.writer_thread, &output_rx, &executor).await;
    }

    #[cfg(unix)]
    #[gpui::test]
    async fn pty_input_reaches_child(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let executor = cx.background_executor.clone();

        let (output_tx, output_rx) = output_channel();
        let spawned = spawn_pty(
            shell_options(r#"read line; printf 'pong:%s' "$line""#),
            TerminalBounds::default(),
            output_tx,
            &executor,
        )
        .expect("failed to spawn pty");

        spawned.handle.notify(&b"ping\r"[..]);

        let (bytes, status) = drain_until_exit(&output_rx, &executor).await;
        assert!(
            String::from_utf8_lossy(&bytes).contains("pong:ping"),
            "writer-thread input should reach the child, got: {:?}",
            String::from_utf8_lossy(&bytes)
        );
        assert_eq!(status.code(), Some(0));
    }

    #[cfg(unix)]
    #[gpui::test]
    async fn pty_resize_updates_kernel_winsize(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let executor = cx.background_executor.clone();

        let (output_tx, output_rx) = output_channel();
        let spawned = spawn_pty(
            shell_options("exec sleep 60"),
            TerminalBounds::default(),
            output_tx,
            &executor,
        )
        .expect("failed to spawn pty");

        let bounds = TerminalBounds::new(
            gpui::px(10.),
            gpui::px(5.),
            gpui::Bounds {
                origin: gpui::Point::default(),
                size: gpui::size(gpui::px(400.), gpui::px(300.)),
            },
        );
        spawned.handle.resize(bounds);

        let size = spawned
            .handle
            .master()
            .expect("master must be open before drop")
            .get_size()
            .expect("failed to read pty size");
        assert_eq!((size.rows, size.cols), (30, 80));
        assert_eq!((size.pixel_width, size.pixel_height), (400, 300));

        spawned.handle.shutdown();
        let (_, status) = drain_until_exit(&output_rx, &executor).await;
        assert_eq!(
            status.signal(),
            Some(libc::SIGHUP),
            "shutdown must signal the child like the alacritty pty drop did"
        );
        assert_thread_finishes(&spawned.reader_thread, &output_rx, &executor).await;
        assert_thread_finishes(&spawned.writer_thread, &output_rx, &executor).await;
    }

    #[cfg(unix)]
    #[gpui::test]
    async fn pty_kill_terminates_long_running_child(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let executor = cx.background_executor.clone();

        let (output_tx, output_rx) = output_channel();
        let spawned = spawn_pty(
            shell_options("exec sleep 60"),
            TerminalBounds::default(),
            output_tx,
            &executor,
        )
        .expect("failed to spawn pty");

        spawned.handle.shutdown();

        let (_, status) = drain_until_exit(&output_rx, &executor).await;
        assert!(!status.success());
        assert_eq!(status.signal(), Some(libc::SIGHUP));
        assert_thread_finishes(&spawned.reader_thread, &output_rx, &executor).await;
        assert_thread_finishes(&spawned.writer_thread, &output_rx, &executor).await;
    }

    /// The resize half of the G3 suite on ConPTY: `PtyHandle::resize` must
    /// apply against a live pseudoconsole (portable-pty updates its cached
    /// size only after the ConPTY resize succeeds).
    #[cfg(windows)]
    #[gpui::test]
    async fn conpty_resize_applies(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let executor = cx.background_executor.clone();

        let (output_tx, output_rx) = output_channel();
        let spawned = spawn_pty(
            cmd_options(&[]),
            TerminalBounds::default(),
            output_tx,
            &executor,
        )
        .expect("failed to spawn pty");

        // Acting on a ConPTY session before conhost is up races its startup;
        // wait for the cmd banner first.
        eprintln!("[conpty-resize] spawned; waiting for banner");
        wait_for_output_bytes(SESSION_LIVE_BYTES, &output_rx, &executor).await;

        let bounds = TerminalBounds::new(
            gpui::px(10.),
            gpui::px(5.),
            gpui::Bounds {
                origin: gpui::Point::default(),
                size: gpui::size(gpui::px(400.), gpui::px(300.)),
            },
        );
        spawned.handle.resize(bounds);
        eprintln!("[conpty-resize] resized");

        let size = spawned
            .handle
            .master()
            .expect("master must be open before drop")
            .get_size()
            .expect("failed to read pty size");
        assert_eq!((size.rows, size.cols), (30, 80));

        spawned.handle.shutdown();
        eprintln!("[conpty-resize] shut down; waiting for threads");
        let SpawnedPty {
            handle,
            reader_thread,
            writer_thread,
            ..
        } = spawned;
        drop(handle);
        assert_thread_finishes(&reader_thread, &output_rx, &executor).await;
        assert_thread_finishes(&writer_thread, &output_rx, &executor).await;
    }

    /// §8.2 ConPTY shutdown hazard: EOF arrives only once the pseudoconsole is
    /// dropped, so closing a terminal must not hang waiting for it.
    #[cfg(windows)]
    #[gpui::test]
    async fn conpty_shutdown_joins_reader_without_eof_hang(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let executor = cx.background_executor.clone();

        let (output_tx, output_rx) = output_channel();
        let spawned = spawn_pty(
            cmd_options(&[]),
            TerminalBounds::default(),
            output_tx,
            &executor,
        )
        .expect("failed to spawn pty");

        // Wait for the cmd banner so the session is fully up.
        wait_for_output_bytes(SESSION_LIVE_BYTES, &output_rx, &executor).await;

        let SpawnedPty {
            handle,
            reader_thread,
            writer_thread,
            ..
        } = spawned;
        handle.shutdown();
        // Dropping the handle closes the pseudoconsole (on a detached closer
        // thread), which is what delivers reader EOF; keeping only the join
        // handles mirrors `Terminal::drop`.
        drop(handle);

        assert_thread_finishes(&reader_thread, &output_rx, &executor).await;
        assert_thread_finishes(&writer_thread, &output_rx, &executor).await;
    }

    /// §8.2 ConPTY exit-observation hazard: the exit code must be observed via
    /// `try_wait` polling while the master (and thus the reader) is still
    /// open, not via reader EOF.
    #[cfg(windows)]
    #[gpui::test]
    async fn conpty_exit_observed_while_master_open(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let executor = cx.background_executor.clone();

        let (output_tx, output_rx) = output_channel();
        // Interactive cmd with `exit 42` typed through the writer thread once
        // the banner has arrived. Typing must come after the banner: input
        // written during conhost startup wedges the session, and children
        // spawned *with arguments* never execute at all on this runner's
        // in-box ConPTY (ledger P4-001), so interactive cmd is the one
        // reliable vehicle for a natural exit here.
        let spawned = spawn_pty(
            cmd_options(&[]),
            TerminalBounds::default(),
            output_tx,
            &executor,
        )
        .expect("failed to spawn pty");

        eprintln!("[conpty-exit] spawned; waiting for banner");
        wait_for_output_bytes(SESSION_LIVE_BYTES, &output_rx, &executor).await;
        eprintln!("[conpty-exit] banner up; typing exit");
        spawned.handle.notify(&b"exit 42\r"[..]);

        // The master (and with it the pseudoconsole) stays open for the whole
        // wait: observing the exit here proves it does not depend on reader
        // EOF, which ConPTY may withhold until the pseudoconsole is dropped.
        let (_, status) = drain_until_exit(&output_rx, &executor).await;
        assert_eq!(status.code(), Some(42));
        drop(spawned);
    }

    /// §8.2 ConPTY kill path: killing a long-running child (the wezterm#7709
    /// `TerminateProcess` fix) must terminate it and let the reader join.
    #[cfg(windows)]
    #[gpui::test]
    async fn conpty_kill_terminates_long_running_child(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let executor = cx.background_executor.clone();

        let (output_tx, output_rx) = output_channel();
        // Interactive cmd as the long-running child (children spawned with
        // arguments never execute on this runner's in-box ConPTY — ledger
        // P4-001 — and an idle interactive cmd runs until killed).
        let spawned = spawn_pty(
            cmd_options(&[]),
            TerminalBounds::default(),
            output_tx,
            &executor,
        )
        .expect("failed to spawn pty");

        // Kill only once the session has demonstrably started; killing into a
        // half-started ConPTY races conhost startup.
        eprintln!("[conpty-kill] spawned; waiting for banner");
        wait_for_output_bytes(SESSION_LIVE_BYTES, &output_rx, &executor).await;

        spawned.handle.shutdown();
        eprintln!("[conpty-kill] shut down; waiting for exit report");

        let (_, status) = drain_until_exit(&output_rx, &executor).await;
        assert!(!status.success(), "killed child must not report success");
        eprintln!("[conpty-kill] exit observed; waiting for threads");

        let SpawnedPty {
            handle,
            reader_thread,
            writer_thread,
            ..
        } = spawned;
        drop(handle);
        assert_thread_finishes(&reader_thread, &output_rx, &executor).await;
        assert_thread_finishes(&writer_thread, &output_rx, &executor).await;
    }

    /// The sustained-flood benchmark (SPEC.md §6 P4; spec-lock resolution 3):
    /// floods the full seam — child → reader thread → bounded(4) × 64 KiB
    /// channel → batch-capped foreground ingest into the alacritty backend —
    /// and reports throughput, the per-turn foreground stall bound, and
    /// input-echo latency during the flood. Validates the channel/batch
    /// tuning; run in release via `script/terminal-flood-bench`.
    #[cfg(unix)]
    #[gpui::test]
    #[ignore = "benchmark; run via script/terminal-flood-bench"]
    async fn sustained_flood_benchmark(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let executor = cx.background_executor.clone();

        const FLOOD_BYTES: u64 = 256 * 1024 * 1024;
        const MARKER_INTERVAL: Duration = Duration::from_millis(250);

        let (output_tx, output_rx) = output_channel();
        // The flood alphabet deliberately excludes 'Z', the echo marker.
        let spawned = spawn_pty(
            shell_options(&format!(
                "yes 0123456789abcdefghijklmnopqrstuv | head -c {FLOOD_BYTES}"
            )),
            TerminalBounds::default(),
            output_tx,
            &executor,
        )
        .expect("failed to spawn flood pty");

        let (events_tx, events_rx) = futures::channel::mpsc::unbounded();
        let mut backend = crate::alacritty::TerminalBackend::new(
            10_000,
            crate::terminal_settings::CursorShape::default(),
            TerminalBounds::default(),
            events_tx,
            crate::terminal_settings::AlternateScroll::On,
        );

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
            let first = recv_output(&output_rx, &executor).await;
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
                    while batches < MAX_BATCHES_PER_TURN {
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
                                exited = matches!(event, TerminalBackendEvent::ChildExit(_));
                                break;
                            }
                            Err(_) => break,
                        }
                    }
                }
                PtyOutput::Event(event) => {
                    exited = matches!(event, TerminalBackendEvent::ChildExit(_));
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
        drop(events_rx);

        assert!(
            total_bytes >= FLOOD_BYTES,
            "flood should deliver every byte, got {total_bytes} of {FLOOD_BYTES}"
        );

        let mib = total_bytes as f64 / (1024.0 * 1024.0);
        println!(
            "sustained-flood benchmark (bounded({OUTPUT_CHANNEL_BATCHES}) × {READ_BATCH_SIZE} B channel, {MAX_BATCHES_PER_TURN} batches/turn):"
        );
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
