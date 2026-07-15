# Parity verification and acceptance strategy

Decision record for [ticket #37](https://github.com/xipeng-jin/zed/issues/37). Defines the
test oracle, the new conformance tests, the interactive validation plan, and the explicit
gate that must be green before the Alacritty-removal phase may execute. This becomes the
verification section of the final migration spec (ticket #38).

Inputs: the [parity matrix](parity-matrix.md) (#29), the
[PTY/threading architecture](pty-threading-architecture.md) (#30), the
[spike findings](spike-findings.md) (#34), and the input-encoding decision (#32).

---

## 1. Oracle principle

Parity is proven by two complementary layers:

1. **Ported expectations.** Existing suites are ported to the seam API with their expected
   values kept byte-identical. The expectations — authored against alacritty behavior — are
   the oracle. A ported test that needs its expectation changed is a **parity finding to
   adjudicate** (see the divergence ledger, §3.4), never a routine test fix.
2. **Differential goldens.** While both cores are in-tree, a differential harness feeds
   identical raw-byte transcripts to both and compares seam-level snapshots. Divergence-free
   runs freeze their snapshots as goldens; the golden-replay job outlives alacritty and
   becomes the permanent regression net.

## 2. Comparison surface

The differential harness compares exactly what Zed consumes — parity is defined as
*"Zed cannot tell the difference"*:

- the seam `Content` snapshot: per-cell char, `fg`/`bg` in Zed's `Color` model, style flags,
  wide-char spacers, hyperlink URI; cursor position + shape; `display_offset`; selection range;
- a fixed probe set: the mode flags Zed reads, window title, working directory (OSC 7),
  `total_lines` (scrollback), and the text-extraction outputs (`content_text`,
  `last_non_empty_lines`).

State neither core exposes through the seam (internal grid representation, dirty tracking,
tab stops) is invisible by construction — no benign-difference allowlist churn.

## 3. Differential harness

### 3.1 Transcript format

A transcript is an ordered event stream: `bytes(chunk)` and `resize(cols, rows)` events.
Resize events are first-class so reflow is covered by the same corpus. Transcripts are
checked in as binary fixtures alongside a small recorder script that captures raw PTY output
(with resizes) from a live session.

### 3.2 Corpus (gating)

- **Recorded real-session transcripts**: a vim/nvim editing session, tmux split/scroll,
  htop, less paging, colored cargo/clippy output, a Zed agent-tool run, a shell session with
  OSC 133/7 prompt marks. Catches interleavings hand-written sequences never produce.
- **Targeted synthetic sequences**, esctest/vttest-inspired and scoped to the parity-matrix
  rows: SGR permutations, cursor movement + scroll regions, wide chars/graphemes at row
  boundaries, WRAPLINE/reflow across resizes, scrollback fill + trim, every OSC Zed handles
  (0/2/7/8/52/133 and 4/10/11/12), the mode flags Zed reads, the kitty-keyboard query. Each
  sequence is named for the matrix row it verifies — the corpus is the parity matrix's
  executable form.

### 3.3 Fuzzing (non-gating)

A structured VT-sequence fuzzer (valid-ish and malformed streams) runs differentially as a
bounded, non-gating job. Every confirmed real divergence is promoted into the synthetic
corpus; malformed-input differences where the parsers legitimately disagree are
ledger-adjudicated. At gate time no fuzz finding may remain unadjudicated.

### 3.4 Divergence ledger

`docs/ghostty-migration/divergence-ledger.md`. Every divergence — a differential mismatch or
a ported-test expectation change — gets a written entry: the triggering input, both
behaviors, the adjudication (**fixed** in the seam/core, or **accepted** with rationale).
Accepted entries are the only permitted deltas at gate time.

## 4. Existing suite disposition

| Class | Rule | Suites |
|---|---|---|
| **A — port with expectations intact** | Rewritten against the seam/new core; expected values byte-identical; changes adjudicated via the ledger | `mappings/keys.rs` (6), `mappings/mouse.rs` (2), `alacritty/hyperlinks.rs` scenario tests (36), the semantic `alacritty.rs` round-trips (hyperlink storage, cell zerowidth) |
| **B — pass literally unchanged** | Above the seam; zero test edits allowed — proves the seam preserved its contract | `terminal.rs` (37), `terminal_element.rs` (19), `terminal_view.rs` (22), `terminal_panel.rs` (11), `terminal_path_like_target.rs` (15), agent terminal-tool integration tests |
| **C — retire with their subject** | Pure type-conversion tests whose subject is deleted with alacritty; replaced by equivalent assertions against ghostty mode state | `Modes`↔`TermMode` round-trip and similar conversion-only tests in `alacritty.rs` |

## 5. Gap-fill component tests

New Zed-owned code (the parity matrix's gap fills) is invisible to the differential harness;
each component seeds its suite from alacritty's upstream tests — the "expectations intact"
principle extended upstream:

- **Vi mode (G2)**: port alacritty's `vi_mode.rs` test suite, scoped to the 16 motions Zed
  dispatches.
- **Search engine (G1)**: must pass the ported `hyperlinks.rs` scenario corpus plus
  search-specific cases covering what alacritty's `search.rs` tests cover (wrapped lines,
  wide chars, scrollback boundaries). The exact engine shape is owned by ticket #35; this
  sets its acceptance bar.
- **PTY seam (G3, portable-pty)**: integration tests for spawn, resize, kill, and
  exit-status propagation.
- **Point arithmetic (G6)**: property-style add/sub/clamp tests including wide-char
  expansion.

## 6. Performance

A reproducible benchmark script reusing the harness's transcript-feeding machinery, run
headless through the seam on **release** builds:

- large colored output dump;
- sustained scrolling with full scrollback;
- alt-screen app churn;
- wide-char/CJK-heavy output.

Baseline numbers are recorded against alacritty before the swap. Gate: no scenario regresses
more than **20%** vs baseline, and no cliff-shaped anomaly. Additionally the shipped native
core must assert **ReleaseFast** (closing the spike's 3000× zig-Debug trap permanently).

## 7. Interactive validation

**Scripted drive, human judgment.** The nested-Xwayland `:99` harness gets a smoke script:
launch dev Zed, open a terminal, run a fixed scenario reel (vim edit, tmux split, colored
output, resize, task terminal, title change), capturing a screenshot per step into an
artifact directory. Automation drives; a human reviews the reel. No pixel-diff assertions.

**Manual checklist** — rows carry expected observations, not vibes. Cadence: after each
phase that swaps a subsystem, the affected rows; before the removal phase, one complete pass
with results recorded.

| Row | Expected observation |
|---|---|
| tmux | split panes render independently; scroll in one pane doesn't disturb the other; status bar updates |
| vim/nvim | syntax colors correct; alt-screen enter/exit restores shell scrollback; resize reflows without artifacts |
| Task terminals | task output streams with colors; terminal reused/cleaned per task settings |
| Agent terminal tool | agent-run commands stream output; `last_n_non_empty_lines` summaries match visible output |
| Debugger console | ANSI colors in console output render with theme-correct colors |
| Copy/paste | selection copies exact text incl. wide chars; paste into vim inserts without executing (bracketed paste); multiline paste into shell behaves per settings |
| OSC title/cwd | tab title follows OSC 0/2; title bar/breadcrumb shows `$CWD` after `cd` (OSC 7) |
| Resize/reflow | wrapped lines re-wrap on width change; no lost content top-of-screen |
| Scrollback | scroll to top of a long output reaches configured history; `clear` behaves as before |
| Find-in-terminal | search finds matches across wrapped lines and scrollback; highlights track scroll |
| Hyperlinks | URL/path hover underlines correct extent (incl. wide chars); Ctrl-click opens; OSC 8 links honored |
| Vi mode | the 16 dispatched motions move/select as before |

## 8. The Alacritty-removal gate

The removal phase may execute only when every criterion below is green, verified as a
checklist in the removal-phase PR description:

1. **Class B suites pass literally unchanged** (zero test edits above the seam).
2. **Class A suites green with expectations intact**; every changed expectation adjudicated
   in the divergence ledger.
3. **Differential corpus** (recorded + synthetic) runs divergence-free modulo
   ledger-accepted entries; goldens frozen and the golden-replay job wired into CI.
4. **Gap-fill component suites green**: vi mode (ported upstream tests), search engine
   (bar per §5 / ticket #35), PTY integration, point arithmetic.
5. **No unadjudicated fuzz findings** open (fuzzing itself stays non-gating in CI).
6. **Perf benchmarks** within 20% of the alacritty baseline, no cliff anomaly, ReleaseFast
   asserted on the shipped artifact.
7. **Full manual checklist pass recorded** (§7 table) and the `:99` smoke reel reviewed.
8. **Two-week Linux soak**: ghostty backend as the daily-driver terminal on real work
   (including agent-tool and debugger sessions) with zero unresolved P0/P1 terminal issues
   filed in the window. Alacritty remains in-tree as the rollback path during the soak.
9. **macOS and Windows gates satisfied** (§9). Per the Linux-first decision these complete
   after the Linux subsystem swaps but before removal executes; the macOS and Linux soak
   windows may run concurrently, and the Windows gates may complete any time during them.

## 9. Platform gates (macOS, Windows)

Decision record for [ticket #40](https://github.com/xipeng-jin/zed/issues/40). The bar is
asymmetric by decision: **macOS runs the full Linux-grade program on real macOS hardware;
Windows runs a reduced bar scoped to the PTY layer**, where all Windows-specific risk lives
(the vt core is platform-independent and the Linux differential corpus vouches for it).

### 9.1 macOS gate — full program

Every §8 criterion 1–8 re-run on a dedicated macOS machine, with these platform bindings:

- **Perf baseline is macOS-local**: the alacritty baseline (§6) is re-recorded on the same
  macOS machine before comparison; the 20%/no-cliff bar is unchanged. The §6 suite gains a
  **sustained-flood scenario** (`yes`/`cat` of a large file, measuring total throughput and
  input-echo latency during the flood) targeting the macOS ~1 KiB master-read cap; it also
  runs on Linux, where it doubles as the channel/batch-tuning validation from the
  architecture note's open question 6. Remediation for a failure is the reserved tuning
  path (channel capacity, batch cap, dedicated-terminal-thread escape hatch), not redesign.
- **`/usr/bin/login` wrapper, ghostty behavior**: portable-pty does not wrap the shell in
  `login` (alacritty_terminal does today for `Shell::System`), so Zed's macOS spawn layer
  owns the wrapper — ported from **ghostty's** exec-layer behavior, not alacritty's, as a
  deliberate upstream-alignment decision. This is a user-visible divergence from
  alacritty-today and gets a divergence-ledger entry (§3.4). Verified two ways: a unit test
  asserting the constructed argv against fixtures captured from ghostty's invocation, and
  three manual smoke items on the macOS machine — `[[ -o login ]]` reports a login shell,
  `PATH` reflects `path_helper` ordering (`/etc/zprofile` ran), and the session appears in
  `who` (utmpx record written).
- **Session-based soak**: two-week window on the macOS machine, ≥3 logged real development
  sessions per week (builds, agent-tool runs, debugger use, long build floods), zero
  unresolved P0/P1 terminal issues at exit, alacritty in-tree as rollback throughout. May
  run concurrently with the Linux soak.

### 9.2 Windows gate — reduced bar

1. **Windows CI suites green** on the existing `self-32vcpu-windows-2022` runner, including
   the Class A/B dispositions of §4.
2. **ConPTY PTY integration suite** (extends §5's PTY seam tests; runs on the Windows CI
   runner) covering the two §7 hazards from the architecture note — ConPTY delivers EOF only
   after the pseudoconsole is dropped, and child-exit observation must not depend on
   reader-EOF ordering:
   - *Shutdown*: spawn `cmd.exe` through the seam, close the terminal; assert the child
     terminated, the reader thread joined within a timeout (no EOF-wait hang), and no
     orphaned `OpenConsole.exe`/conhost processes remain.
   - *Exit observation*: spawn a command exiting with a known code; assert the exit is
     observed with the right code while the master is still open. Mechanism decision:
     **`try_wait` polling on the existing `pty_info` cadence** (no new thread, matches the
     current alacritty `child_watcher` approach); `WaitForSingleObject` on
     `as_raw_handle()` is the documented fallback if polling latency ever bites.
   - *Kill path*: kill a long-running child; assert termination and reader-thread join.
3. **`ChildKiller::kill()` fix sourced by git pin**: portable-pty is pinned as a git
   dependency to the wezterm-repo rev containing wezterm#7709 (merged 2026-06-07, absent
   from the 0.9.0 crates.io release), matching the existing `alacritty_terminal` git-pin
   precedent. Returning to a crates.io release once one ships is a post-removal follow-up,
   not a gate.
4. **Manual smoke checklist on real Windows hardware**: open the terminal in PowerShell and
   `cmd`, run a task, resize, paste multi-line text, kill a hung process, close the terminal
   while a child is running.

### 9.3 Blocking classification

All §9.1 and §9.2 items **block the removal phase**. Explicitly riding after removal:
migrating portable-pty from the git pin back to a crates.io release, and any channel/batch
tuning beyond the perf bar (including activating the dedicated-terminal-thread escape
hatch), which happens only if a real regression appears.
