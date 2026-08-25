> [!NOTE]
> **v2 status (2026-08-25):** v1 decision record, salvaged as the v2 starting text (salvage-policy.md rule 1). **Not locked for v2**: the re-opened ticket amends this file in place and removes this banner on resolution. Source: `migration/libghostty` @ `e537270dac`.

# Spike findings: libghostty-vt-backed terminal inside Zed

_Wayfinder ticket [#34](https://github.com/xipeng-jin/zed/issues/34). Spike branch:
`spike/ghostty-vt-terminal` (throwaway, never merges). Gate: `ZED_GHOSTTY_SPIKE=1`,
Linux only._

## What the spike is

The minimal end-to-end path: `forkpty` PTY → reader thread → byte channel →
foreground GPUI task owning a vendored `ghostty_vt::Terminal` → `vt_write` →
`RenderState` snapshot → hand-built `Content`/`IndexedCell` → the **unmodified**
`terminal_element` renderer. Input rides Zed's existing legacy `to_esc_str`
encoder; writes go straight to the PTY master fd. No selection, no search, no
hyperlinks, no vi mode.

~460 lines in `crates/terminal/src/ghostty_spike.rs` plus ~90 lines of hooks in
`terminal.rs` (a third `TerminalType` variant, a sync() bypass, resize/scroll
forwarding).

## Seam-friction findings

### 1. `!Send` bites at construction, not at rendering (structural, must be planned for)

`TerminalBuilder::new` runs inside `cx.background_spawn` (deliberately, for
signal-mask reasons — see the comment at `terminal.rs:1191`). A `!Send` ghostty
`Terminal` therefore **cannot be a field of `Terminal`** as the code is shaped
today: the entity is constructed on a background thread and moved to the
foreground. The spike works around it by having the foreground driver task own
the ghostty terminal, communicating via channels; the real migration must
either (a) split construction so the core is created foreground-side in
`subscribe()` (what the spike does), or (b) move terminal construction back to
the foreground thread. The PTY/threading architecture note assumed the core
lives "inside the Terminal entity" — that stays true only if construction is
restructured. This is the single biggest deviation the spike surfaced vs. the
architecture note.

Corollary: every existing call site that reads the grid synchronously from
`&Terminal` (`get_content`, `last_n_non_empty_lines`, `with_renderable_cells`,
`content_text`, search, hyperlink discovery) assumes the core is reachable from
the entity. With the driver-task-owned core those would all need command/reply
round-trips — unacceptable. Conclusion for the seam ticket: **the core must be
a foreground-owned field of `Terminal`** (option (a)/(b)), not a task-local.

### 2. `Arc<FairMutex>` disappears cleanly

Nothing in the spike needed a lock. `sync()` + `last_content` is already a
message-passing design: alacritty's lock exists because its event-loop thread
mutates the grid concurrently. With ghostty on the foreground thread the lock
(and `with_renderable_cells`'s lock-holding closure) simply evaporates. No
`unsafe`, no `Send` assertions were needed anywhere in the spike.

### 3. RenderState → Content mapping is mostly mechanical, with four impedance points

The happy path (codepoint, bold/italic/inverse/dim/strikethrough/underline
flags, wide char + spacer flags, cursor shape/position) mapped 1:1 in ~120
lines. The impedance points:

- **Graphemes vs `char` + zerowidth**: ghostty cells carry N codepoints;
  alacritty-shaped `Cell` wants first char + `push_zerowidth` for the rest.
  Mechanical today because Zed's `Cell` wraps the alacritty cell; the real
  seam should own a grapheme-native cell type instead of replaying the
  first-char+zerowidth split.
- **Viewport vs grid coordinates**: `RenderState` iterates the *viewport*
  (rows 0..N); Zed's `Content` speaks alacritty grid lines where scrollback
  shows as negative lines (`grid_line = viewport_row - display_offset`) and
  the cursor needs the same shift. Easy once seen, invisible until then —
  `terminal_element` renders by enumerated line groups, but mouse math and
  `DisplayCursor` arithmetic depend on the alacritty convention.
- **`display_offset` reconstruction**: ghostty's `scrollbar()` gives
  `{total, offset-from-top, len}`; Zed wants offset-from-bottom. One-line
  subtraction, but `scrollbar()` is documented "may be expensive… don't call
  too frequently" and the spike calls it every frame. The real seam should
  read it only on scroll/resize/dirty-full.
- **Flattened colors vs the color contract**: `fg_color()/bg_color()` return
  resolved RGB (palette + defaults applied). The spike maps `Some(rgb)` →
  `Color::Spec` and `None` → `Named(Foreground/Background)`, which loses the
  `Indexed`/`Named` classification Zed's theme mapping wants. This confirms
  the color-contract decision (ticket #31): the real seam must read the
  **style-level** `StyleColor` (palette index vs rgb vs default) from
  `cell.style()`, not the flattened per-cell queries. The flattened path is
  fine for a spike; it renders correctly but theme-remapping semantics
  (e.g. theme switch recoloring already-drawn palette text) would be wrong.

### 4. Synchronous callbacks compose fine with the byte-channel design

`on_pty_write` (query responses) fires inside `vt_write` on the foreground
task and writes straight to the PTY fd — correct ordering for free, exactly as
the architecture note predicted. No re-entrancy trouble: the callback gets
`&Terminal` while the driver holds `&mut`, and the wrapper's design (no
`vt_write` from inside a callback) is enforced by borrow shape.

### 5. Legacy input encoding works against the ghostty core — with the known kitty caveat

bash/ls/htop/vim all usable with Zed's existing `to_esc_str` tables reading
`Modes` rebuilt from ghostty `mode()` queries (DECCKM, bracketed paste, mouse
modes, alt screen via `active_screen()`). The kitty-keyboard impedance from the
parity matrix stands: the core answers `CSI ? u` itself, so kitty-probing apps
(nvim) would mis-negotiate under legacy-only encoding. Confirms the input
ticket's decision to adopt ghostty's `key::Encoder` as its own phase.

### 6. FFI/build landmines

- **None new from the vendored crates**: `ghostty_vt` built, linked, and ran
  inside the full zed binary on the first try (source-build fallback path,
  zig 0.15.2). No symbol clashes, no allocator issues, no crashes in FFI.
- The vendored rev's Rust API surface was sufficient for the whole spike
  (render iterators, scrollbar, modes, callbacks); nothing needed from
  refs-HEAD that the vendored fork lacks.
- Per-cell FFI chattiness is real but cheap: building a frame makes ~6 FFI
  calls per cell (raw_cell, graphemes_len, fg/bg, has_styling, style). See
  perf numbers below.

## Perf sanity

Instrumentation: the spike driver logs per-rebuild timings for `vt_write`,
`RenderState::update` (snapshot), and Content build (full IndexedCell grid,
~1950 cells at 1280×800); `ZED_TERM_PERF=1` logs alacritty `make_content` for
baseline.

**Steady state (zig `Debug` native lib, interactive shell use):**

| stage | avg | max |
|---|---|---|
| `vt_write` (small batches) | 25–85 µs | 1.5 ms |
| `RenderState::update` snapshot | **14 µs** | 55 µs |
| Content build (full grid, ~6 FFI calls/cell) | 620–750 µs | 1.2 ms |

The snapshot itself is essentially free; the per-cell FFI walk to rebuild the
full `Content` costs ~0.7 ms/frame — same order as alacritty's `make_content`
(baseline below). Dirty tracking reported Clean/Partial correctly per frame —
usable, but at these numbers not *necessary* for parity.

**Landmine: zig `Debug` builds of libghostty-vt collapse once scrollback is
non-empty.** After ~300 lines entered scrollback, `vt_write` degraded from
~8 ns/byte to ~24 µs/byte (≈3000×), with single calls up to **3.6 s** on
`seq 1 200000` — and because the spike parses on the GPUI foreground thread,
each long `vt_write` froze the entire UI (input queued, no repaints). Two
independent lessons:

1. **Build strategy**: the source-fallback path follows the Cargo profile, so
   dev builds get a zig `Debug` core whose runtime safety checks appear to
   scale with scrollback. Dev builds must pin `ReleaseFast`
   (`LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseFast`) — worth folding into the
   build-strategy doc / `build.rs` default. (Prebuilts are ReleaseFast
   already.)
2. **Architecture**: even with a fast core, unbounded per-wakeup byte batches
   on the foreground thread are a UI-latency hazard. The spike's 2 MiB cap is
   too coarse; the real seam wants a time-budgeted drain (parse ≤ N ms per
   frame, yield, continue) — or the architecture note's escape hatch (dedicated
   terminal thread) if budgeting proves fiddly.

**ReleaseFast re-run** (`LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseFast`, zed still a
debug build): the same `seq 1 200000` (1.49 MB) completed in **54 ms wall**
(shell-measured), processed across 214 rebuilds with `vt_write` avg 35 µs /
max 382 µs (≈5 ns/byte — ~14,000× the Debug rate), snapshot avg 0.8 µs,
content build avg 172 µs. UI stayed responsive throughout. The pathology is
purely the zig Debug optimize mode; the core itself is comfortably fast.

Debug death rattle for the record: the Debug-core process eventually processed
the same 1.46 MB in a single `vt_write` of **106.6 s**.

**Alacritty baseline** (same machine, same debug zed, `ZED_TERM_PERF=1`):
`make_content` 80–140 µs for the same 1358-cell grid; `seq 1 200000` wall time
0.145 s (spike: 0.054 s — both fine; different PTY drain patterns). So the
spike's full-grid rebuild is ~2–4× alacritty's snapshot cost, entirely in the
per-cell FFI walk. Ghostty exposes style IDs / `has_styling`, so a real seam
can cut most per-cell calls (skip `style()` for unstyled cells — already done —
and cache by style id); parity does not depend on exploiting dirty tracking.

## Validation log

Nested-Xwayland harness (`:99`), dev build, `ZED_GHOSTTY_SPIKE=1`,
fresh `--user-data-dir`. All via XTEST-driven interaction + screenshots:

- Terminal panel opens; bash prompt renders with correct prompt colors and a
  block cursor; typing echoes correctly.
- `ls --color`: 16-color palette output correct (bold blue dirs).
- SGR styles: bold red / italic green / underline blue / inverse all render.
- Truecolor: 24-bit fg and bg escape sequences render exactly.
- Wide chars: CJK (你好世界) renders in wide cells with correct spacer
  handling; combining accent (e◌́) renders composed. Emoji (🚀) came out blank —
  **also blank on the alacritty baseline** in the same harness, so it's font
  fallback on the nested X server, not a spike defect.
- Scrollback: wheel scroll into 300 lines of history and back works
  (`display_offset` reconstruction from ghostty `scrollbar()` is correct).
- Alt screen: `less` and `top` enter/leave the alternate screen correctly;
  `top` live-updates with bold + inverse header.
- Resize: window resize reflows the grid; `top` redraws at the new size
  (TIOCSWINSZ + ghostty `resize` both wired).
- Child exit → `Event::CloseTerminal` closes the tab.

## Verdict for the seam-design ticket

The integration is viable and the architecture note's shape survives contact,
with five concrete inputs for the seam design:

1. **Construction must move foreground-side (or split).** `TerminalBuilder::new`
   on the background executor cannot own a `!Send` core; the grid-reading API
   surface (`get_content`, search, hyperlinks) demands the core be a field of
   `Terminal`, so restructure construction rather than adopt the spike's
   driver-task workaround.
2. **The `FairMutex` and its lock choreography delete cleanly** — nothing
   needed it; `sync()` + `last_content` already isolates rendering from the
   core.
3. **Colors must come from `Style`/`StyleColor`, not the flattened per-cell
   RGB queries**, to honor the ticket-#31 contract (`Named`/`Indexed`
   classification for theme remapping).
4. **Coordinate convention needs one deliberate decision**: keep alacritty's
   negative-line grid space at the `Content` boundary (spike approach —
   `viewport_row − display_offset`) or migrate `Content`/mouse math to
   viewport space. Either works; mixing them is the trap. Related:
   `scrollbar()` is documented as potentially expensive — read it on
   scroll/resize/dirty-full, not per frame.
5. **Byte-drain needs a time budget** (or the dedicated-thread escape hatch),
   and dev source-builds of the native lib must pin `ReleaseFast` — zig Debug
   cores are unusable with non-empty scrollback (3000× vt_write degradation,
   multi-second foreground stalls).

Nothing surfaced that argues against the incremental-swap plan; no new
build/FFI landmines beyond the optimize-mode pin.
