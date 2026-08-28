# Parity verification and acceptance strategy

Decision record for [ticket #37](https://github.com/xipeng-jin/zed/issues/37), **re-validated
for v2 on 2026-08-28** (grilling session; the v1 text of 2026-07-15 is the starting position —
§1–§8 are this ticket's, §9 was re-validated by ticket #40 on 2026-08-26). Defines the test
oracle, the new conformance tests, the interactive validation plan, and the explicit gate that
must be green before the Alacritty-removal phase may execute. This is the verification section
of the migration spec; SPEC v2 (ticket #38) assembles it. The v1 spec-lock amendment is kept at
the end.

Inputs: the [parity matrix](parity-matrix.md) (#29, v2), the
[PTY/threading architecture](pty-threading-architecture.md) (#30, v2), the
[input-encoding decision](input-encoding.md) (#32, v2), the
[color/clipboard contract](color-and-clipboard-contract.md) (#31, v2), the
[build strategy](build-strategy.md) (#28, v2), the [salvage policy](salvage-policy.md)
(#83), and v1's execution record on `migration/libghostty` @ `e537270dac` (tickets #50, #51).
Terms *unverified*, *verified*, *obsolete*, *retired*, *attribution A/B*, and *chunking
equivalence* are defined in `CONTEXT.md`.

## 0. What moved since v1 (re-validation table)

Baseline for this pass: `migration/libghostty2` (`crates/terminal` 102 tests, was 86), ghostty
`8867c37c5` (`include/ghostty/vt/{snapshot.h,terminal.h}`), libghostty-rs `de9fd9b0fa` vendored
as `crates/ghostty_vt{,_sys}`. None of v1's harness artifacts exist on v2 yet
(`crates/terminal/src/differential.rs`, seven `.ztrx` fixtures, `script/terminal-{record-transcript,perf-baseline,flood-bench,smoke-reel}`,
`.github/workflows/{terminal_differential,pty_integration,ghostty_source_build}.yml`); they are
class-3 salvage (salvage-policy.md rule 3), applied by the executor at the harness phase.

| v1 item | v2 disposition |
|---|---|
| Two-layer oracle (ported expectations + differential goldens) | **Re-confirmed verbatim** (§1). ghostty's `snapshot.h` goldens were considered and **rejected** as either a second or a replacement golden layer. |
| Comparison surface | **Extended** (§2) with the reads upstream Zed added since v1: grid shape, `used_lines`, cwd `scrollback_position`, the `GridLinesChange` verdict. |
| Corpus: 7 recorded ZTRX fixtures, 39 synthetic rows | **Carried as-is** (raw PTY bytes do not rot; ZTRX v1 format unchanged) **plus new recordings** for the v2-new consumers (§3.2). |
| Fuzz lane | **Carried non-gating**; P7-016 re-run with the APC class now attributable via `UNKNOWN_SEQUENCE` (§3.3). |
| Waiver mechanics (never-fires fails the run) | **Unchanged and applied to `unverified` entries**: the first v2 run turns each entry `verified` or `retired` (§3.4). No ledger reset. |
| Harness probes | **New** (§3.5): `UNKNOWN_SEQUENCE` on the harness's ghostty side (triage only), `VT_PROCESSING_ERROR == false` as a gating invariant, and a chunking-equivalence lane over `vt_write_until_ground`. |
| Class A/B/C split (86 tests) | **Redone for 102 tests** (§4.1): 18 new names (16 net) classified; one flagged. |
| Perf: five scenarios, 20 %, closed via seam cache + fork patch | **Scenarios and bar unchanged**; reference build restated for the upstream baseline pin (§6.2); the row-reuse cache carries as seam code; the fork patch is **not** applied unless the explicit trigger in §6.3 fires; attribution is by A/B, not by a core counter. |
| 12-row manual checklist, `:99` reel, two-week soak | **Unchanged**; checklist gains row 13 (cwd history at the scrollback cap). |
| Nine-point gate | **Re-stated** (§8) with the fork CI trigger branch moved to `migration/libghostty2`. |
| `snapshot` vt-feature (kept on by #28 pending this ticket) | **Dropped from the trimmed feature set**: `vt_features = "-kitty_graphics,-glyph_protocol,-snapshot"` (13.45 MB / 167 exports per build-strategy.md §0 row 4). Terminal persistence via `snapshot.h` is out of scope for the migration. |

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

**[v2] Goldens stay at the seam.** ghostty `8867c37c5` can serialize a whole terminal
(`ghostty_snapshot_encode*`, `include/ghostty/vt/snapshot.h`), and freezing those byte streams
per corpus transcript was considered as a post-removal net. Rejected: only ghostty can produce
one, so it is never a differential oracle between the cores; it would fail on every benign
internal or format change (the header states "format version 1 is a work in progress"), which
is exactly the allowlist churn §2 is designed to avoid; and Zed cannot observe what it would
catch. The golden is the seam `Content` + probe surface, nothing deeper.

## 2. Comparison surface

The differential harness compares exactly what Zed consumes — parity is defined as
*"Zed cannot tell the difference"*:

- the seam `Content` snapshot: per-cell char, `fg`/`bg` in Zed's `Color` model, style flags,
  wide-char spacers, hyperlink URI; cursor position + shape; `display_offset`; selection range;
- a fixed probe set: the mode flags Zed reads, window title, working directory (OSC 7),
  `total_lines` (scrollback), and the text-extraction outputs (`content_text`,
  `last_non_empty_lines`);
- **[v2]** the reads upstream Zed added since v1 (parity-matrix.md §0.1): the grid-shape
  quartet `columns`/`screen_lines`/`total_lines`/`display_offset` (B12), `used_lines` (B14),
  the cwd-history `scrollback_position` = `history + cursor line` (B15), and the
  `GridLinesChange` verdict computed for each event from the previous snapshot (B13 — derived,
  but a wrong derivation across resizes is precisely what the harness must catch). Zed-internal
  hover state (`HoveredWord.id`, `clear_hyperlink`) is not a probe.

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
- **[v2] Corpus policy.** v1's seven recorded fixtures
  (`crates/terminal/transcripts/recorded/*.ztrx`: vi, tmux, top, less, cargo, agent-tool shell,
  OSC 133/7 bash) and 39 synthetic rows carry unchanged by salvage import; raw PTY bytes are
  independent of the tool versions that produced them, and re-recording would only replace
  known coverage with unknown coverage. The ZTRX v1 format is unchanged. New recordings are
  added for the consumers Zed gained since v1: a shell session exercising cwd history through
  the scrollback cap (B15), and a key session covering the ctrl+alt / alt+shift encodings
  (P3). Synthetic rows are added for each parity-matrix §0.1 row and each §5 "new (expected)"
  ledger entry (DECRQSS, XTGETTCAP, in-band resize).

### 3.3 Fuzzing (non-gating)

A structured VT-sequence fuzzer (valid-ish and malformed streams) runs differentially as a
bounded, non-gating job. Every confirmed real divergence is promoted into the synthetic
corpus; malformed-input differences where the parsers legitimately disagree are
ledger-adjudicated. At gate time no fuzz finding may remain unadjudicated.

**[v2]** Carried non-gating (v1's `#[ignore]` lane + CI `continue-on-error`). Ledger P7-016's
four open classes (combining-mark attachment, CSI overflow clamping, truncated-CSI recovery,
NUL handling) are re-run at the harness phase; the APC class is now attributable through the
§3.5 `UNKNOWN_SEQUENCE` probe. Promotion to gating was considered and declined — gate point 5
already binds the *findings*, which is the part that matters.

### 3.4 Divergence ledger

`docs/ghostty-migration/divergence-ledger.md`. Every divergence — a differential mismatch or
a ported-test expectation change — gets a written entry: the triggering input, both
behaviors, the adjudication (**fixed** in the seam/core, or **accepted** with rationale).
Accepted entries are the only permitted deltas at gate time.

**[v2] Waiver mechanics for carried entries.** The ledger is salvaged, not reset: 42 entries,
41 `unverified` and one `obsolete` (P5-001). v1's rule stands unchanged — a divergence passes
the harness only through a `Waiver` naming its ledger id, with shape-pinning check functions
and optional step bounds, and **a waiver that never fires fails the run**. Applied to
`unverified` entries on the first v2 run this yields exactly one of three outcomes per entry,
recorded on its `v2 status` line:

- the waiver fires and its shape check passes → **`verified`** (re-adjudicated by the harness
  phase's executor, citing the run);
- the waiver never fires → the run fails; the executor confirms the divergence is gone at the
  pin (parity-matrix.md §5 predicts which) and marks the entry **`retired: no longer diverges
  at <pin>`**, deleting the waiver;
- the waiver fires but its shape check fails → a **new** entry (the old one stays for history,
  marked `superseded by <id>`), adjudicated as fixed or accepted.

The strictness is deliberate: "allowed to not fire on the first run" would make `verified`
mean nothing. Entries parity-matrix.md §5 lists as "not re-fire candidates" still go through
the same run; the list is a prediction, not an exemption.

### 3.5 Harness probes (v2)

Three ghostty-side aids, none of which is a comparison field (alacritty has no analogue):

1. **`GHOSTTY_TERMINAL_OPT_UNKNOWN_SEQUENCE`** (with `UNKNOWN_MAX_BYTES` set) is installed on
   the harness's ghostty backend only — never in the product seam
   (color-and-clipboard-contract.md §7). It is APC-only and synchronous; its output is attached
   to fuzz reports so the P7-016 APC class is triaged by content rather than by guess.
2. **`GHOSTTY_TERMINAL_DATA_VT_PROCESSING_ERROR`** is a sticky, un-resettable bool. The harness
   asserts it is **false after every gating transcript** (recorded and synthetic) — a core
   that silently hit an internal error on parity input is a failure even when the seam
   snapshot matches. For fuzz inputs it is reported, not asserted.
3. **Chunking-equivalence lane** over `ghostty_terminal_vt_write_until_ground` +
   `DATA_VT_GROUND`: each recorded fixture is replayed under three chunkings — 64 KiB
   pump-sized (the production shape), ground-aligned, and byte-at-a-time — and the final seam
   snapshot plus the PTY-response stream must be identical across them. This is the executable
   form of the PTY decision that batches need no split at parser ground
   (pty-threading-architecture.md). Gating, as synthetic rows.

## 4. Existing suite disposition

| Class | Rule | Suites |
|---|---|---|
| **A — port with expectations intact** | Rewritten against the seam/new core; expected values byte-identical; changes adjudicated via the ledger | `mappings/keys.rs` (**7**), `mappings/mouse.rs` (2), `alacritty/hyperlinks.rs` scenario tests (36), the semantic `alacritty.rs` round-trips (hyperlink storage, cell zerowidth) + `semantic_selection_stops_at_tree_branch` |
| **B — pass literally unchanged** | Above the seam; zero test edits allowed — proves the seam preserved its contract | `terminal.rs` (**51**), `pty_info.rs` (1), `terminal_element.rs`, `terminal_view.rs` (+3 gpui tests since v1), `terminal_panel.rs`, `terminal_path_like_target.rs`, agent terminal-tool integration tests |
| **C — retire with their subject** | Pure type-conversion tests whose subject is deleted with alacritty; replaced by equivalent assertions against ghostty mode state | `Modes`↔`TermMode` round-trip and similar conversion-only tests in `alacritty.rs` |

### 4.1 [v2] Disposition of the tests added since v1 (`crates/terminal`: 86 → 102)

Eighteen new names, two removed (`test_hyperlink_ctrl_click_{drag_within_bounds,same_position}`
were renamed to `test_ctrl_click_*`), sixteen net. `terminal.rs`'s test code is now split across
a `domain_tests` mod and the main `tests` mod (with `hyperlinks` and `perf` submodules); the
split changes nothing about class B — the rule is zero edits to any of it.

| # | Test | File | Class | Why |
|---|---|---|---|---|
| 1 | `semantic_selection_stops_at_tree_branch` | `alacritty.rs` | **A** | Builds an alacritty `Term` and pins the `─` semantic-escape addition (parity-matrix A7). Ports as a `select_word` boundary-codepoints test with the same expected selection. |
| 2 | `test_ctrl_alt_codes` | `mappings/keys.rs` | **A** | Encoder contract (parity-matrix P3). Input-encoding v2 pre-adjudicated the rows: ctrl+alt+letter native, ctrl+alt+i/m via the seam fill (ledger P3-002 extended) — any expectation change is already decided there. |
| 3–7 | `test_ctrl_click_drag_within_bounds`, `test_ctrl_click_same_position`, `test_ctrl_hover_with_changing_bounds`, `test_ctrl_hover_with_changing_content`, `test_ctrl_hover_with_modifier_change_only` | `terminal.rs` | **B** | Above the seam; drive VT through `write_output`; exercise the `GridLinesChange` arithmetic (B13) that search-and-hyperlinks.md ports verbatim. |
| 8–15 | `test_cwd_at_line_empty_history_returns_none`, `test_cwd_at_line_returns_cwd_for_line_at_or_after_recorded_position`, `test_cwd_at_line_returns_none_when_line_is_before_any_recorded_cwd`, `test_cwd_at_line_selects_most_recent_cwd_before_click`, `test_record_cwd_change_stores_entry_at_current_cursor_position`, `test_record_cwd_change_uses_command_boundary`, `test_remote_terminal_does_not_record_local_cwd`, `test_reset_cwd_history_discards_stale_coordinates` | `terminal.rs` | **B** | Zed-owned cwd-history logic over the `SCROLLBACK_ROWS`/`CURSOR_Y` reads (B15). |
| 16 | `test_cwd_at_line_ignores_history_at_scrollback_cap` | `terminal.rs` | **B — flagged** | Reads `scrolling_history` and encodes exact-line eviction; parity-matrix B15 says ghostty's page-granular pruning breaks that premise. It stays B: either it passes unchanged (the heuristic is Zed-side) or it is the first v2 ledger finding. Reclassifying before it fails would answer the harness's question for it. If `TrackedGridRef` anchoring lands post-swap, its subject changes and it moves to C then. |
| 17–18 | `test_terminal_shift_click_extends_existing_selection`, `test_terminal_shift_drag_selects_while_mouse_tracking` | `terminal.rs` | **B** | The drag test writes `last_content.mode` *and* drives `?1002h ?1006h` — the at-the-seam shape v1's swap flagged on three tests. Expected to pass with zero edits under live encode-time sync; any edit is a reviewer classification, per the v1 precedent. |

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

### 6.1 [v2] Scenarios and bar

The five v1 scenarios carry unchanged (`colored_dump`, `wide_char_cjk`, `alt_screen_churn`,
`sustained_scroll` from `script/terminal-perf-baseline`; `sustained_flood` from
`script/terminal-flood-bench`, landing at P4 per the spec-lock amendment), the 20 % bar and
the no-cliff/ReleaseFast conditions too. The `sustained_scroll` pure-scroll row-reuse cache
that closed v1's gate is seam code and carries by file (salvage-policy.md rule 3) together
with its cell-for-cell regression test; the scenario is measured **with** it — the uncached
shape has no production analogue (perf-baseline.md).

### 6.2 [v2] Reference build

Both backends in **one** release-mode invocation of each script, on the same machine:

- **alacritty**: the in-tree `alacritty_terminal` backend, re-recorded in that run (never the
  v1 numbers — perf-baseline.md is history, salvage-policy.md rule 4);
- **ghostty**: the **upstream baseline** prebuilt at the pin — release `ghostty-8867c37c55`
  built with `-Dcpu=baseline` and the trimmed `vt_features` (build-strategy.md §0 rows 3–5),
  attested and hash-pinned in `ghostty_pin.toml`; until the artifact pipeline publishes it,
  the pinned-source fallback built with the same `cpu`/`vt_features` is acceptable for a
  *provisional* reading, and the gate reading is repeated on the published archive.

Two columns decide the gate. No third column exists unless §6.3 fires.

### 6.3 [v2] Attribution and the fork-patch trigger

ghostty's C API exposes no style-set size or page-split counter, and v2 carries no fork
instrumentation. Attribution for a `colored_dump` failure is by **A/B**: the same run repeated
with v1's fork patch `636ce3a46f` (standard-page style capacity 128 → 512, branch
`zed/perf-style-capacity` on `xipeng-jin/ghostty`) applied to a source build at the pin. If the
patch alone brings the scenario inside the bar, style capacity is the cause.

The fork-patch trigger, stated once: the executor re-applies `636ce3a46f` on a fork branch,
re-pins `source_repo`/`commit`, and re-publishes **only if** `colored_dump` or
`sustained_scroll` fails the 20 % bar on the upstream baseline prebuilt **and** the A/B
attributes the failure to the patch's subject. If the A/B does **not** attribute it, the swap
is **held** for engineering — a perf failure is never adjudicated into the ledger (v1
precedent: ticket #51, owner decision of 2026-07-21). Upstreaming the patch remains a separate
decision.

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
| **[v2]** cwd history at the scrollback cap | in a session whose output exceeds `scrollback_lines`, Ctrl-click on a path above and below the last `cd` resolves against the right directory; once history is evicted past the cap the breadcrumb/hover falls back rather than resolving against a stale cwd |

The scripted reel (`script/terminal-smoke-reel`, v1's 13 frames: launch, terminal open,
colored output, OSC 2 title, vim alt-screen edit and restore, tmux split with BCE status bar,
5 000-line scrollback + history scroll + `clear`, resize narrow/back) carries by file.

## 8. The Alacritty-removal gate

The removal phase may execute only when every criterion below is green, verified as a
checklist in the removal-phase PR description:

1. **Class B suites pass literally unchanged** (zero test edits above the seam).
2. **Class A suites green with expectations intact**; every changed expectation adjudicated
   in the divergence ledger.
3. **Differential corpus** (recorded + synthetic, including the §3.5 chunking-equivalence
   rows and the `VT_PROCESSING_ERROR == false` invariant) runs divergence-free modulo
   ledger-accepted entries; **no ledger entry is still `unverified`** (§3.4: every carried
   entry is `verified`, `retired`, or superseded); goldens frozen and the golden-replay job
   wired into CI.
4. **Gap-fill component suites green**: vi mode (ported upstream tests), search engine
   (bar per §5 / ticket #35 — gate in numbers: ≤ 5 ms @ 10k rows, ≤ 50 ms @ 100k), PTY
   integration, point arithmetic.
5. **No unadjudicated fuzz findings** open (fuzzing itself stays non-gating in CI).
6. **Perf benchmarks** within 20% of alacritty measured in the same run against the
   upstream baseline prebuilt (§6.2), no cliff anomaly, ReleaseFast asserted on the shipped
   artifact; if the §6.3 trigger fired, the reading is on the re-published fork pin.
7. **Full manual checklist pass recorded** (§7 table, 13 rows) and the `:99` smoke reel
   reviewed.
8. **Two-week Linux soak**: ghostty backend as the daily-driver terminal on real work
   (including agent-tool and debugger sessions) with zero unresolved P0/P1 terminal issues
   filed in the window. Alacritty remains in-tree as the rollback path during the soak.
9. **macOS and Windows gates satisfied** (§9). Per the Linux-first decision these complete
   after the Linux subsystem swaps but before removal executes; the macOS and Linux soak
   windows may run concurrently, and the Windows gates may complete any time during them.

**[v2] CI.** The three fork workflows carry by file with their trigger branch moved to
`migration/libghostty2`: `terminal_differential.yml` (gating corpus + non-gating fuzz step),
`pty_integration.yml` (Linux binding; hosted Windows advisory per §9.2), and
`ghostty_source_build.yml` (build-strategy.md §0 row 7). Points 1–5 are read from these runs
on the removal PR's head commit.

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
