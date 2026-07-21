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

---

P5 entries (dark ghostty backend core, ticket #46) describe the dark
`ghostty::TerminalBackend` only — production stays on alacritty until P8, so
nothing here is user-observable during the dark window. Entries are recorded
now because the backend suite pins the behavior.

## P5-001 — Scrollback limit is page-granular, not an exact line count

- **Phase / change**: P5 (dark ghostty backend core, ticket #46). The
  creation contract sets `Options.max_scrollback` from Zed's
  `scrolling_history` setting (SPEC.md §3).
- **Triggering input**: any terminal whose scrollback grows past the
  configured line limit (e.g. `max_scroll_history_lines: 2` followed by
  thousands of output lines).
- **Old behavior**: alacritty's grid caps history at *exactly* the
  configured number of lines; the oldest line disappears as soon as line
  N+1 scrolls off.
- **New behavior**: ghostty's `max_scrollback` is a byte limit over its
  page storage, rounded up to whole 512 KiB pages (despite the C header
  documenting "lines"; `Screen.init` documents bytes). The seam converts
  lines → whole standard pages (`lines.div_ceil(215) × 512 KiB`, a
  standard page holding 215 rows at up to 215 columns), so at least the
  configured line count is retained in the common case, and pruning
  happens at page granularity. A terminal may retain *more* history than
  configured (up to the page boundary), and rows with very wide grids or
  heavy grapheme/style data may retain slightly less. Zero remains exact:
  scrollback fully disabled.
- **Adjudication**: **accepted**. The setting's intent is a memory bound
  with an approximate history horizon, which the page conversion
  preserves; ghostty's own scrollback-limit config has identical
  semantics, storage is allocated lazily, and historical pages compress.
  Exact line-count parity is structurally unavailable without forking the
  core. Pinned by
  `ghostty::tests::creation_contract_bounds_scrollback` (zero exact,
  non-zero bounded) and, from P7, by the differential corpus row
  `scrollback_trim_past_limit` (ghostty retains more; alacritty's
  extraction is a suffix of ghostty's; viewport and recent-line probes
  exact).

## P5-002 — Selection text trims trailing spaces at a mid-row selection cut

- **Phase / change**: P5 (dark ghostty backend core, ticket #46).
  `selection_text` / `Content::selection_text` format through ghostty's
  selection formatter (plain, unwrap, trim), matching ghostty's own
  `Screen.selectionString` copy semantics.
- **Triggering input**: a selection whose end lands on space cells that are
  *interior* to the row — e.g. selecting `b ` out of `a世b 世世x` (the row
  continues with text after the selected space).
- **Old behavior**: alacritty's `bounds_to_string` cuts each row at
  `min(row.line_length(), selection end + 1)`; because the row's last
  non-space cell lies beyond the selection, the selected space survives:
  `"b "`.
- **New behavior**: ghostty's formatter trims trailing whitespace per
  formatted line, so the selection-trailing space is dropped: `"b"`.
  Row-trailing whitespace (typed or unwritten) is trimmed identically by
  both backends, so the delta is exactly the mid-row cut case.
- **Adjudication**: **accepted**. Neither formatter flag reproduces
  alacritty's per-row `line_length` cut (`trim` drops the interior space,
  `no-trim` keeps unwritten trailing blanks alacritty removes), and an
  exact seam reconstruction would re-derive row content lengths around the
  unwrap join for a cosmetic whitespace difference in copied text. Ghostty
  the terminal ships this exact behavior for its own copy path. Pinned by
  `ghostty::tests::simple_selection_over_wide_chars_matches_alacritty`
  (case 3 asserts both behaviors, ledger-referenced).

---

P6 entries describe the dark backend's gap-fill components (SPEC.md §6) —
unused by production until P8. As with P5, entries are recorded when a suite
pins the behavior. No ported-test expectation changed in the hover re-host
(ticket #48): the 36 hyperlink scenarios run byte-identical.

## P6-001 — Adjacent identical-URI OSC 8 links merge into one hover region

- **Phase / change**: P6 (hover/hyperlink pipeline re-host, ticket #48).
  The hover-extent walk in `ghostty/hyperlinks.rs` compares URIs because
  ghostty exposes no OSC 8 `id` (gap G7); `Hyperlink.id` stays `None`.
- **Triggering input**: two adjacent OSC 8 links with distinct `id`
  parameters but the same URI, hover over either.
- **Old behavior**: alacritty compares the full hyperlink (`id` + URI), so
  the two links are separate hover regions.
- **New behavior**: URI equality merges them into one hover region.
- **Adjudication**: **accepted** — pre-adjudicated at spec lock (SPEC.md §9
  "accepted merge"; §4.2 S2). Pinned by
  `ghostty::hyperlinks::tests::osc8::adjacent_distinct_links_with_identical_uris_merge`.

## P6-002 — Hover logical-line walk caps at 100 wrapped rows

- **Phase / change**: P6, as above. `line_search_left`/`line_search_right`
  bound their wrap-flag walks at `MAX_SEARCH_LINES` (100) rows per
  direction from the hover point.
- **Triggering input**: hovering inside a logical line that spans more than
  100 wrapped rows (a pathological fully-wrapped buffer) — URL/path
  detection sees a truncated logical line.
- **Old behavior**: Zed's alacritty fork walks the wrap chain uncapped
  (`Term::line_search_left/right` have no bound at the pinned rev).
- **New behavior**: the walk truncates at 100 wrapped rows per direction —
  the bound SPEC.md §4.4 mandates, ported from upstream alacritty's hint
  highlighting (`MAX_SEARCH_LINES`).
- **Adjudication**: **accepted** — spec-mandated (§4.4 "alacritty's
  100-wrapped-row cap"): a bounded hover cost beats exact parity on
  degenerate buffers. No scenario exercises >100 wrapped rows; documented
  in the module doc.

## P6-003 — URL hover text drops zerowidth combining marks

- **Phase / change**: P6, as above. Both URL and path regexes run over one
  extracted base-chars-only line, and the reported URL text is sliced from
  that same line.
- **Triggering input**: hovering a URL containing a combining mark written
  as separate codepoints (e.g. `e` + U+0301) inside an OSC-8-free line.
- **Old behavior**: alacritty's URL regex also matches on base chars only,
  but the matched text is re-extracted via `bounds_to_string`, which
  appends zerowidth chars — the reported URL keeps the combining marks.
- **New behavior**: the reported URL text is the base-chars-only match
  slice — combining marks are dropped.
- **Adjudication**: **accepted** — alacritty's own path branch already
  dropped zerowidth chars (its hover line is built from `cell.c`), so the
  delta is URL-branch-only and keeps URL text consistent with path text;
  a URL whose identity depends on unnormalized combining marks is
  pathological. No ported expectation changed; the `iri` scenarios
  (single-codepoint wide chars) run byte-identical.

## P6-004 — The vi cursor does not ride content rotation during live output

- **Phase / change**: P6 (vi mode port, ticket #49). The seam keeps the vi
  cursor as a plain grid-convention point on `ghostty::TerminalBackend`;
  alacritty's `vi_mode_cursor` lives inside `Term`, where
  `scroll_up_relative` / `scroll_down_relative` shift it with rotating
  content and `Term::resize` shifts it by the content delta before the
  viewport clamp. Those rotations happen below the seam in ghostty (inside
  `vt_write` / `resize`), where the seam has no hook. Recorded by
  inspection, per the P4 precedent — no ported-test expectation changed
  (upstream's vi suite only exercises static grids).
- **Triggering input**: vi mode active while the program keeps emitting
  scrolling output (e.g. entering vi mode during a running build), or a
  window resize that moves rows between screen and history while the vi
  cursor is set.
- **Old behavior**: the vi cursor sticks to the content line it was on,
  following it toward/into scrollback as new lines arrive (clamped to the
  viewport top when display-pinned), and rides the resize content delta.
- **New behavior**: the vi cursor keeps its viewport-relative grid
  coordinates while content rotates beneath it; the ported seam clamps it
  to the viewport on `scroll_display` and `resize` exactly as alacritty
  does, so it never leaves the visible region or the grid.
- **Adjudication**: **accepted**. Vi mode is a navigation mode over
  quiescent output; every Zed-dispatched interaction (motions,
  scroll-follow, selection drag, goto) recomputes the cursor through the
  ported paths, which the upstream-seeded suite and the alacritty
  differential tests pin. A tracked-grid-ref cursor would follow content
  but diverge from alacritty's viewport-clamp behavior instead. Revisit if
  the P7 differential corpus surfaces a user-visible delta.

---

P7 entries come from the differential harness itself (ticket #50): identical
transcripts fed to both backends inside one test binary, seam snapshots
compared after every event (`crates/terminal/src/differential.rs`). Every
divergence below is pinned by a corpus `Waiver` naming its entry id — a
waiver that stops firing fails the run, so these adjudications cannot go
stale silently. Two comparator normalizations are pre-adjudicated by the
spec and implemented in the harness rather than entered here: `Indexed(0–15)`
compares equal to the same-named ANSI color (SPEC.md §9 accepted information
loss; Zed renders both through the identical theme slot), and OSC title
events compare through the `terminal.rs` glue state (`Title("")` and
`ResetTitle` both produce an empty breadcrumb).

## P7-001 — Tab characters occupy cells on alacritty, blanks on ghostty

- **Phase / change**: P7 (differential harness, ticket #50). Recorded by
  the harness; no code change.
- **Triggering input**: any `\t` reaching the emulator (corpus row
  `tab_stops`).
- **Old behavior**: alacritty's `put_tab` writes `'\t'` into the tab-origin
  cell; extraction (`bounds_to_string`) reproduces the tab and skips the
  fill cells up to the next tab stop (`"\ta"`).
- **New behavior**: ghostty advances the cursor over blank cells; the
  renderer sees spaces and extraction yields the equivalent spaces
  (`"        a"`, same visual width).
- **Adjudication**: **accepted**. Rendering is identical (a `'\t'` cell
  draws as blank); the delta is copy/extraction bytes of equal width, and
  ghostty the terminal ships this representation. Cursor-probe equality in
  the same row still pins tab-stop *positions* (HTS/TBC/CBT/CHT). Pinned by
  `tab_stops` waivers.

## P7-002 — Legacy alt-screen modes ?47/?1047 are implemented by ghostty only

- **Phase / change**: P7, as above.
- **Triggering input**: `CSI ? 47 h/l`, `CSI ? 1047 h/l` (corpus row
  `alt_screen_legacy_modes`).
- **Old behavior**: Zed's alacritty fork ignores both modes — writes land on
  the primary screen and no buffer swap happens.
- **New behavior**: ghostty performs the standard legacy buffer swap.
- **Adjudication**: **accepted** — a capability restoration; applications
  using the legacy modes get the behavior every other terminal gives them.
  The primary path Zed's ecosystem uses (?1049) is byte-identical
  (`alt_screen_enter_exit` runs clean). Pinned by
  `alt_screen_legacy_modes` waivers.

## P7-003 — Destructive clears rotate lines into scrollback on alacritty

- **Phase / change**: P7, as above.
- **Triggering input**: `CSI 2 J`, and `CSI Ps M` (DL) with the scroll
  region at the top of the screen (corpus rows `erase_operations`,
  `insert_delete_chars_lines`).
- **Old behavior**: alacritty implements these via grid rotation, pushing
  the cleared/deleted top lines into scrollback (`total_lines` grows; the
  pre-clear screen remains reachable by scrolling up).
- **New behavior**: ghostty erases/deletes in place (xterm semantics);
  nothing enters scrollback.
- **Adjudication**: **accepted**. Ghostty matches the xterm behavior;
  the user-visible delta is that a bare `ESC[2J` no longer leaves the old
  screen in history (the `clear` command is unaffected — it sends `3J`,
  and the G5 seam clear pins its own semantics). Watch during the P9 soak.
  Pinned by checked waivers (ghostty history never exceeds alacritty's;
  ghostty's extracted text is a suffix of alacritty's).

## P7-004 — IL/DL home the cursor column on ghostty

- **Phase / change**: P7, as above.
- **Triggering input**: `CSI Ps L` / `CSI Ps M` with the cursor mid-row
  (corpus row `scroll_region_decstbm`).
- **Old behavior**: alacritty leaves the cursor column untouched.
- **New behavior**: ghostty moves the cursor to column 1, per the DEC/xterm
  specification for IL/DL.
- **Adjudication**: **accepted** — ghostty is standard; the delta is one
  cursor-position frame until the application repositions (full-screen
  programs always do). Pinned by the `ghostty_homed_cursor_column` check
  (same line, column 0).

## P7-005 — Column-shrink reflow rotates overflow into scrollback on alacritty

- **Phase / change**: P7, as above.
- **Triggering input**: a column-shrinking resize while a wrapped line
  grows taller with rows below it (corpus row `reflow_shrink_and_grow`).
- **Old behavior**: alacritty anchors the cursor's screen row by rotating
  the extra wrapped rows into scrollback: content shifts up, history grows,
  the cursor keeps its viewport-relative row.
- **New behavior**: ghostty reflows in place: content keeps its position,
  history does not grow, the cursor rides its content line downward.
- **Adjudication**: **accepted**. Both are legitimate reflow strategies;
  the logical content is identical (the waiver check asserts full extracted
  text equality modulo trailing blank rows) and ghostty the terminal ships
  this behavior. Pinned by `reflow_shrink_and_grow` checked waivers.

## P7-006 — Mouse protocol/encoding mode flags are not mutually exclusive on ghostty

- **Phase / change**: P7, as above.
- **Triggering input**: setting more than one of modes 1000/1002/1003, or
  both 1005 and 1006 (corpus rows `modes_toggle_all`,
  `mouse_mode_exclusivity`).
- **Old behavior**: alacritty makes the three protocol modes (and the two
  encoding modes) mutually exclusive — setting one clears the others, so
  un-setting the active one leaves *no* mouse mode.
- **New behavior**: ghostty tracks each flag independently; the seam's
  `Modes` rebuild reports every set flag, and un-setting 1003 can reveal a
  still-set 1000.
- **Adjudication**: **accepted**. Zed's mouse policy takes the strongest
  set protocol and SGR over UTF-8, which matches alacritty's outcome for
  every escalating sequence real applications send; only a deliberate
  protocol *downgrade* without clearing (unobserved in the wild) differs.
  Pinned by the `ghostty_mouse_flags_superset` check (non-mouse bits
  identical, alacritty's bits a subset).

## P7-007 — ANSI-mode DECRQM goes unanswered by ghostty

- **Phase / change**: P7, as above.
- **Triggering input**: `CSI Ps $ p` (ANSI form, e.g. IRM `CSI 4 $ p`;
  corpus row `decrqm_ansi_mode_reports`). The private form
  (`CSI ? Ps $ p`) is byte-identical on both.
- **Old behavior**: alacritty reports, e.g. `CSI 4;2 $ y`.
- **New behavior**: ghostty sends no reply — the querying application sees
  the same silence an unsupporting terminal produces.
- **Adjudication**: **accepted** — degraded-but-standard: DECRQM clients
  must (and do) handle no-reply. Pinned by the
  `alacritty_extra_is_ansi_decrpm` check.

## P7-008 — Ghostty answers the kitty keyboard progressive-enhancement query

- **Phase / change**: P7, as above. The forced finding of the #32
  input-encoding resolution, now pinned differentially.
- **Triggering input**: `CSI ? u` (and push/pop `CSI > flags u`,
  `CSI < u`; corpus row `kitty_keyboard_query`).
- **Old behavior**: silence (the fork's kitty support is compile-time off).
- **New behavior**: ghostty's core answers (`CSI ? 0 u`, tracking pushed
  flags).
- **Adjudication**: **accepted** — SPEC.md §1 explicitly accepts kitty
  keyboard arriving via ghostty as an incidental capability gain; #32
  found legacy-only encoding was never parity-neutral. Pinned by the
  `ghostty_extra_is_kitty_report` check.

## P7-009 — Secondary DA firmware stamp matched to the alacritty era

- **Phase / change**: P7, as above; seam change in
  `crates/terminal/src/ghostty.rs` (`on_device_attributes`).
- **Triggering input**: `CSI > c` (vim sends it as t_RV on startup; corpus
  row `secondary_device_attributes`).
- **Old behavior**: alacritty answers `CSI > 0;2601;1 c`
  (`version_number("0.26.1")` of the pinned fork).
- **New behavior (before the fix)**: the dark backend's registration
  answered `CSI > 0;0;0 c`.
- **Adjudication**: **fixed in the seam** — the registration now stamps
  `0;2601;1`, byte-identical to today, so version-sniffing applications see
  no change at the swap. The corpus row also catches future fork pin bumps
  changing the stamp.

## P7-010 — `content_text` trailing blank rows

- **Phase / change**: P7, as above; seam change in
  `crates/terminal/src/ghostty.rs` (`content_text`).
- **Triggering input**: any `Terminal::get_content()` call — alacritty's
  `bounds_to_string` emits one newline per blank row below the last
  occupied row (minus the single trailing newline it strips), ghostty's
  formatter trimmed them all.
- **Adjudication**: **fixed in the seam** — the ghostty backend counts
  trailing blank rows (alacritty's `line_length` notion of occupancy) and
  re-appends the newlines, making the public extraction byte-identical.
  Pinned by every corpus row's full-probe `ContentText` comparison.

## P7-011 — BCE background lost from erased cells (dark-backend bug)

- **Phase / change**: P7, as above; seam fix in
  `crates/terminal/src/ghostty.rs` (`collect_viewport_cells`).
- **Triggering input**: EL/ED under an active SGR background — e.g. a tmux
  status bar (`recorded_tmux_split_scroll`; corpus row
  `bce_erase_with_background`).
- **Old behavior**: alacritty fills erased cells with the current
  background.
- **New behavior (before the fix)**: the snapshot read only the style
  layer, but ghostty stores an erased cell's background as cell *content*
  (`BgColorPalette`/`BgColorRgb` content tags) — the seam surfaced default
  backgrounds, visually dropping status-bar fills.
- **Adjudication**: **fixed in the seam** — the snapshot now reads the
  bg-color content tags. A P5 latent bug caught by the first recorded
  transcript; exactly the class the differential harness exists for.

## P7-012 — Ghostty answers the color-scheme report and XTVERSION

- **Phase / change**: P7, as above.
- **Triggering input**: `CSI ? 996 n` and `CSI > 0 q` (tmux probes both on
  startup; `recorded_tmux_split_scroll`).
- **Old behavior**: silence on both.
- **New behavior**: ghostty reports the color scheme (`CSI ? 997 ; s n`,
  answered from the seam's registered scheme) and XTVERSION
  (`DCS > | libghostty ST` — the core substitutes its own name; an empty
  callback return cannot suppress the reply).
- **Adjudication**: **accepted** — both are standard, capability-signaling
  replies; the color-scheme report is a wanted integration (theme-aware
  applications), and XTVERSION identifies the core truthfully. Pinned by
  the `ghostty_extra_is_ignored_query_response` check.

## P7-013 — Erased cells keep active SGR attribute flags on alacritty

- **Phase / change**: P7, as above.
- **Triggering input**: EL/ED while attribute SGRs (inverse, bold) are
  active — `top`'s header bars (`recorded_top_process_viewer`).
- **Old behavior**: alacritty stamps the full SGR template into the fill,
  so erased blanks carry INVERSE/BOLD flags (an inverse blank renders as a
  visible block).
- **New behavior**: ghostty erases attribute-free, carrying only the
  background color (xterm semantics).
- **Adjudication**: **accepted** — ghostty matches xterm; full-screen
  programs overdraw these cells immediately. Pinned by the
  `erased_cells_drop_attribute_flags` check (colors and text identical,
  ghostty flags empty).

## P7-014 — Cursor column after a column-grow resize with pending wrap

- **Phase / change**: P7, as above.
- **Triggering input**: a resize that widens the grid while the cursor sits
  in the pending-wrap state after filling a full row
  (`recorded_top_process_viewer`, 80→110 columns).
- **Old behavior**: alacritty un-wraps and places the cursor one past the
  old row end (column 80).
- **New behavior**: ghostty keeps it on the last written column (79).
- **Adjudication**: **accepted** — a one-column delta in a state the
  application always exits by repositioning; in the observed sessions the
  cursor is hidden throughout (the waiver check requires both cursors
  hidden). Revisit only if a visible-cursor variant surfaces.

## P7-015 — Alt-screen resize anchoring differs until the application redraws

- **Phase / change**: P7, as above.
- **Triggering input**: resizing while the alternate screen is active —
  vi's 100×30 → 70×20 shrink and tmux's 110×30 → 80×24 shrink
  (`recorded_vi_editing_session`, `recorded_tmux_split_scroll`).
- **Old behavior**: alacritty keeps the top rows (truncating from the
  bottom) on alt-screen row shrink.
- **New behavior**: ghostty anchors near the cursor/bottom, so a different
  row window survives the shrink.
- **Adjudication**: **accepted**. The divergent frames are transient: every
  alt-screen application redraws on SIGWINCH, and both backends converge
  on the very next output chunk (the recorded transcripts show exactly
  this). No steady state differs. Pinned by the recorded-session waivers,
  which the following redraw steps bound.

## P7-016 — Non-gating fuzz lane: outstanding finding classes

- **Phase / change**: P7, as above. The structured VT fuzzer
  (`differential::fuzz`, `#[ignore]`d, run by CI with continue-on-error)
  applies the checked adjudications above corpus-wide; what remains are
  open findings, recorded here per §3.3 so none is silently lost. At P7
  the lane reports findings in these classes:
  - combining-mark attachment (zerowidth placement after CJK, at row
    starts, and across wrap boundaries);
  - CSI parameter overflow clamping (e.g. `CSI 99999999999999999999 A`);
  - truncated/malformed CSI recovery (how many following bytes are
    consumed);
  - NUL and C0-in-sequence handling;
  - wide-char placement interactions after the above desynchronize the
    cursor.
- **Adjudication**: **open** — fuzzing is non-gating at P7 by decision
  (SPEC.md §7). Each confirmed real divergence gets promoted into the
  synthetic corpus and its own entry as it is triaged during the P8–P10
  window; the P10 gate requires zero unadjudicated fuzz findings.

## P8-001 — Encoder options honored live from terminal state

- **Phase / change**: P8 (the swap, ticket #51). The temporary
  Zed-`Modes`→encoder-options shim died with the swap; the key and mouse
  encoders now read their options off the live foreground-owned terminal via
  `set_options_from_terminal` at encode time (SPEC.md §4.3, §6 P8 row).
- **Triggering input**: any application toggling terminal state the shim
  could not see: the kitty keyboard progressive-enhancement query, DEC 1036
  (alt-sends-escape), or modifyOtherKeys.
- **Old behavior**: kitty flags structurally `DISABLED`, alt-esc prefix
  unconditionally on, modifyOtherKeys unconditionally off — the alacritty
  core neither tracked nor answered these, so the shim pinned them.
- **New behavior**: ghostty answers the kitty query internally and the
  encoder honors the negotiated flags (the seam's `legacy_fill` byte fills —
  F13–F20, ctrl-punctuation, ctrl-i/ctrl-m — are skipped while kitty flags
  are active, since distinguishing those keys is the protocol's purpose);
  DEC 1036 and modifyOtherKeys follow live terminal state.
- **Adjudication**: **accepted**. Exact-freshness encode-time sync is the
  locked design (SPEC.md §4.3); kitty keyboard arriving via ghostty's
  encoder is an accepted incidental capability gain (SPEC.md §1). With no
  application-driven state changes, output is byte-identical: the ported
  `keys.rs`/mouse contract suites pass with expectations untouched, their
  harness re-plumbed to drive a live terminal into the `Modes` states via
  the VT stream (`mappings::test_support::backend_with_modes`).

## P8-002 — SGR-Pixels reports carry cell-granular coordinates

- **Phase / change**: P8, as above. With mouse-encoder options read live,
  SGR-Pixels (DEC 1016) becomes reachable — the alacritty core never
  tracked it.
- **Triggering input**: an application enabling `CSI ?1016h` and observing
  mouse reports.
- **Old behavior**: mode 1016 ignored; reports stayed SGR cell-based.
- **New behavior**: reports use the SGR-Pixels wire format, but Zed's
  policy layer owns the grid math and feeds the encoder an identity 1×1
  cell map, so the "pixel" coordinates are cell numbers.
- **Adjudication**: **accepted**. Strictly more protocol-conformant than
  ignoring the mode; applications get valid SGR-Pixels framing at cell
  resolution. Revisit only if a real application needs sub-cell precision.

## P8-003 — Theme-change color answers refresh on the render sync

- **Phase / change**: P8, as above. Theme colors are pushed into ghostty as
  embedder defaults at terminal creation and re-pushed from `Terminal::sync`
  when the active theme changes (pointer-compared per frame); ghostty then
  answers OSC 4/10/11/12 internally (SPEC.md §4.2).
- **Triggering input**: an application querying e.g. `OSC 11 ; ? ST` in the
  window between a theme change and the terminal's next rendered frame.
- **Old behavior**: the alacritty `ColorRequest` event path read the live
  theme at request-processing time — always fresh.
- **New behavior**: the answer reflects the previous theme until the next
  `sync` pushes the new palette (at most one frame in a visible terminal;
  a never-rendered terminal answers from creation-time colors).
- **Adjudication**: **accepted**. The staleness window is one render frame
  and self-heals; ordering of answers within the PTY byte stream is
  strictly better than the event path (SPEC.md §3 D3). Display-only
  terminals without a theme at construction (headless hosts, unit tests)
  keep ghostty's built-in defaults — parity with the alacritty-era event
  path, which had no theme global to read in those contexts either.
