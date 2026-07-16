# Migration spec: `alacritty_terminal` → libghostty-vt

**Status: LOCKED** — signed off 2026-07-16 (wayfinder ticket
[#38](https://github.com/xipeng-jin/zed/issues/38); map
[#27](https://github.com/xipeng-jin/zed/issues/27))

This is the single execution-ready specification for replacing
`alacritty_terminal` with a vendored libghostty-vt core in Zed's built-in
terminal. It folds every resolved wayfinder decision into one document: an
implementer needs only this spec, the decision records it links (all in this
directory), and the vendored crates to execute the migration without
reopening any decision.

Each section states the operative decisions at execution level and links the
decision record that holds the full rationale, alternatives, and code
citations. Where the spec-lock session superseded a record's detail, this
spec states the final form, names the supersession inline, and the affected
record carries a matching amendment — **the spec is the single execution
authority; on any residual conflict, it wins**.

| Area | Decision record | Ticket |
|---|---|---|
| Native build strategy | [build-strategy.md](build-strategy.md) | [#28](https://github.com/xipeng-jin/zed/issues/28) |
| Parity gap analysis | [parity-matrix.md](parity-matrix.md) | [#29](https://github.com/xipeng-jin/zed/issues/29) |
| PTY / threading | [pty-threading-architecture.md](pty-threading-architecture.md) | [#30](https://github.com/xipeng-jin/zed/issues/30) |
| Color / parser contract | resolution comment on the ticket | [#31](https://github.com/xipeng-jin/zed/issues/31) |
| Input encoding | resolution comment on the ticket | [#32](https://github.com/xipeng-jin/zed/issues/32) |
| Vendored bindings | commit `55f56d263e` | [#33](https://github.com/xipeng-jin/zed/issues/33) |
| Spike findings | [spike-findings.md](spike-findings.md) | [#34](https://github.com/xipeng-jin/zed/issues/34) |
| Search / hyperlinks | [search-and-hyperlinks.md](search-and-hyperlinks.md) | [#35](https://github.com/xipeng-jin/zed/issues/35) |
| Seam + phase plan | [seam-and-phase-plan.md](seam-and-phase-plan.md) | [#36](https://github.com/xipeng-jin/zed/issues/36) |
| Verification strategy | [verification-strategy.md](verification-strategy.md) | [#37](https://github.com/xipeng-jin/zed/issues/37) |
| Artifact pipeline | [artifact-pipeline.md](artifact-pipeline.md) | [#39](https://github.com/xipeng-jin/zed/issues/39) |
| Platform gates | [verification-strategy.md §9](verification-strategy.md#9-platform-gates-macos-windows) | [#40](https://github.com/xipeng-jin/zed/issues/40) |

---

## 1. Objective and scope

Replace `alacritty_terminal` (Zed fork, git pin) with a vendored
libghostty-vt core in `crates/terminal`, incrementally: seam-first,
subsystem-by-subsystem, the build green after every phase, Alacritty dropped
as the final phase. Linux ships first; macOS and Windows have explicit gates
(§8) that block the final removal.

**Execution context (declared at spec lock)**: execution happens on this
fork (`xipeng-jin/zed`), on the `migration/libghostty` branch, phase by
phase. The P9 soak means the developer daily-driving fork builds — no wider
user population is exposed at any phase. Upstreaming (a PR series to
`zed-industries/zed`, release-channel exposure policy, the prebuilt-repo
transfer per [artifact-pipeline.md §9](artifact-pipeline.md)) is an explicit
post-migration effort outside this spec.

**In scope**: byte-identical (or ledger-adjudicated) behavioral parity for
everything Zed's terminal does today, on the same public `terminal::` API —
downstream crates compile unmodified.

**Out of scope** (fresh efforts after the migration lands): kitty graphics
rendering, sixel, kitty keyboard as a user-facing feature, scrollback
compression UX, an `is_safe`-driven unsafe-paste confirmation dialog.
Incidental capability gains (e.g. kitty keyboard arriving via ghostty's
encoder) are accepted, not pursued.

Fact base: `alacritty_terminal` is a dependency of exactly one crate
(`crates/terminal`); exactly two files reference it (`src/alacritty.rs`,
`src/alacritty/hyperlinks.rs`). Everything downstream compiles against
Zed-owned types on `terminal::` re-export paths. The parity matrix's 93 rows
resolve to 65 covered, 11 partial (seam translation), and 8 logical gaps
(G1–G8), every one of which has a locked fill strategy below.

## 2. Native library: vendoring, build, artifacts

### 2.1 Vendored crates (done — commit `55f56d263e`)

`crates/ghostty_vt_sys` + `crates/ghostty_vt`: a one-time fork of
libghostty-rs @ `51bf4bf732`, fully Zed-owned, manual upstream pulls. License
is MIT (upstream's LICENSE text governs over its `MIT OR Apache-2.0`
metadata). Bindings are pre-generated; the `bindgen-tool` feature's
`gen-bindings` bin restamps a `headers_sha256` guard in `ghostty_pin.toml`
tying bindings to the native pin. A pin bump without regenerated bindings (or
vice versa) is un-mergeable.

### 2.2 Build contract

Prebuilt-by-default hybrid ([build-strategy.md](build-strategy.md) §6):

1. `GHOSTTY_VT_LIB_DIR` → link a local `libghostty-vt.a` directly (nix,
   packagers, offline).
2. `GHOSTTY_SOURCE_DIR` → zig build from a local ghostty checkout (ghostty
   dev loop; unpinned, warns).
3. `GHOSTTY_VT_FROM_SOURCE=1` → git-fetch the pinned commit + zig build
   (escape hatch; what the artifact workflow runs).
4. Default → download the Zed-published, sha256-pinned prebuilt from the
   `prebuilt_repo` release, verify, link. No zig, no git, no bindgen.

One pin file, `crates/ghostty_vt_sys/ghostty_pin.toml` (commit, release tag,
`prebuilt_repo`, per-target sha256s, `headers_sha256`) — bumped atomically
with the vendored bindings in one PR. Source paths require zig ≥ 0.15.2,
< 0.16, version-checked by `build.rs` with actionable messages
(build-strategy §6.4). **All paths default to ReleaseFast regardless of cargo
profile** — the spike measured zig-Debug cores degrading `vt_write` ~3000×
with non-empty scrollback; `LIBGHOSTTY_VT_SYS_OPTIMIZE` remains the explicit
override. A dedicated CI job builds from source at the pin on every PR so the
escape hatch cannot rot. `pkg-config` and `link-dynamic` features are dropped.

### 2.3 Artifact pipeline

Dedicated repo `xipeng-jin/libghostty-vt-prebuilt` (coordinate lives in
`ghostty_pin.toml`; fork→upstream handoff = GitHub repo transfer, preserving
URLs and hashes). `workflow_dispatch`-only workflow on one x86_64 runner
cross-builds the day-one matrix (`{x86_64,aarch64}-unknown-linux-{gnu,musl}`)
via direct zig + `nm`/link smoke checks; releases are append-only immutable
`ghostty-<commit10>` tags with `libghostty-vt-<commit10>-<triple>.tar.gz`
(`.a` + LICENSE) assets, indefinite retention. GitHub artifact attestation
on; minisign rejected; the in-tree sha256 pins remain the sole build-time
gate. Nix consumes the prebuilt via a fixed-output `fetchurl` wired as
`GHOSTTY_VT_LIB_DIR`. No Debug and no `-Dsimd=false` variants. Licensing via
a static `script/licenses/ghostty-vt-LICENSES` section covering ghostty, the
bindings authors, and bundled simdutf/highway/uucode/compiler-rt.

Implementation order: [artifact-pipeline.md §10](artifact-pipeline.md)
(create repo + workflow → dispatch at the pin → stamp hashes →
`fetch_prebuilt` in `build.rs` + delete the interim empty-table source
fallback → nix derivation → licenses section). This is the **P0 track** in
the phase plan (§6): it has no dependency on P1–P2 and must complete before
P3, which takes the first production dependency on `ghostty_vt`.

## 3. Runtime architecture

Decision record: [pty-threading-architecture.md](pty-threading-architecture.md);
refined by [spike-findings.md](spike-findings.md) and
[seam-and-phase-plan.md](seam-and-phase-plan.md).

- **PTY layer (D1)**: `portable-pty` behind a thin Zed-owned seam module
  (`crates/terminal/src/pty.rs`, shared by both backends). **Git-pinned at
  the wezterm#7709 rev** (Windows `ChildKiller::kill()` fix, absent from
  0.9.0); returning to a crates.io release is a post-removal rider.
  portable-pty's `pre_exec` clears the signal mask, subsuming Zed's
  `SignalMask` fork fix (zed#42234). The reader thread reaps on EOF and emits
  `ChildExit` — no SIGCHLD machinery.
- **Ownership (D2)**: the `!Send` ghostty `Terminal` + `RenderState` are
  owned by the GPUI foreground thread as plain fields of the `Terminal`
  entity. No mutex, no `unsafe`. Reader/writer `std::thread`s move raw bytes
  over a bounded(4) × 64 KiB channel (kernel-queue backpressure, ghostty's
  own numbers); the foreground pump `vt_write`s a bounded number of batches
  per turn, then yields. The dedicated-terminal-thread escape hatch is
  preserved by the channel seams and activates only if a real regression
  appears post-removal. **Supersession (spec lock)**: the spike's suggested
  time-budgeted drain is superseded by the per-turn batch cap + the
  sustained-flood benchmark (§7) — tuning knobs, not redesign.
- **Events (D3)**: ghostty callbacks are `'static` closures capturing
  `Rc<RefCell<VecDeque<TerminalBackendEvent>>>` (plus `Rc<Cell<…>>` for
  synchronous answers: `on_size` from `TerminalBounds`, `on_color_scheme`
  from the theme). Callbacks fire synchronously inside `vt_write` on the
  foreground; the queue drains FIFO immediately after each `vt_write`,
  preserving PTY-response ordering (strictly stronger than today's
  lock-and-comment dance). Query-answering callbacks that must be registered:
  `WRITE_PTY` (mandatory — vim/tmux hang without it), `SIZE`,
  `DEVICE_ATTRIBUTES`, `XTVERSION`, `TITLE_CHANGED`, `BELL`,
  `CLIPBOARD_WRITE` (PTY terminals only; display-only registers the subset).
- **Construction (S4)**: `TerminalBuilder::new` keeps its
  `Task<Result<TerminalBuilder>>` signature but runs on the **foreground**
  executor (the background placement existed only for the signal-mask fix,
  now subsumed). The backend is constructed whole, in one place — no
  two-phase `Option<Core>` limbo.
- **DisplayOnly / headless**: `write_output` becomes a direct `vt_write` +
  callback drain; the headless subprocess pumps send raw bytes over the same
  bounded channel. DisplayOnly means "no `PtyHandle`", nothing else.

### Terminal creation contract

At `Terminal::new` the backend must: set `Options.max_scrollback` from
settings (creation-time only — parity, Zed never changes it at runtime);
unset `Mode::ALT_SCROLL` (parity with today's `unset_private_mode`); set the
kitty image storage limit to 0 (Zed does not render images; revisit as a
follow-up feature); register the callback set above; push theme colors
(§4.2). Runtime-mutable config is exactly cursor style/blink
(`set_default_cursor_style` / `set_default_cursor_blink`).

## 4. Seam design

Decision record: [seam-and-phase-plan.md](seam-and-phase-plan.md).

### 4.1 Structure (S1, S7)

Direct replacement via **duck-typed `TerminalBackend` structs** — no trait,
no enum, no within-platform runtime or cfg choice (S1 as amended at spec
lock; the *per-platform* cfg of §5 is a separate, bounded mechanism). P1
collapses `Terminal.{term, term_config, output_processor}` + the
free-function seam into `alacritty::TerminalBackend`;
`ghostty::TerminalBackend` is built *dark* (compiled and tested, unused by
production) with the identical inherent method surface; the swap PR re-points
one import + the construction call. Rollback during the soak = revert that
small PR. Module layout:

```
crates/terminal/src/
  pty.rs                 ← shared PTY seam (portable-pty, threads, channels)   [P4]
  ghostty.rs             ← ghostty::TerminalBackend                            [P5]
  ghostty/hyperlinks.rs  ← ported hover pipeline (string logic unchanged)      [P6]
  ghostty/grid_search.rs ← search engine; internal ScreenPoint type            [P6]
  ghostty/vi_mode.rs     ← alacritty vi_mode.rs port, 16 motions               [P6]
  alacritty.rs           ← gains the same TerminalBackend wrapper in P1;
  alacritty/hyperlinks.rs   both deleted at P10
```

`terminal.rs` holds `backend: TerminalBackend` and never sees a lock again.
The full backend method surface (ingest, snapshot, reads, mutations,
selection, vi, search/hover, theme push, derived events) is specified in
[seam-and-phase-plan.md §3](seam-and-phase-plan.md#3-ghosttyterminalbackend-interface).

### 4.2 Domain type contracts

- **Color (#31)**: the public contract becomes a Zed-owned
  `Color`/`NamedColor`/`Rgb` mirror of identical shape on the same
  `terminal::` re-export path — zero downstream churn; vte leaves the public
  API. The seam maps ghostty `StyleColor` **semantically** at snapshot build,
  read from `cell.style()` (never the flattened per-cell RGB):
  `None`→`Named(Foreground/Background)`, `Palette(0–15)`→`Named`,
  `Palette(16–255)`→`Indexed`, `Rgb`→`Spec`. Dim stays a cell flag.
  Theme defaults + the full 256-entry palette are pushed into ghostty at
  creation **and on every theme change** (setters preserve OSC overrides);
  ghostty answers OSC 4/10/11/12 queries internally with correct ordering,
  and the `ColorRequest` event path is deleted.
- **vte's fate (#31)**: survives as a utility dependency scoped to
  `strip_ansi_text` / `parse_ansi_text` (ghostty has no standalone stream
  parser). After P2, vte appears in exactly one file's imports.
- **Cell / Hyperlink (S2)**: fully Zed-owned, accessor-compatible
  (`character()`, `zerowidth()`, flag predicates): `c: char`, `fg`/`bg:
  Color`, `flags: CellFlags`, rare data behind `Option<Arc<CellExtra>>`
  (grapheme tail + hyperlink). `Hyperlink` drops its `Alacritty` variant;
  OSC 8 `id` is not exposed by ghostty (G7) — hover-extent equality compares
  URIs; `id` stays `None`.
- **Coordinates (S3)**: today's alacritty grid convention (line 0 = top of
  active screen, scrollback negative) at **every public boundary** —
  `Content`, cursor, selection, mouse math, `Terminal.matches`. The ghostty
  backend converts at snapshot build (`grid_line = viewport_row −
  display_offset`). Ghostty Screen space lives only inside `grid_search`
  behind a typed `ScreenPoint`, converted at the `find_matches` exit. Scroll
  delta signs invert in the seam; `display_offset` derives from `scrollbar()`
  (offset-from-top → offset-from-bottom), cached and refreshed only on
  scroll/resize/dirty-full — never per frame.
- **Modes**: the 17-flag `Modes` bitfield rebuilds per snapshot from typed
  `mode()` getters (full mapping: parity matrix §K). `Modes::VI` is
  synthesized from Zed's own vi-mode state.

### 4.3 Input encoding (#32)

Ghostty owns all input **encoding**; Zed owns input **policy**. Forced
finding: the core answers the kitty-keyboard progressive-enhancement query
itself, so legacy-only encoding was never parity-neutral.

- Keys: ghostty `key::Encoder` replaces `to_esc_str`'s escape-byte
  generation; Zed's owned surface is a thin GPUI `Keystroke` → ghostty
  `Key`/`Mods` mapping (`option_as_meta` → the encoder's option-as-alt).
  Vi-mode interception stays upstream of encoding.
- Mouse: ghostty `mouse::Encoder` for wire formats (incl. SGR-Pixels); Zed
  keeps the policy layer (mode gating, grid math, scroll-report repeats,
  `alt_scroll` synthesis).
- Paste: ghostty `paste::encode` (a verified superset of Zed's
  sanitization); one contract check — if ghostty maps `\r\n`→`\r\r`,
  pre-normalize `\r\n`→`\n` Zed-side.
- Focus: ghostty focus helper replaces the hardcoded `\x1b[I`/`\x1b[O`.
- **Seam rule: Zed never hand-writes escape sequences to the PTY.**
- Mode sync happens at encode time from the live foreground-owned Terminal
  (`set_options_from_terminal`) — exact freshness, no cached options.
  Between P3 and P8 a temporary Zed-`Modes`→encoder-options shim feeds the
  encoders from the alacritty core; deleted at the swap.
- The ported `keys.rs` / scroll-report suites become permanent contract
  tests: same keystroke + modes ⇒ byte-identical output, mismatches
  ledger-adjudicated.

### 4.4 Search and hyperlinks (#35, G1)

Zed-owned `grid_search` engine: ghostty extracts, Rust `regex` matches.
Search = one bulk `format_selection_buf` extract + wrap-flag line map,
per-line regex over logical-line slices (the hard-newline barrier is
structural), byte↔cell walk only on match rows. Stateless per call —
foreground extract → background regex → foreground map; a max-scrollback
benchmark gates any cache retrofit. Whole scrollback, no cap. Matches are
plain Zed-owned ranges (Screen-space internal per S3), parity invalidation
(resize clears; drift self-heals via re-search); `TrackedGridRef` rejected
for match lists. Hover stays per-logical-line: OSC 8 URI walk first, then
URL/path regexes over one extracted logical line (alacritty's 100-wrapped-row
cap); the hyperlinks.rs string pipeline is untouched. Regex semantics port
exactly: smart-case, empty-match discard, size-limit/compile-failure →
no-matches.

### 4.5 Remaining gap fills

- **Vi mode (G2)**: port alacritty `vi_mode.rs` scoped to the 16 motions Zed
  dispatches, over seam grid reads; scroll-follow and selection tie-in port
  with it.
- **Clear (G5)**: VT-sequence emulation preserving the prompt line —
  seam-local `vt_write` (never the PTY) of `CSI <cursor_y> S` → `CUP
  1;<col+1>` → `CSI 0J` → `CSI 3J`, injected at a chunk boundary. Pinned by a
  seam test written against the alacritty backend first, required to pass
  identically on ghostty. Documented fallback: adjudicate to native
  semantics via the ledger.
- **Point arithmetic (G6)**: seam-layer `add/sub/clamp` helpers with
  wide-char expansion (~50 lines), property-tested.
- **`shrink_to_used`** → compress-until-done loop over ghostty's incremental
  `compress()`; **`append_text_to_term`** → plain `vt_write` (`append_lines`
  — the unsafe hack deletes); **OSC 52 read / `ClipboardLoad`** — non-gap,
  dead code today; variant deleted at P10.

## 5. Platform rollout and the `cfg` lifecycle

*(Decided at spec lock, amending S1's scope — see
[seam-and-phase-plan.md, Amendments](seam-and-phase-plan.md#amendments-spec-lock-2026-07-16).)*

Linux-first rides **temporary per-platform cfg**, not early artifact
publication:

- The `ghostty_vt` crates enter `crates/terminal` as a Linux-only
  target-specific dependency
  (`[target.'cfg(target_os = "linux")'.dependencies]`).
- Exactly three bounded cfg points exist during the window: the dependency
  declaration, the encoder call sites in `mappings/` (from P3), and the
  backend import/construction in `terminal.rs` (from P8). On macOS/Windows
  these resolve to the alacritty-era paths (`to_esc_str`, alacritty
  backend) unchanged.
- Each platform gate (§8) begins by extending the artifact matrix and
  widening the cfg for that platform, then runs its verification program
  against the now-live ghostty paths.
- All three cfg points are dead by P10: the removal phase requires the
  gates complete, at which point the ghostty paths are unconditional and the
  cfg markers are deleted with the alacritty code.

S1's "no trait, no enum, no runtime or cfg switch" is thereby scoped to
**within-platform** choice: no build of Zed ever contains a runtime or
compile-time choice between backends for its own platform; the harness (P7)
holds both backends inside one test binary on platforms where both compile.

## 6. Phase plan

Every phase is a PR (or short PR series); the build is green and Class B
suites pass untouched after every merge. The dual-core window opens at P7 and
closes at P10. Fixed ordering edges: P0 before P3 (first production dep on
`ghostty_vt`), encoders before the core swap (#32), PTY before the swap
(S5), harness before the swap (#37).

| Phase | Contents | Acceptance criteria |
|---|---|---|
| **P0 — Artifact pipeline** (parallel track; added at spec lock) | Prebuilt repo + workflow; dispatch at the pin; stamp hashes; `fetch_prebuilt`; nix derivation; licenses section ([artifact-pipeline.md §10](artifact-pipeline.md)). | Plain `cargo build` links the prebuilt on all four Linux targets; source-build CI job green; `nix build` green. |
| **P1 — Backend-struct refactor** | Collapse term/config/processor + free functions into `alacritty::TerminalBackend`; `terminal.rs` stops locking. Pure mechanics. | All suites green, zero test edits. |
| **P2 — Zed-owned domain types** | #31 Color mirror + S2 owned `Cell`/`Hyperlink`; conversion at snapshot build. | Class B untouched; vte out of the public API; the two `Arc`-sharing round-trip tests retired via ledger; downstream crates compile unmodified. |
| **P3 — Ghostty encoders** (gated on P0; Linux-cfg per §5) | Key/mouse/paste/focus encoders against the alacritty core via the `Modes`→options shim; `keys.rs`/scroll-report suites ported as contract tests. First production dep on `ghostty_vt`. | Contract tests byte-identical (mismatches ledger-adjudicated); copy/paste + keys manual rows. |
| **P4 — PTY/threading swap** | `pty.rs` (portable-pty git-pinned per §8.2); reader/writer threads; bounded byte channel; foreground pump → `Processor::advance`; EventLoop/SignalMask deleted; construction moves foreground; headless converges on the byte channel. | Class B untouched; PTY integration suite (spawn/resize/kill/exit) green on Linux + Windows CI incl. ConPTY shutdown/exit/kill tests; **sustained-flood benchmark lands here** (decided at spec lock) and validates channel/batch tuning; affected manual rows. |
| **P5 — Dark ghostty backend: core** | Snapshot path, mutations, selection, G5 clear, theme push; G5 clear test + G6 point tests + `format_selection_buf` characterization tests. Unused by production. | Backend suite green; clear test passes identically on both backends. |
| **P6 — Dark ghostty backend: gap fills** | `grid_search`, hover pipeline, `vi_mode` port, full #35 test plan (36 hyperlink scenarios re-hosted; upstream vi tests; byte↔cell map tests; max-scrollback extraction benchmark). Divisible; parallel with P7. | #35's acceptance bar met. |
| **P7 — Differential harness** (window opens) | Harness holds both backends; recorder + recorded/synthetic corpus; non-gating fuzzer; divergence ledger seeded; alacritty perf baseline recorded (release builds). | Corpus runs divergence-free modulo adjudicated entries; CI job wired. |
| **P8 — The swap** (Linux-cfg per §5) | `terminal.rs` re-points to `ghostty::TerminalBackend`; shim deleted; `ColorRequest`/`ClipboardLoad`/`TextAreaSizeRequest` handler arms deleted; encode-time mode sync live; `append_lines` replaces the unsafe append. Rollback = revert this PR. | Gate items 1–4; perf within 20% of the P7 baseline; full 12-row manual checklist + `:99` smoke reel. |
| **P9 — Soak + platform gates** (window, not one PR) | Two-week Linux soak; macOS full program per §8.1 (opens by widening the cfg + artifact matrix); Windows remainder per §8.2 (ditto). | Gate items 5–9. |
| **P10 — Removal** (window closes) | Freeze goldens + wire golden-replay CI; delete `alacritty.rs`/`alacritty/hyperlinks.rs` + Class C tests; drop `alacritty_terminal` from both Cargo.tomls + prune the lock; slim `TerminalBackendEvent` + `PtySender`-era glue; delete the §5 cfg markers; regenerate `licenses.md` (alacritty leaves, vte stays); commit the final ledger state. | The nine-point gate checklist reproduced and checked in the PR description; `grep -r alacritty crates/` returns only ledger/doc references. |

Post-removal riders: portable-pty back to a crates.io release; channel/batch
tuning; the dedicated-terminal-thread escape hatch only on a real regression.

## 7. Verification

Decision record: [verification-strategy.md](verification-strategy.md).

Two-layer oracle. (1) **Ported expectations**: existing suites port with
expected values byte-identical; any changed expectation is a parity finding
adjudicated in the divergence ledger
([divergence-ledger.md](divergence-ledger.md), created at P7), never a
routine test fix. (2) **Differential goldens**: while both cores are
in-tree, identical raw-byte transcripts (recorded real sessions +
per-matrix-row synthetic sequences, with first-class resize events) feed both
backends and seam-level snapshots are compared — parity is "Zed cannot tell
the difference". Divergence-free runs freeze into goldens; the golden-replay
CI job outlives alacritty as the permanent regression net. Fuzzing runs
differentially, non-gating; no unadjudicated finding may remain at gate time.

Suite disposition: **Class A** port with expectations intact (keys, mouse,
hyperlink scenarios, semantic round-trips); **Class B** pass literally
unchanged, zero edits (all suites above the seam — the proof the seam held);
**Class C** retire with their subject (pure type-conversion tests). Gap-fill
components seed their suites from alacritty's upstream tests.

Performance: transcript-driven benchmarks on release builds (colored dump,
sustained scroll, alt-screen churn, wide-char/CJK, and sustained flood —
which lands at P4). Gate: no scenario regresses >20% vs the alacritty
baseline, no cliff anomaly, and the shipped native core asserts ReleaseFast.

Interactive: scripted `:99` smoke reel (automation drives, a human reviews);
a 12-row manual checklist with expected observations, affected rows after
each subsystem phase, one full pass before removal.

**The nine-point removal gate** (all green, reproduced in the P10 PR):
Class B untouched; Class A intact (ledger-adjudicated); differential corpus
clean + goldens frozen + replay job wired; gap-fill suites green; no
unadjudicated fuzz findings; perf within 20%/no cliff/ReleaseFast; full
manual checklist + reviewed smoke reel; two-week Linux daily-driver soak with
zero unresolved P0/P1 issues (alacritty in-tree as rollback throughout); §8
platform gates satisfied.

## 8. Platform gates (block P10)

Decision record: [verification-strategy.md §9](verification-strategy.md#9-platform-gates-macos-windows).
Asymmetric by decision. Each gate **opens** by extending the artifact matrix
and widening the §5 cfg for its platform.

### 8.1 macOS — full program

Every gate criterion 1–8 re-run on real macOS hardware: macOS-local perf
baseline; the sustained-flood scenario (targeting the ~1 KiB master-read
cap); the `/usr/bin/login` wrapper ported from **ghostty's** exec-layer
behavior (a deliberate, ledger-adjudicated divergence from alacritty-today;
verified by argv fixtures + three manual smoke items); a two-week
session-based soak (≥3 real dev sessions/week), concurrent with Linux's.

### 8.2 Windows — reduced bar (PTY-scoped)

Windows CI green incl. Class A/B dispositions; a three-test ConPTY
integration suite (shutdown without EOF-wait hang, exit observation via
`try_wait` polling on the `pty_info` cadence, kill path); portable-pty
git-pinned for the wezterm#7709 `kill()` fix; manual smoke on real Windows
hardware. The vt core is platform-independent — the Linux differential corpus
vouches for it.

## 9. Deferred decisions and accepted losses

Recorded so no implementer reopens them:

- **Accepted information loss**: ghostty collapses SGR 31 vs 38;5;1 into
  `Palette(1)` → both become `Named(Red)`; verified unobservable in Zed.
- **Accepted regression (non-gap)**: OSC 52 *read* stays unsupported
  (matches Zed's effective behavior today).
- **Accepted merge**: adjacent distinct OSC 8 links with identical URIs merge
  into one hover region (G7 — no id exposed).
- **Deferred to the Windows gate**: msvc vs gnullvm target choice,
  `ghostty-vt-static.lib` naming, ubsan-rt behavior under MSVC.
- **Deferred post-removal**: portable-pty crates.io migration, channel/batch
  tuning, dedicated-terminal-thread escape hatch, kitty
  graphics/sixel/semantic-prompt/in-band-resize feature adoption, unsafe-paste
  confirmation UX.

## 10. Spec-lock resolutions (2026-07-16)

Decisions made during the sign-off grilling
([ticket #38](https://github.com/xipeng-jin/zed/issues/38)), each folded into
the sections above:

1. **Platform-`cfg` lifecycle** (§5): temporary per-platform cfg; S1 scoped
   to within-platform choice. Resolves the S1 ↔ build-strategy §6.6 conflict.
2. **P0 artifact-pipeline track** (§6): the pipeline implementation is a
   phase with acceptance criteria, parallel to P1–P2, hard edge P0→P3.
3. **Sustained-flood benchmark lands at P4** (§6, §7); the spike's
   time-budgeted-drain idea is formally superseded (§3, D2).
4. **Spec home**: `docs/ghostty-migration/SPEC.md`, beside its records.
5. **Execution context**: fork-first, declared in §1; upstreaming is a
   separate post-migration effort.
6. **Precedence**: the spec is the single execution authority; superseded
   records carry matching amendments
   ([seam-and-phase-plan.md](seam-and-phase-plan.md),
   [verification-strategy.md](verification-strategy.md)).
