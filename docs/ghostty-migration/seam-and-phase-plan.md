# Seam design and incremental swap phase plan

Decision record for [ticket #36](https://github.com/xipeng-jin/zed/issues/36). Defines the
replacement seam module's interface, the coexistence strategy for the two cores, and the
ordered phase plan ending in `alacritty_terminal` removal. This is the spine of the final
migration spec (ticket #38).

Inputs (all locked, none reopened here): the [parity matrix](parity-matrix.md) (#29), the
[PTY/threading architecture](pty-threading-architecture.md) (#30), the color contract (#31),
the input-encoding decision (#32), the vendored bindings (#33), the
[spike findings](spike-findings.md) (#34), the [search/hyperlink design](search-and-hyperlinks.md)
(#35), the [verification strategy](verification-strategy.md) incl. §9 platform gates (#37, #40),
and the [artifact pipeline](artifact-pipeline.md) (#39).

Fact base: `alacritty_terminal` is a dependency of exactly one crate (`crates/terminal`), and
exactly two files reference it (`src/alacritty.rs`, `src/alacritty/hyperlinks.rs`). Everything
downstream compiles against Zed-owned types on `terminal::` re-export paths.

---

## 1. Decision summary

| # | Decision | One-line rationale |
|---|----------|--------------------|
| **S1** | **Direct replacement via duck-typed `TerminalBackend` structs — no trait, no enum, no runtime or cfg switch.** A pre-swap refactor collapses `Terminal.{term, term_config, output_processor}` + the free-function seam into `alacritty::TerminalBackend`; `ghostty::TerminalBackend` is built *dark* (compiled, tested, unused by production) with the identical inherent method surface; the swap PR re-points one import + the construction call. | A runtime abstraction must unify `Arc<FairMutex<Term>>` (Send) with a `!Send` core — the union of both worlds carried precisely during the highest-risk window; a cfg flag doubles CI and blocks a single-process dual-core harness. Duck typing gets both cores into one test binary with zero production abstraction; rollback during the soak is reverting a small PR. |
| **S2** | **`Cell`/`Hyperlink` become fully Zed-owned, accessor-compatible, landed pre-swap with the #31 Color mirror in one PR.** Owned storage (`c: char`, `fg`/`bg: Color`, `flags: CellFlags`, rare data behind `Option<Arc<CellExtra>>` holding grapheme tail + hyperlink); `Hyperlink` drops its `Alacritty` variant. | Ghostty render cells are transient FFI handles, so owned materialization at snapshot time is forced; keeping the accessor shape (`character()`, `zerowidth()`, flag predicates) keeps `terminal_element`, `repl`, and every Class B suite untouched. Doing it against alacritty first makes `Content` core-agnostic — the differential harness compares the two backends' `Content` values directly. Bundled with #31 because both change the same `fg`/`bg` fields. |
| **S3** | **Grid convention at every public boundary; Screen space internal-only with a typed guard.** `Content`, cursor, selection, mouse math, and `Terminal.matches` keep today's alacritty grid convention (line 0 = top of active screen, scrollback negative); the ghostty backend converts at snapshot build (`grid_line = viewport_row − display_offset`). Ghostty Screen space lives only inside `grid_search` as a distinct `ScreenPoint` type, converted to grid points at the `find_matches` exit. | Class B suites encode grid-space expectations and must pass literally unchanged (gate 1). Mixing spaces in one `Point` type is the spike's named trap — the distinct type makes it a compile error. This narrows #35 (Screen-space match storage becomes an internal representation; observable behavior is parity either way, since #35 already accepts drift + re-search self-healing). |
| **S4** | **Construction moves wholly foreground as a signature-preserving executor change, riding with the PTY phase.** `TerminalBuilder::new` keeps returning `Task<Result<TerminalBuilder>>`; the future runs on the foreground executor instead of `background_spawn`. The backend is constructed whole, in one place. | The background placement exists only for the signal-mask fix (zed#42234), which portable-pty's `pre_exec` subsumes. Call-site check: the task is created and awaited on the foreground already (`project/terminals.rs:263`); `new_display_only` is already synchronous+foreground. Whole-construction beats the spike's split (no two-phase `Option<Core>` limbo). Cost: `openpty` + fork/exec on the UI thread — single-digit ms on a rare user action. |
| **S5** | **The PTY/threading architecture lands as its own phase before the core swap, against the alacritty core.** Reader thread → bounded(4)×64 KiB channel → foreground pump → `Processor::advance(&mut term.lock(), bytes)` (the exact analogue of today's DisplayOnly path); alacritty's `EventLoop`, `Notifier`, SIGCHLD and `SignalMask` machinery delete here. | The two highest-risk changes (threading model, emulator core) become separately bisectable; a threading regression surfaces while the emulator oracle is still alacritty. Mirrors the #32 encoders-first precedent. The channel seams are identical by design (#30's escape-hatch argument), so nothing is throwaway — at swap time the pump's consumer changes one call. |
| **S6** | **G5 (`clear_saved_screen`) resolves to VT-sequence emulation preserving the prompt line.** Seam-local `vt_write` (never the PTY): `CSI <cursor_y> S` → `CUP 1;<col+1>` → `CSI 0J` → `CSI 3J` (scrollback erased last so scrolled-in rows die too), injected at a chunk boundary. Pinned by a new seam test written against the *alacritty* backend first, then required to pass identically on ghostty. | "`clear` behaves as before" is a manual-checklist gate row; the native-semantics alternative spends a ledger entry on something four standard sequences get right. Risk retires during the dark phase; documented fallback is adjudicating to native semantics. |
| **S7** | **The PTY seam is a shared sibling module (`pty.rs`), outside both backends.** portable-pty open/spawn, reader (reaps on EOF) + writer threads, `PtyHandle { notify, resize, shutdown }`, `ProcessIdGetter` impls. | After S5 it is core-agnostic by construction; both backends are pure byte consumers, and the differential harness feeds transcripts below the PTY exactly as #37 specifies. |
| **S8** | **Ten-phase plan (§4), dual-core window opening at the harness phase (P7) and closing at removal (P10).** | Every phase PR-sized and green; fixed edges honored (encoders before swap per #32, PTY before swap per S5, harness before swap per #37); the nine-point gate sits between P8/P9 and P10, with §9 platform gates inside P9. |

Decisions inherited and merely *placed* here (not reopened): runtime scrollback reconfiguration
is a non-issue (parity-matrix row 71: Zed only sets `scrolling_history` at creation;
`apply_config` fires solely for cursor style, and ghostty has runtime
`set_default_cursor_style`/`set_default_cursor_blink`); `shrink_to_used` → compress-until-done
loop (matrix row 180); `append_text_to_term` → plain `vt_write` (matrix row 179, unsafe hack
deleted); OSC 52 read / `ClipboardLoad` drops as a non-gap (matrix §1).

## 2. Module layout

```
crates/terminal/src/
  pty.rs                 ← shared PTY seam (portable-pty, threads, channels)   [new, P4]
  ghostty.rs             ← ghostty::TerminalBackend                            [new, P5]
  ghostty/hyperlinks.rs  ← ported hover pipeline (#35; string logic unchanged) [new, P6]
  ghostty/grid_search.rs ← #35 engine; internal ScreenPoint type lives here    [new, P6]
  ghostty/vi_mode.rs     ← alacritty vi_mode.rs port, 16 motions (#29)         [new, P6]
  alacritty.rs           ← gains the same TerminalBackend wrapper in P1;
  alacritty/hyperlinks.rs   both deleted at P10
```

`terminal.rs` holds `backend: TerminalBackend` (replacing the three storage couplings at
`terminal.rs:1413-1415`) and never sees a lock again — the alacritty backend locks internally.
The contract between the two modules is "same inherent method surface", enforced by the swap
PR compiling; drift during the dark window is caught by the differential harness exercising
the full surface in CI.

## 3. `ghostty::TerminalBackend` interface

`!Send`, foreground-constructed (S4), owned by the `Terminal` entity.

**State**: `ghostty_vt::Terminal<'static, 'static>`; `RenderState`;
`Rc<RefCell<VecDeque<TerminalBackendEvent>>>` callback queue;
`Rc<Cell<TerminalBounds>>` (answers `on_size` synchronously);
`Rc<Cell<ColorScheme>>` (answers CSI ?996n); ported `ViModeCursor`; selection state;
cached `display_offset` refreshed from `scrollbar()` only on scroll/resize/dirty-full
(spike input #4's perf note — never per frame).

**Method surface** (mirrored by `alacritty::TerminalBackend` from P1):

- **Ingest** — `write(&mut self, bytes)`: `vt_write` + immediate FIFO drain of the callback
  queue (PTY-response ordering per #30 D3). Serves the pump, `write_output`, and the headless
  subprocess path. DisplayOnly means "no `PtyHandle`", nothing else.
- **Snapshot** — `make_content(&mut self, last: &Content) -> Content`: `RenderState::update` →
  S3 coordinate conversion → owned S2 `Cell`s with the #31 `StyleColor` semantic mapping
  (read from `cell.style()`, never the flattened per-cell RGB). A `Dirty::Clean` frame may
  return the previous `Content` untouched. `renderable_cells()` feeds `repl`'s consumer.
- **Reads** — `total_lines`, `screen_lines`, `display_offset`, `content_text`,
  `last_n_non_empty_lines`, `full_content_range`, `modes() -> Modes` (the matrix's 17-flag
  mapping over typed getters).
- **Mutations** — `resize(bounds)` (explicit cols/rows/px; the alacritty `Dimensions` trait
  impl on `TerminalBounds` dies with the fork), `scroll_display`, `scroll_to_point`,
  `clear()` (the S6 sequence), compress-until-done (ex-`shrink_to_used`), `append_lines()`
  (ex-`append_text_to_term`, now safe).
- **Selection** — `set/update_selection`, `selection_text` (`format_selection_buf`),
  `select_all`, semantic word selection passing alacritty's escape-char set per call
  (matrix row 76).
- **Vi** — `toggle_vi_mode`, `vi_motion`, `vi_goto_point`, `update_vi_cursor_for_scroll`,
  `update_selection_to_vi_cursor` over the ported cursor.
- **Search/hover** — the #35 surface: `extract_logical_lines`, `cell_map_for_line`,
  the `find_matches` sandwich pieces, `find_from_terminal_point`.
- **Theme** — `push_theme_colors()` at creation and on theme change (#31's reverse-direction
  contract: defaults + full 256-entry palette; OSC overrides survive).
- **Derived events** — `CursorBlinkingChange` and `MouseCursorDirty` computed
  frame-over-frame from snapshot/mode diffs (no ghostty callback exists).
  `ColorRequest` / `ClipboardLoad` / `TextAreaSizeRequest` are never emitted by this backend;
  their `terminal.rs` handler arms die at the swap (P8), the enum variants at removal (P10) —
  the dark alacritty backend still produces them for the harness in between.

Input encoding is deliberately **not** on the backend: per #32, the encoders live in
`mappings/` and read live modes from the backend at encode time; the `Modes`→options shim
exists only between P3 and P8.

## 4. Phase plan

Every phase is a PR (or short PR series); the build is green and Class B suites pass
untouched after every merge. **The dual-core window opens at P7 and closes at P10.**

| Phase | Contents | Acceptance criteria |
|---|---|---|
| **P1 — Backend-struct refactor** | Collapse `term`/`term_config`/`output_processor` + free functions into `alacritty::TerminalBackend`; `terminal.rs` stops locking. Pure mechanics. | All suites green, zero test edits. |
| **P2 — Zed-owned domain types** | #31 Color mirror + S2 owned `Cell`/`Hyperlink`; `make_content` converts at snapshot build. | Class B untouched; vte out of the public API (one file's imports); the two `Arc`-sharing round-trip tests retired via ledger entry; downstream crates compile unmodified. |
| **P3 — Ghostty encoders (#32)** | Key/mouse/paste/focus encoders against the alacritty core via the `Modes`→options shim; `keys.rs`/scroll-report suites ported as contract tests. First production dep on `ghostty_vt` ⇒ **gated on #39 prebuilts live**. | Contract tests byte-identical (mismatches ledger-adjudicated); copy/paste + keys manual rows. |
| **P4 — PTY/threading swap (S5+S4+S7)** | `pty.rs` (portable-pty **git-pinned at the wezterm#7709 rev** per §9.2); reader/writer threads; bounded byte channel; foreground pump → `Processor::advance`; EventLoop/SignalMask deleted; construction moves foreground; headless subprocess converges on the byte channel. | Class B untouched; G3 PTY integration suite (spawn/resize/kill/exit) green on Linux + Windows CI incl. §9.2 ConPTY shutdown/exit/kill tests; sustained-flood benchmark validates channel/batch tuning; affected manual rows. |
| **P5 — Dark ghostty backend: core** | `ghostty::TerminalBackend` snapshot path, mutations, selection, S6 clear, theme push; G5 clear test + G6 point tests + `format_selection_buf` characterization tests. Unused by production. | Backend suite green; clear test passes identically on both backends. |
| **P6 — Dark ghostty backend: gap fills** | `grid_search`, hover pipeline, `vi_mode` port, full #35 §4 test plan (36 hyperlink scenarios re-hosted; upstream vi tests; byte↔cell map tests; max-scrollback extraction benchmark). Divisible; parallel with P7. | #35's acceptance bar met. |
| **P7 — Differential harness** (window opens) | Harness holds both backends; recorder + recorded/synthetic corpus; non-gating fuzzer; divergence ledger seeded; **alacritty perf baseline recorded** (release builds). | Corpus runs divergence-free modulo adjudicated entries; CI job wired. |
| **P8 — The swap** | `terminal.rs` re-points to `ghostty::TerminalBackend`; shim deleted; `ColorRequest`/`ClipboardLoad`/`TextAreaSizeRequest` handler arms deleted; encode-time mode sync live; `append_lines` replaces the unsafe append. Rollback = revert this PR. | Gate items 1–4 (Class B untouched, Class A intact, differential corpus clean, gap-fill suites green); perf within 20% of the P7 baseline; full 12-row manual checklist + `:99` smoke reel. |
| **P9 — Soak + platform gates** (window, not one PR) | Two-week Linux soak; macOS full program per §9.1 (the `/usr/bin/login` wrapper lands as its own PR + ledger entry; macOS-local baseline; session soak, concurrent with Linux's); Windows §9.2 remainder (manual smoke on real hardware). | Gate items 5–9. |
| **P10 — Removal** (window closes) | See §5. | The nine-point §8 gate checklist reproduced and checked in the PR description; `grep -r alacritty crates/` returns only ledger/doc references. |

Post-removal riders (per §9.3): portable-pty back to a crates.io release; channel/batch
tuning; the dedicated-terminal-thread escape hatch only if a real regression appears.

## 5. Removal checklist (P10)

Absorbs the map's "Alacritty removal checklist" fog item. Executes only with all nine §8
gate criteria green:

1. Freeze the differential goldens; delete the harness's alacritty side; wire the
   golden-replay CI job (the permanent regression net outliving alacritty).
2. Delete `crates/terminal/src/alacritty.rs` + `alacritty/hyperlinks.rs` + Class C tests.
3. Drop `alacritty_terminal` from `crates/terminal/Cargo.toml` and the workspace
   `Cargo.toml` (the zed-industries/alacritty fork git pin); prune `Cargo.lock`.
4. Slim `TerminalBackendEvent` (delete `ColorRequest`, `ClipboardLoad`,
   `TextAreaSizeRequest`, and other now-dead variants) and remaining `PtySender`-era glue.
5. Regenerate `licenses.md`: alacritty attribution leaves; **vte stays**, scoped to
   `strip_ansi_text`/`parse_ansi_text` per #31.
6. Commit the final divergence ledger state.
