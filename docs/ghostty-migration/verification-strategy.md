> [!NOTE]
> **v2 status (2026-08-25):** v1 decision record, salvaged as the v2 starting text (salvage-policy.md rule 1). **Not locked for v2**: the re-opened ticket amends this file in place and removes this banner on resolution. Source: `migration/libghostty` @ `e537270dac`.

# Parity verification and acceptance strategy

Decision record for [ticket #37](https://github.com/xipeng-jin/zed/issues/37). Defines the
test oracle, the new conformance tests, the interactive validation plan, and the explicit
gate that must be green before the Alacritty-removal phase may execute. This is the
verification section of the final migration spec, now assembled and locked as
[SPEC.md](SPEC.md) (ticket #38), which is the execution authority; see the
[Amendment](#amendment-spec-lock-2026-07-16) at the end for the one detail the spec lock
changed.

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

Decision record for [ticket #40](https://github.com/xipeng-jin/zed/issues/40), **re-validated
for v2 on 2026-08-26** (grilling session; the v1 text of 2026-07-15 plus the §8.2 amendment of
2026-07-21 is the starting position). The bar is asymmetric by decision: **macOS runs the full
Linux-grade program on real macOS hardware; Windows runs a reduced bar scoped to the PTY
layer**, where all Windows-specific risk lives (the vt core is platform-independent and the
Linux differential corpus vouches for it). Terms *platform gate*, *advisory*, and *binding
evidence* are defined in `CONTEXT.md`.

Sources for the v2 pass: ghostty `8867c37c5` (`src/build/GhosttyLibVt.zig:250-268`,
`:340-420`, `src/build/LibsystemOverrideStep.zig`, commits `1fe1b2d23`, `84254a9d8`,
`d65cb5128`); libghostty-rs `de9fd9b0fa` (`build.rs`, commits `bac73b9`, `1f135a3`); this
fork's `.github/workflows/*` and `script/bundle-windows.ps1`; `Cargo.toml:741`
(`portable-pty = "0.9.0"`); ticket #45's forensic record and remaining-work checklist;
[pty-threading-architecture.md §3.7](pty-threading-architecture.md) and
[artifact-pipeline.md §3.1](artifact-pipeline.md).

### 9.0 What moved since v1 (re-validation table)

| v1 item | v2 disposition |
|---|---|
| Asymmetric bar (macOS full, Windows PTY-scoped) | **Re-confirmed.** |
| §8.2 amendment (hosted Windows runners advisory) | **Folded in from the start** (§9.2), with the substrate owner stated: the fork cannot run `self-32vcpu-windows-2022` (label resolves only in upstream's org — every Windows job on the fork uses it, so they never run here), so the self-hosted CI half is an **upstreaming-time** criterion and the fork's gate discharges on real-hardware evidence (§9.2.1). |
| Deferred: msvc vs gnullvm | **Closed: msvc.** Zed's Windows bundles are `*-pc-windows-msvc` (`bundle-windows.ps1:47`); gnullvm would be a whole-Zed toolchain change, not a terminal decision, and the upstream fixes target msvc-static specifically. |
| Deferred: `ghostty-vt-static.lib` naming | **Resolved upstream.** ghostty names the static archive `ghostty-vt-static.lib` (avoids the DLL import-lib collision); libghostty-rs `bac73b9` links `static=ghostty-vt-static` on `windows-msvc`. |
| Deferred: ubsan-rt under MSVC | **Resolved upstream, asymmetry recorded.** `GhosttyLibVt.zig:250-268`: `bundle_ubsan_rt = false` on all Windows targets (LNK4229 `/exclude-symbols`), `stack_protector = false` on msvc static (no `BufferOverflowU` for consumers), `_fltused` exported (`lib_vt.zig`), `ntdll`+`kernel32` linked. Consequence: the Windows archive carries no ubsan runtime and no stack cookies while Linux/macOS archives bundle ubsan-rt — a debug-aid asymmetry, not a production defence, accepted. The only gate residue is that the artifact link smoke must be **warning-free** (§9.2.2) so an upstream regression of these fixes is caught. |
| Windows aarch64 | **In scope of the Windows gate, artifact-only** (§9.2.2). The fork bundles `aarch64-pc-windows-msvc` (`run_bundling.yml` `bundle_windows_aarch64`, cross-target on the x86_64 runner); P10 deletes alacritty, so every bundled target needs a core. libghostty-rs `1f135a3` removed only its `windows-11-arm` CI job; `zig_target()` still maps the triple and the upstream fixes key on `abi == .msvc`, not arch. Runtime validation on ARM64 Windows is **not** required (no hardware) — recorded as a known gap. Supersedes artifact-pipeline.md §3.1's "x86_64 MSVC only". |
| macOS program vs Apple linker changes | **Artifact only; program unchanged** (§9.1). Apple `ld` (`d65cb5128`, `initLibApple`) links only the **dylib**, which v2 never uses; `LibsystemOverrideStep` post-processes the **static archive** (memcpy/memmove/memset/libm rebound to libSystem) and needs Apple's `nmedit`, so a non-Darwin build silently ships the slower variant. `osVersionMinLibVt` changes only the iOS floor. |
| portable-pty git pin | **Re-confirmed mandatory.** Still `0.9.0` at `Cargo.toml:741`; the inverted `kill()` is verified in [pty-threading-architecture.md §3.2](pty-threading-architecture.md). |
| #45 remaining-work checklist | **Absorbed verbatim into §9.2.1** so the gate record is self-contained; #45 stays a frozen forensic record (map "Out of scope"). |
| Blocking classification | **Both gates still block P10**, with the Windows CI half split out as upstreaming-time (§9.3). |

### 9.1 macOS gate — full program

Every §8 criterion 1–8 re-run on a dedicated macOS machine, with these platform bindings:

- **Perf baseline is macOS-local**: the alacritty baseline (§6) is re-recorded on the same
  macOS machine before comparison; the 20%/no-cliff bar is unchanged. The sustained-flood
  scenario (lands at P4 per the spec-lock amendment below; targets the macOS ~1 KiB
  master-read cap, total throughput + input-echo latency during the flood) is **re-recorded**
  on the macOS machine. Remediation for a failure is the reserved tuning path (channel
  capacity, batch cap, dedicated-terminal-thread escape hatch), not redesign.
- **[v2] Artifact under test is the Darwin-built static archive.** v2 links
  `libghostty-vt-static.a`, never the dylib, so the Apple-`ld` change is irrelevant; but the
  archive must have passed `LibsystemOverrideStep` (built on a Darwin runner, per
  artifact-pipeline.md §3.1). A cross-built archive binds memcpy/libm to bundled compiler-rt
  and is **disqualified as gate evidence** — the perf baseline and the soak run only on
  override-applied artifacts. No variant comparison is part of the program.
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
- **CI**: macOS Class A/B suites green on the fork's existing macOS CI
  (`namespace-profile-mac-large`, `run_tests.yml`).

### 9.2 Windows gate — reduced bar

Target ABI is **msvc** (closed, §9.0). Evidence is split by who can produce it:

#### 9.2.1 Binding on the fork (blocks P10)

1. **Real-hardware manual smoke** on a real Windows machine, dev build (absorbed from #45):
   - spawn the default shell (pwsh) and `cmd`, echo round-trip, typing-latency sanity;
   - resize — the child's reported console size follows the pane;
   - run a one-shot task and confirm the exit status surfaces;
   - paste multi-line text;
   - **P4-001 probe**: a `ShellKind::Cmd` task with spaced/quoted args (e.g.
     `cmd /C echo "hello world"`) — portable-pty MSVC-quotes unconditionally and cmd.exe does
     not parse backslash-escaped quotes; documented exit if it bites: raw-cmdline patch in the
     portable-pty fork path;
   - kill a hung process;
   - close a terminal while the child is mid-output — verifies the detached
     `ClosePseudoConsole` closer thread: no UI freeze, no orphaned conhost or child;
   - quit Zed with live terminals open — no orphaned processes.
2. **ConPTY PTY integration suite run locally on that hardware** (`cargo test -p terminal
   --lib` on the Windows box; output attached to the gate record). The three tests extend §5's
   PTY seam tests and cover the two §7 hazards from the architecture note — ConPTY delivers
   EOF only after the pseudoconsole is dropped, and child-exit observation must not depend on
   reader-EOF ordering:
   - *Shutdown*: spawn `cmd.exe` through the seam, close the terminal; assert the child
     terminated, the reader thread joined within a timeout (no EOF-wait hang), and no
     orphaned `OpenConsole.exe`/conhost processes remain.
   - *Exit observation*: spawn a command exiting with a known code; assert the exit is
     observed with the right code while the master is still open. Mechanism: **`try_wait`
     polling** every 100 ms on a background task (as landed in v1 P4;
     pty-threading-architecture.md §3.7); `WaitForSingleObject` on `as_raw_handle()` is the
     documented fallback if polling latency ever bites.
   - *Kill path*: kill a long-running child; assert termination and reader-thread join.
   Windows Class A/B dispositions (§4) ride along on the same local run.
3. **`ChildKiller::kill()` fix sourced by git pin**: portable-pty pinned to wezterm
   `8afe0ad307` (wezterm#7709, absent from 0.9.0), matching the `alacritty_terminal` git-pin
   precedent. Returning to a crates.io release once one ships is a post-removal follow-up.
4. **Re-adjudicate ledger P4-001** (cmd.exe quoting) from the probe's outcome.

#### 9.2.2 Binding on the fork, artifact-side (blocks P10)

5. **Artifact matrix widened to `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc`**,
   both cross-compiled from the Linux runner (no host-only post-processing on Windows), inner
   file `ghostty-vt-static.lib`. Smoke is **link-only** and must be **warning-free** (no
   LNK4229, no unresolved `BufferOverflowU`/`_fltused`/`Nt*` symbols) — this is the
   verification that upstream's msvc-static fixes still hold. aarch64 gets no runtime
   evidence (known gap: no ARM64 Windows hardware); it is in the matrix because the fork
   bundles it and P10 removes the fallback core.

#### 9.2.3 Binding at upstreaming (does not block P10 on the fork)

6. **Windows CI green on upstream's self-hosted runners** (`self-32vcpu-windows-2022`):
   Class A/B + the three-test ConPTY suite, i.e. v1's `pty_integration.yml` Windows job
   re-pointed at that label with `continue-on-error` dropped, carrying the two mechanical
   fixes (quoted `pty::` filter; `core.longpaths` + `CARGO_NET_GIT_FETCH_WITH_CLI`).

**Advisory, never binding**: the fork's GitHub-hosted Windows job (`continue-on-error:
true`). Hosted `windows-latest` starves ConPTY below the seam in bare portable-pty (#45,
run 29603970998, no-GPUI control) and proves only compile/clippy and the
`TerminateProcess → try_wait` path (pty-threading-architecture.md §3.7). Non-binding
diagnostics if anyone rehabilitates hosted runners: the `windows-2025` image and
portable-pty's case-sorted environment block under `CREATE_UNICODE_ENVIRONMENT`.

### 9.3 Blocking classification

**Both gates block P10.** On the fork that means: all of §9.1, and §9.2.1 + §9.2.2 — alacritty
is the rollback, and removing it before any Windows runtime evidence exists would leave
Windows users with no fallback. §9.2.3 (self-hosted Windows CI) binds at upstreaming, not at
P10; it is the honest statement of what the fork can produce, not a relaxation of the bar.
The two soaks may overlap and the Windows items may complete during them.

Explicitly riding after removal: migrating portable-pty from the git pin back to a crates.io
release; any channel/batch tuning beyond the perf bar (including activating the
dedicated-terminal-thread escape hatch), which happens only if a real regression appears;
runtime validation on ARM64 Windows if hardware ever becomes available.

## Amendment (spec lock, 2026-07-16)

Made while assembling and locking [SPEC.md](SPEC.md) (ticket #38), the execution
authority: **the sustained-flood scenario lands at phase P4**, not at the macOS gate.
§9.1's "the §6 suite gains a sustained-flood scenario" conflicted with the phase plan,
whose P4 acceptance criteria already require that benchmark to validate channel/batch
tuning — an acceptance criterion needs its instrument when the phase lands. The macOS gate
**re-records** the scenario on macOS hardware (unchanged bar); nothing else in §9.1
changes. Relatedly, the spike's suggested time-budgeted drain is formally superseded by
the per-turn batch cap + this benchmark + the dedicated-thread escape hatch (SPEC.md §3).
