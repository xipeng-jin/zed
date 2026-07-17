# Divergence ledger

Adjudication record for the `alacritty_terminal` → libghostty-vt migration
(prescribed by [verification-strategy.md §3.4](verification-strategy.md#34-divergence-ledger);
operative spec: [SPEC.md §7](SPEC.md#7-verification)). Every divergence — a
differential mismatch or a ported-test expectation change — gets an entry
here: the triggering input, both behaviors, and the adjudication (**fixed**
in the seam/core, or **accepted** with rationale). Accepted entries are the
only permitted deltas at gate time.

The ledger's full seeding happens at P7 with the differential harness; it is
opened early because P2's acceptance criteria require entries for the two
retired `Arc`-sharing round-trip tests (SPEC.md §6, P2 row).

Entry ID format: `<phase>-<sequence>`.

---

## P2-001 — `terminal_hyperlink_from_alacritty_keeps_alacritty_storage` retired

- **Phase / change**: P2 (Zed-owned domain types, ticket #43). `Hyperlink`
  dropped its `Alacritty` storage variant and became a fully owned
  `{ id: Option<Arc<str>>, uri: Arc<str> }` struct (SPEC.md §4.2, S2).
- **Triggering input**: `crates/terminal/src/alacritty.rs` test
  `terminal_hyperlink_from_alacritty_keeps_alacritty_storage`, which asserted
  `matches!(&hyperlink.data, HyperlinkData::Alacritty(_))` — i.e. that
  conversion kept the alacritty `Arc` storage alive inside the Zed type.
- **Old behavior**: `terminal_hyperlink_from_alacritty` wrapped the alacritty
  `Hyperlink` handle; the Zed value shared alacritty's allocation.
- **New behavior**: conversion copies `id`/`uri` into Zed-owned storage at
  snapshot build; no alacritty allocation outlives the seam.
- **Adjudication**: **accepted**. Storage sharing was an implementation
  detail of the wrapper era, unobservable through the public accessor
  surface; ghostty render cells are transient FFI handles, so owned
  materialization is forced (S2 rationale). The semantic half of the
  round-trip survives as
  `terminal_hyperlink_from_alacritty_preserves_id_and_uri`
  (id and uri preserved verbatim).

## P2-002 — `terminal_cell_from_alacritty_shares_extra_storage` retired

- **Phase / change**: P2 (Zed-owned domain types, ticket #43). `Cell` became
  fully Zed-owned (`c`/`fg`/`bg`/`flags` + rare data behind
  `Option<Arc<CellExtra>>`), converted at snapshot build (SPEC.md §4.2, S2).
- **Triggering input**: `crates/terminal/src/alacritty.rs` test
  `terminal_cell_from_alacritty_shares_extra_storage`, which asserted
  `Arc::ptr_eq` between the alacritty cell's `extra` and the converted Zed
  cell's `extra`.
- **Old behavior**: `terminal_cell_from_alacritty` cloned the alacritty cell
  wholesale, so both sides pointed at the same `CellExtra` allocation.
- **New behavior**: conversion materializes a Zed-owned `CellExtra`
  (grapheme tail + hyperlink) per snapshot; clones of the *owned* cell still
  share that one allocation.
- **Adjudication**: **accepted**, same rationale as P2-001. The semantic
  half survives as `terminal_cell_from_alacritty_preserves_zerowidth`.
  Accompanying note: the `terminal.rs` domain test
  `terminal_cell_clone_shares_extra_storage` reaches into the cell's private
  representation (`cell.cell.extra`); its field path was mechanically
  re-pointed to the owned field (`cell.extra`). Its expectation —
  `Arc::ptr_eq` across `Cell::clone` — is unchanged and still passes; no
  behavioral delta.

---

P3 entries (ghostty input encoders, ticket #44) are Linux-only for the
duration of the §5 cfg window: macOS/Windows keep the alacritty-era paths
until their §8 gates open, at which point these adjudications apply there
too. "Old behavior" below means the alacritty-era `to_esc_str` /
`mappings/mouse.rs` / `Terminal::paste` output; "new behavior" the ghostty
encoder path. Entries adjudicated **fixed in the seam** produce today's
bytes and are pinned by the permanent contract suites in
`mappings/keys.rs` / `mappings/mouse.rs` / `mappings/paste.rs`; entries
adjudicated **accepted** are pinned by the `adjudicated_divergences` /
ledger-referencing tests in the same files.

## P3-001 — F13–F20 sequences supplied by the seam

- **Phase / change**: P3 (ghostty input encoders, ticket #44). Keys route
  through ghostty `key::Encoder` (SPEC.md §4.3).
- **Triggering input**: F13–F20, plain and modified (e.g. `f13`,
  `alt-f13`).
- **Old behavior**: xterm sequences `\x1b[25~` … `\x1b[34~` (modified:
  `\x1b[25;N~` …).
- **New behavior (raw encoder)**: ghostty's legacy encoder emits nothing
  for F13+ (its legacy table stops at F12; kitty-protocol codes exist but
  kitty flags are structurally off until P8).
- **Adjudication**: **fixed in the seam**. `legacy_fill` in
  `mappings/keys.rs` supplies the alacritty-era sequences, keeping the
  contract suite byte-identical. A deliberate, bounded exception to the
  "no hand-written escape sequences" seam rule, carried until kitty
  keyboard support gives applications a first-class F13+ path; revisit at
  P8/P10.

## P3-002 — ctrl-sequence fills: `ctrl-[`, `ctrl-_`, `ctrl-?`, `ctrl-i`, `ctrl-m`

- **Phase / change**: P3, as above.
- **Triggering input**: `ctrl-[`, `ctrl-_`, `ctrl-?`, `ctrl-i`/`ctrl-m`
  (incl. ctrl-shift variants of the letters).
- **Old behavior**: `0x1b`, `0x1f`, `0x7f`, `0x09`, `0x0d`.
- **New behavior (raw encoder)**: nothing. Ghostty keys its ctrl table off
  the *physical* key (GPUI delivers the shifted character, so `_` maps to
  the Minus key, `?` to Slash), relies on host-supplied utf8 text for
  `ctrl-[` (which Zed does not pass — the binding API forbids C0 text),
  and deliberately reserves `ctrl-i`/`ctrl-m` so kitty-aware applications
  can distinguish them from tab/enter.
- **Adjudication**: **fixed in the seam**. `legacy_fill` supplies the
  single control bytes (control characters, not escape sequences — the
  seam rule is untouched). Contract suite byte-identical.

## P3-003 — alt + multi-character key names no longer leak the name

- **Phase / change**: P3, as above.
- **Triggering input**: alt + a named non-text key that the legacy tables
  missed: `alt-tab`, `alt-escape`, `alt-space`, `alt-back`.
- **Old behavior**: the alt-as-meta branch formatted `\x1b` + the *GPUI
  key name string*: `alt-tab` → `\x1btab` (4 bytes), `alt-space` →
  `\x1bspace`.
- **New behavior**: proper encodings: `\x1b\x09`, `\x1b\x1b`, `\x1b ` (and
  `alt-back` → `\x1b\x7f`).
- **Adjudication**: **accepted** — the old output was an unambiguous
  mapping bug (typed the key name into the shell). Pinned by
  `adjudicated_divergences::alt_named_keys`.

## P3-004 — modified-key table gaps now encode

- **Phase / change**: P3, as above.
- **Triggering input**: modified `f5` (any modifier) and modified
  `delete`, e.g. `shift-f5`, `shift-delete`.
- **Old behavior**: none — the legacy modified table listed `"F5"`
  (capital, never delivered by GPUI) and omitted `delete` entirely, so
  these fell through to the text path (usually a no-op).
- **New behavior**: `\x1b[15;N~`, `\x1b[3;N~`.
- **Adjudication**: **accepted** — table typo/omission; the new output is
  standard xterm. Pinned by
  `adjudicated_divergences::modified_key_table_gaps`.

## P3-005 — super modifier encodes as kitty-style 8

- **Phase / change**: P3, as above.
- **Triggering input**: super/cmd + an encodable named key, e.g.
  `cmd-up`.
- **Old behavior**: `\x1b[1;1A` — the super modifier forced the modified
  form but contributed nothing to the code, producing the malformed
  modifier code 1 ("no modifiers").
- **New behavior**: `\x1b[1;9A` — kitty's super bit (8).
- **Adjudication**: **accepted** — the old form was malformed; ghostty's
  is the convention modern terminals share. Pinned by
  `adjudicated_divergences::super_modifier`.

## P3-006 — previously-silent combos emit xterm CSI 27 encodings

- **Phase / change**: P3, as above.
- **Triggering input**: modifier+key combos with no legacy sequence:
  `ctrl-enter`, `ctrl-tab`, `shift-escape`, `ctrl-shift-tab`, ….
- **Old behavior**: nothing (fell through; typically a no-op).
- **New behavior**: xterm "other keys" encodings, e.g. `ctrl-enter` →
  `\x1b[27;5;13~`.
- **Adjudication**: **accepted** — an incidental capability gain in the
  #32 resolution's sense: bytes where none were sent before, using the
  xterm-standard form; shells that don't bind them ignore the sequence.
  Watch during the P9 soak. Pinned by
  `adjudicated_divergences::csi_27_combos`.

## P3-007 — paste sanitization is a superset

- **Phase / change**: P3 (ghostty `paste::encode` owns paste bytes).
- **Triggering input**: pasted text containing ESC / NUL / DEL.
- **Old behavior**: bracketed — ESC characters *removed*, NUL/DEL passed
  through; unbracketed — all three passed through raw.
- **New behavior**: unsafe control bytes are replaced with spaces in both
  modes (defusing bracketed-paste-end injection even unbracketed).
- **Adjudication**: **accepted** — verified superset per the #32
  resolution, which assigned paste sanitization to ghostty wholesale. The
  `\r\n`→`\r` collapse contract is preserved by pre-normalizing
  `\r\n`→`\n` before encoding (the resolution's one required check).
  Pinned by `paste::tests::ghostty_sanitization_superset`.

## P3-008 — modified F3 uses `CSI 13;N~`

- **Phase / change**: P3, as above.
- **Triggering input**: F3 with any modifier, e.g. `shift-f3`.
- **Old behavior**: `\x1b[1;2R`.
- **New behavior**: `\x1b[13;2~`.
- **Adjudication**: **accepted** — `CSI 1;N R` collides with the cursor
  position report; xterm moved modified F3 to `CSI 13;N~` and ghostty
  follows. Pinned by `adjudicated_divergences::modified_f3`.

## P3-009 — UTF-8 mouse coordinates beyond 2014 encode instead of dropping

- **Phase / change**: P3 (ghostty `mouse::Encoder` owns mouse wire
  formats).
- **Triggering input**: mode 1005 (UTF-8 mouse) report with a coordinate
  ≥ 2015 — requires a terminal grid wider/taller than 2015 cells.
- **Old behavior**: the report was dropped entirely.
- **New behavior**: the coordinate is encoded as three-byte UTF-8.
- **Adjudication**: **accepted** — valid UTF-8 the peer can decode, in a
  configuration that cannot occur in practice; dropping input was the
  worse behavior. X10-format caps (drop beyond 222) are unchanged and
  byte-identical. Pinned in
  `mouse::tests::contract::utf8_format_wide_coordinates`.

## P3-010 — ctrl-shift-letter encodes the caret code

- **Phase / change**: P3, as above.
- **Triggering input**: `ctrl-shift-a` … `ctrl-shift-z`.
- **Old behavior**: nothing. The legacy table's ctrl-shift rows keyed on
  *uppercase* keys (`("A", CtrlShift)`), but GPUI's canonical keystroke
  form is lowercase key + shift modifier, so those rows never matched —
  ctrl-shift-letter fell through to the text path (a no-op). The upstream
  `test_ctrl_codes` suite asserted only that `ctrl-shift-x` equals
  `ctrl-X`, which held vacuously as `None == None`; that ported
  expectation still holds on the new path (both now the caret byte).
- **New behavior**: the same caret code as plain ctrl-letter (`0x01` …
  `0x1a`), matching xterm and every mainstream terminal. The seam strips
  shift before handing letters to ghostty's ctrl mapping (and
  `legacy_fill` does the same for its `ctrl-i`/`ctrl-m` bytes).
- **Adjudication**: **accepted** — the old silence was an artifact of the
  dead uppercase rows, not a behavioral choice. Pinned by
  `adjudicated_divergences::ctrl_shift_letters`.

---

P4 entries (PTY/threading swap, ticket #45) apply on **all platforms
immediately** — the PTY seam is shared, not behind the §5 cfg. No ported-test
expectation changed at P4 (Class B ran untouched); these entries record
behavioral deltas of the alacritty `tty`/`EventLoop` → portable-pty seam
found by inspection, per the P3 precedent.

## P4-001 — Windows cmd.exe arguments are always MSVC-quoted

- **Phase / change**: P4 (PTY seam on portable-pty, ticket #45). The
  alacritty Windows tty accepted `escape_args: false`
  (`ShellKind::Cmd`) and joined arguments into the ConPTY command line raw;
  Zed passed `shell_kind.tty_escape_args()` to get that. portable-pty's
  `CommandBuilder` has no raw mode: every argument goes through
  MSVC-C-runtime quoting (`append_quoted`).
- **Triggering input**: a Windows terminal/task with `ShellKind::Cmd` whose
  arguments contain spaces or embedded quotes, e.g.
  `cmd /C echo "hello world"`.
- **Old behavior**: `cmd /C echo "hello world"` (raw join; cmd parses its
  own quoting).
- **New behavior**: arguments with spaces are wrapped and inner quotes
  backslash-escaped (`cmd /C "echo \"hello world\""`); cmd.exe does not
  understand backslash-escaped quotes.
- **Adjudication**: **accepted for the cfg window, re-examined at the
  Windows gate (§8.2)**. Zed's default Windows shell is PowerShell (escaped
  today already, and portable-pty's quoting matches); only user-configured
  cmd.exe with quote-bearing args diverges. The §8.2 manual smoke ("run a
  task") exercises this; if it bites, the documented exit is a raw-cmdline
  patch in the portable-pty fork-or-vendor path (D1's exit strategy).
  `util::shell::ShellKind::tty_escape_args` is dead code until then.
- **P4 CI evidence (2026-07-17)**: this is worse than a quote mis-parse on
  at least some hosts. On `windows-latest`, `cmd.exe /C "<quoted arg with
  spaces>"` under ConPTY emitted a 4-byte VT preamble and then wedged —
  the command never executed, the child never exited, and after
  `TerminateProcess` the wedged conhost also withheld reader EOF past
  pseudoconsole close (instrumented run `d6d58ce20a`, `pty_integration`
  job). Interactive `cmd.exe` and unquoted command lines behave normally,
  and the PTY suite uses only those. Raises the §8.2 gate priority of the
  raw-cmdline exit; `ShellKind::Cmd` tasks with spaced arguments should be
  assumed broken on Windows until the gate resolves this.

## P4-002 — Missing or invalid working directory falls back to `$HOME` (unix)

- **Phase / change**: P4, as above.
- **Triggering input**: `TerminalBuilder::new` with `working_directory:
  None`, or a path that no longer exists.
- **Old behavior**: alacritty's `pre_exec` ignored a failed `chdir`; the
  child inherited Zed's own working directory.
- **New behavior**: portable-pty's `as_command` resolves the cwd to `$HOME`
  when the requested directory is unset or not a directory.
- **Adjudication**: **accepted** — Zed passes a concrete project directory
  on every mainline path, and `$HOME` is the friendlier fallback for the
  rare orphan case (matches what standalone terminals do). Unobservable in
  any suite.

## P4-003 — Default-shell resolution validates `$SHELL`; `SHELL` is always exported

- **Phase / change**: P4, as above. The no-explicit-shell case (unix
  `Shell::System`) resolves through the seam instead of alacritty's
  `ShellUser::from_env`.
- **Triggering input**: spawning with `shell: None` while `$SHELL` points at
  a non-executable path, or with `SHELL`/`USER`/`HOME` absent from Zed's
  environment.
- **Old behavior**: `$SHELL` was trusted verbatim (spawn failed later if
  bogus); `USER`/`HOME` were re-derived env-first-then-passwd and explicitly
  set on the child; `SHELL` itself was not set for explicit commands.
- **New behavior**: portable-pty falls back to the passwd shell when
  `$SHELL` is not executable, and exports `SHELL=<resolved shell>` to every
  child; `USER`/`HOME` ride the inherited base environment (no passwd
  re-derivation). The macOS `/usr/bin/login` wrapper is preserved
  seam-side, gated on `$USER` being present (its ghostty-derived
  replacement lands at the §8.1 gate).
- **Adjudication**: **accepted** — strictly more robust resolution; the
  passwd re-derivation only mattered when Zed itself ran without
  `USER`/`HOME`, which the .app launch path does not produce.

## P4-004 — Headless subprocess stdout/stderr share one parser

- **Phase / change**: P4, as above. The `HeadlessTerminal` subprocess pumps
  send raw bytes over the same bounded channel as the PTY reader; the
  foreground ingests through the backend's single parser.
- **Triggering input**: a headless-host task writing ANSI to stdout and
  stderr concurrently.
- **Old behavior**: each stream had its own vte parser; the shared term
  mutex interleaved *parsed effects* at lock granularity, so a split escape
  sequence on one stream could not corrupt the other's parse state.
- **New behavior**: batches interleave at channel granularity into one
  parser; an escape sequence split across a batch boundary can interleave
  with the other stream's bytes mid-sequence.
- **Adjudication**: **accepted** — the architecture note (§6) already
  classed this as "the same observable class of interleaving"; concurrent
  mixed-stream ANSI was never ordering-guaranteed, and the headless path is
  eval-CLI-only. The existing `test_no_pty_task_terminal_captures_output`
  suite passes untouched.

## P4-005 — Pump coalescing timer replaced by the bounded batch cap

- **Phase / change**: P4, as above. The foreground pump in
  `TerminalBuilder::subscribe` re-shaped around the bounded byte channel.
- **Triggering input**: any PTY output — most visibly interactive trickles
  (keystroke echo) and floods.
- **Old behavior**: the pump coalesced events on a 4 ms timer with a
  100-event cap before each `terminal.update`;
  pty-threading-architecture.md §3.2(d) anticipated that shape surviving
  "carrying bytes instead of parsed events" (open question 6 left the timer's
  necessity to empirical validation).
- **New behavior**: no timer. Each turn ingests up to
  `MAX_BATCHES_PER_TURN × READ_BATCH_SIZE` (4 × 64 KiB) then yields; each
  emulator-origin event gets its own update. SPEC.md §3 D2 mandates exactly
  this ("a bounded number of batches per turn, then yields") and formally
  superseded the spike's time-budgeted drain.
- **Adjudication**: **accepted** — the empirical validation open question 6
  asked for is the P4 sustained-flood benchmark: 84.2 MiB/s through the
  seam, max per-turn foreground stall 2.2 ms (release), echo latency
  ≤ 623 µs mean 264 µs during flood; interactive rows re-verified on the
  `:99` harness. Coalescing knobs remain a post-removal tuning rider
  (SPEC.md §6) if a real regression appears.
