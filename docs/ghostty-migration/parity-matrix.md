# libghostty-vt feature-parity matrix

Authoritative mapping of everything Zed's terminal uses from `alacritty_terminal` (and `vte`)
onto what libghostty-vt (C API at `ghostty/include/ghostty/vt*.h`) and the vendored
libghostty-rs safe bindings (`libghostty-rs/crates/libghostty-vt/src/`) provide, with a fill
strategy for every gap.

Sources of truth read for this matrix:

- Zed seam: `crates/terminal/src/alacritty.rs`, `crates/terminal/src/alacritty/hyperlinks.rs`,
  `crates/terminal/src/terminal.rs`, `crates/terminal/src/mappings/`,
  `crates/terminal_view/src/terminal_element.rs`, `crates/debugger_ui/src/session/running/console.rs`.
- alacritty_terminal (Zed fork, `~/.cargo/git/checkouts/alacritty-20195d12a03fa0c5/4c12966/alacritty_terminal/`),
  cited below as `alacritty:<path>`.
- libghostty-vt C headers (`refs/ghostty/include/ghostty/vt/*.h`, cited as `vt/<header>`),
  ghostty core (`refs/ghostty/src/terminal/`, cited as `ghostty:<path>`), and libghostty-rs
  (cited as `lg:<file>`).

Citations are `file:line`. Every `Zed uses` claim was verified against the code on branch
`migration/libghostty` (2026-07-14).

---

## 1. Verdict summary

**Row tally: 93 rows — 65 Covered, 11 Partial, 17 Gap rows, and the 17 gap rows collapse
into 8 logical gaps (G1-G8 below).**

The libghostty-vt core covers the entire grid/state/parsing surface Zed needs, usually with a
richer model (dirty tracking, tracked grid refs, graphemes, typed modes). The true gaps cluster
in exactly the places the ticket suspected, plus one it did not:

| # | Gap | One-line fill strategy |
|---|-----|------------------------|
| G1 | **Regex search over the grid** (find-in-terminal + URL hover) | Zed-owned engine: extract logical-line text via ghostty row/cell iteration (WRAPLINE-aware, spacer/grapheme-aware byte↔cell map), run Rust `regex` over it; extraction on the owner thread, matching on a background thread. |
| G2 | **Vi mode** (`ViModeCursor`/`ViMotion`) | Port alacritty `vi_mode.rs` (~small, self-contained) into Zed-owned code over seam grid reads; only the 16 motions Zed dispatches; `Modes::VI` becomes a Zed-side flag. |
| G3 | **PTY + event loop** (`tty`, `EventLoop`, `Notifier`) | Deliberate ghostty non-goal; port alacritty's `tty` module (unix + ConPTY) into a Zed-owned crate, reader task feeds `vt_write`, `on_pty_write` goes back to the writer. Design owned by the architecture ticket. |
| G4 | **Kitty keyboard protocol impedance** (newly found) | ghostty answers `CSI ? u` queries unconditionally, so apps will enable kitty encoding that Zed's `to_esc_str` cannot produce; adopt ghostty's `key::Encoder` for key input. |
| G5 | **`clear_saved_screen` raw grid surgery** | No `grid_mut` equivalent; emulate by feeding VT sequences through `vt_write` at a chunk boundary, or accept ghostty-native clear semantics (design question for the seam ticket). |
| G6 | **Point arithmetic / `Boundary` clamping helpers** | Small seam-layer utilities over ghostty's tagged coordinate spaces (add/sub/clamp with wide-char expansion). |
| G7 | **Hyperlink OSC 8 *id*** | Not exposed by ghostty (URI only); compare adjacent cells by URI for hover extent, drop the `id` accessor (only a seam test uses it). |
| G8 | **`FairMutex` shared-lock model vs `!Send`/`!Sync` core** | Terminal owned by one thread; every cross-thread lock site (`find_matches`, `total_lines`, …) reroutes through the owner. Constraint recorded here; design owned by the architecture ticket. |

**Non-gaps the ticket suspected (refinements):**

- **OSC 52 read**: Zed *never* exposes it. PTY terminals use alacritty's default `Osc52::OnlyCopy`
  (write allowed, paste denied at `alacritty:src/term/mod.rs:1727`); display-only terminals set
  `Osc52::Disabled` (`crates/terminal/src/alacritty.rs:126`); there is no user setting. ghostty's
  write-only `CLIPBOARD_WRITE` callback (read "always ignored", `vt/terminal.h:455-456`) matches
  Zed's effective behavior exactly. `TerminalBackendEvent::ClipboardLoad` is dead code post-migration.
- **Block selection**: Zed's `SelectionType` has no `Block` variant (`crates/terminal/src/terminal.rs:148-153`);
  `SelectionRange.is_block` is always false in practice. ghostty's rectangle flag exists anyway.
- **Alt-screen reflow**: identical semantics. alacritty also skips reflow on the alt screen
  (`alacritty:src/term/mod.rs:676-678` — reflow flag is `!is_alt`), same as ghostty's documented
  `resize` behavior. No Zed behavior depends on a difference.
- Recon-1 said the seam sets "OSC52 disabled by default" — that is only true for the display-only
  config; the PTY config keeps alacritty's default `OnlyCopy` (`crates/terminal/src/alacritty.rs:131-140`).

---

## 2. The matrix

Verdicts: **C** = Covered, **P** = Partial (equivalent exists, seam must translate/verify),
**G** = Gap (nothing on the ghostty side; fill strategy required).

### A. Terminal core lifecycle & config (8 rows: 6 C, 1 P, 1 G)

| Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|
| `Term::new(config, &bounds, ZedListener)` — `crates/terminal/src/alacritty.rs:185-198` | `Terminal::new(Options{cols,rows,max_scrollback})` — `lg:terminal.rs:239-246,275`; `ghostty_terminal_new` `vt/terminal.h:1204` | C | — |
| `Config.scrolling_history` — `crates/terminal/src/alacritty.rs:119-140` | `Options.max_scrollback` (creation-time only; no runtime setter in the option enum, `vt/terminal.h:635-890`) | C | Zed also only sets it at creation; `apply_config` is invoked solely for cursor-style changes (`crates/terminal/src/terminal.rs:1824-1827`). |
| `Config.default_cursor_style` + `set_options` — `crates/terminal/src/alacritty.rs:142-151` | `set_default_cursor_style` / `set_default_cursor_blink` — `lg:terminal.rs:697,705` (runtime-mutable) | C | — |
| `Config.osc52` gate — `crates/terminal/src/alacritty.rs:126` (display-only: Disabled) + implicit `OnlyCopy` PTY default (`alacritty:src/term/mod.rs:356-381`) | `CLIPBOARD_WRITE` callback — `lg:terminal.rs:1678`, `vt/terminal.h:889`; read never forwarded (`vt/terminal.h:455-456`) | C | Register the callback for PTY terminals, skip it for display-only. Read path drops for free (see §1 non-gaps). |
| `unset_private_mode(AlternateScroll)` on new — `crates/terminal/src/alacritty.rs:193-195` | `set_mode(Mode::ALT_SCROLL, false)` — `lg:terminal.rs:457,942` | C | — |
| vte `Processor::advance(&mut *term, bytes)` (PTY thread + `write_output` `crates/terminal/src/terminal.rs:1829-1840,1415`) | `vt_write(&[u8])` — `lg:terminal.rs:315` (single ingest point, never fails) | C | Display-only path maps 1:1; PTY path moves the parse from alacritty's IO thread to the terminal's owner thread (see §4). |
| `Config.semantic_escape_chars` (implicit default `,│\`|:"' ()[]{}<>\t` — `alacritty:src/term/mod.rs:45,360`) | Per-call `with_boundary_codepoints` on word selection — `lg:selection.rs:492` | P | No global config; the seam passes alacritty's char set on every `select_word` call to preserve double-click semantics. |
| `FairMutex<Term>` shared across threads — `crates/terminal/src/alacritty.rs:51`, `crates/terminal/src/terminal.rs:1413` | None: all ghostty types are `!Send`/`!Sync` (recon-2 §3, confirmed by `PhantomData` non-Send markers throughout `lg:terminal.rs`) | G | **G8.** Single-owner-thread architecture; see §4. |

### B. Grid/cell read path (11 rows: 10 C, 1 P)

| Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|
| `renderable_content()` snapshot per frame in `make_content` — `crates/terminal/src/alacritty.rs:807-848`; fields `display_iter/selection/cursor/display_offset/mode` (`alacritty:src/term/mod.rs:2393-2400`) | `RenderState` `update` or two-phase `begin_update`+`end` — `lg:render.rs:352,381,405`; `vt/render.h` | C | ghostty is richer: global + per-row dirty flags (`lg:render.rs:444,626`) which Zed currently doesn't need (full snapshot per frame, recon-1 §6) but can adopt later. |
| `display_iter` → `Indexed<&Cell>` with grid `Point` per cell — `crates/terminal/src/alacritty.rs:810-816` | `RowIterator`/`CellIterator` lending iterators — `lg:render.rs:560-716`; viewport row index tracked by the caller, column via cell position | C | Shape differs (row-major nested vs flat indexed); `Content.cells: Vec<IndexedCell>` (`crates/terminal/src/terminal.rs:490`) is rebuilt in the seam with synthesized points. |
| `Cell.c` char + `cursor_char` — `crates/terminal/src/alacritty.rs:461-463,841` | Cell codepoint / graphemes — `lg:screen.rs:347`, `lg:render.rs:804-840`; cursor cell via `grid_ref(cursor)` (`lg:terminal.rs:378`) | C | — |
| `Cell.zerowidth()` combining chars — `crates/terminal/src/alacritty.rs:481-488` | Grapheme APIs (`graphemes_len/buf/utf8`) — `lg:render.rs:814-840`, `lg:screen.rs:94` | C | Richer: ghostty supports mode 2027 grapheme clustering (`lg:terminal.rs:955`). |
| `Cell.fg/bg: vte Color` — `crates/terminal/src/alacritty.rs:470-478`; consumed by `convert_color` (`crates/terminal_view/src/terminal_element.rs:1710`) | `Style.fg_color/bg_color: StyleColor::{None,Palette,Rgb}` — `lg:style.rs:31-79`; flattened RGB (`lg:render.rs:774-792`, INVALID_VALUE = no explicit color) | P | Seam maps `StyleColor` → vte `Color` (`None`→`Named(Foreground/Background)`, `Palette(0-15)`→`Named` ANSI variants, `Palette(16-255)`→`Indexed`, `Rgb`→`Spec`) so the renderer's theme-driven resolution and the `terminal.rs:53` re-export contract survive unchanged. Do **not** use ghostty's flattened RGB for these cells — it would bypass Zed theme colors. |
| Flags `INVERSE/DIM/BOLD/ITALIC/ALL_UNDERLINES/UNDERCURL/STRIKEOUT` — `crates/terminal/src/alacritty.rs:496-540` | `Style{inverse,faint,bold,italic,underline: Underline::{Single,Double,Curly,Dotted,Dashed},strikethrough,…}` — `lg:style.rs:31-44,122-129` | C | ghostty adds `blink/invisible/overline/underline_color` (unused today). |
| `WIDE_CHAR_SPACER`/`LEADING_WIDE_CHAR_SPACER` — `crates/terminal/src/alacritty.rs:501-503`, `crates/terminal/src/alacritty/hyperlinks.rs:23-25` | `CellWide::{Narrow,Wide,SpacerTail,SpacerHead}` — `lg:screen.rs:435-444` | C | — |
| `WRAPLINE` on last cell of row — `crates/terminal/src/alacritty.rs:943-953` | `Row::is_wrapped` / `is_wrap_continuation` row flags — `lg:screen.rs:295-299` | C | Cleaner: a row-level flag instead of a cell flag. |
| Selection membership in snapshot (`content.selection: SelectionRange`) — `crates/terminal/src/alacritty.rs:837-839` | Row-local selection range — `lg:render.rs:641`; per-cell `is_selected` — `lg:render.rs:910`; whole-selection snapshot via `GHOSTTY_TERMINAL_DATA_SELECTION` (`vt/terminal.h:1189`) | C | — |
| `bottom_row_occupied` heuristic — `crates/terminal/src/alacritty.rs:824-830` | Derived from iterated cells (pure seam logic) | C | Ports verbatim. |
| `Cell::has_visible_style_modifier` etc. — `crates/terminal/src/alacritty.rs:536-540` | Derived from `Style` + `has_styling` (`lg:render.rs:919`) | C | — |

### C. Content/text extraction (5 rows: 5 C)

| Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|
| `content_text` / `bounds_to_string` — `crates/terminal/src/alacritty.rs:850-854`; `alacritty:src/term/mod.rs:558` | `Formatter` (plain/VT/HTML, trim/unwrap options) — `lg:fmt.rs:18-130`, `vt/formatter.h` | C | Formatter's plain output replaces `bounds_to_string`; note alacritty's tab-compression quirk (hyperlinks.rs comment `crates/terminal/src/alacritty/hyperlinks.rs:335-338`) doesn't apply to cell iteration. |
| `last_non_empty_lines` (WRAPLINE-aware logical lines) — `crates/terminal/src/alacritty.rs:870-890,934-961` | Reimplement over `Row::is_wrapped` + cell iteration from the bottom | C | Straight port in the seam; used by `last_n_non_empty_lines` (`crates/terminal/src/terminal.rs:2284-2287`) for agent tooling. |
| `full_content_range` (topmost→bottommost) — `crates/terminal/src/alacritty.rs:864-868`; drives `select_all` (`crates/terminal/src/terminal.rs:1879-1881`) | `total_rows` (`lg:terminal.rs:638`) + Screen-space points; or `select_all` directly (`lg:selection.rs:218`) | C | — |
| `selection_to_string` — `crates/terminal/src/alacritty.rs:245-247`, used in `make_content` and copy (`crates/terminal/src/terminal.rs:1679-1689`) | `format_selection_alloc/buf` — `lg:selection.rs:367-431` | C | — |
| Cell-accurate line text with tab preservation for path matching — `crates/terminal/src/alacritty/hyperlinks.rs:335-368` | Row/cell iteration (codepoint + wide/spacer info) | C | This existing code is the prototype for the G1 extraction layer. |

### D. Coordinates & points (5 rows: 2 C, 2 P, 1 G)

| Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|
| `Point{Line(i32), Column}`: signed, active-screen-relative, negative lines = scrollback — `crates/terminal/src/alacritty.rs:743-754`, `crates/terminal/src/terminal.rs:438-448`; mouse mapping `crates/terminal/src/mappings/mouse.rs:194-232` | Tagged unsigned points: `Point::{Active,Viewport,Screen,History}(x:u16,y:u32)` — `lg:terminal.rs:752-825`, `vt/point.h:44-58` | P | Seam translation layer: Zed line `L` with display offset ↔ `Viewport(y = L + display_offset)` for hit-testing, `Screen(y = history_rows + L)` for stable ranges (search matches, selections). Zed's public `Point` (i32 line) can stay; only the seam converts. This underpins every other row. |
| `Boundary::{Grid,Cursor,None}` + `point.add/sub(term, boundary, n)` + `grid_clamp` — `crates/terminal/src/alacritty.rs:930`, `crates/terminal/src/alacritty/hyperlinks.rs:101,111,279,381-401`; semantics `alacritty:src/index.rs:34-45` | None — ghostty points are plain data; no arithmetic helpers | G | **G6.** Implement `add/sub/clamp` in the seam over `(cols, total_rows, viewport)` with wide-char expansion via cell `wide()` lookups; ~50 lines, all call sites are inside hyperlinks/search code that is being rewritten anyway. |
| `Direction`/`Side` (left/right half of cell) in selections — `crates/terminal/src/alacritty.rs:378-385`, `crates/terminal/src/mappings/mouse.rs:202-232` | Selection is `GridRef`-granular (`lg:selection.rs:56`); gesture API takes fractional positions (`lg:selection/gesture.rs:253`) | P | Fold the already-computed half-cell side into the chosen cell when building `GridRef` endpoints (side=Left → this cell, side=Right → next cell for start anchors); verify against the seam selection tests. |
| `Dimensions` trait impl on `TerminalBounds` — `crates/terminal/src/alacritty.rs:281-297` | Not needed: `resize(cols, rows, cell_w_px, cell_h_px)` takes plain numbers — `lg:terminal.rs:329` | C | `TerminalBounds::num_lines/num_columns` (`crates/terminal/src/terminal.rs:763-774`) feed it directly; the trait impl is deleted. |
| Point stability across scroll/reflow (alacritty: none; Zed re-derives every frame) | `TrackedGridRef` survives scroll and reflow — `lg:terminal.rs:402`, `lg:screen.rs:163-255` | C | Richer than alacritty; useful for hover word and search-match anchoring. |

### E. Scrolling & viewport (6 rows: 6 C)

| Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|
| `Scroll::{Delta,PageUp,PageDown,Top,Bottom}` → `scroll_display` — `crates/terminal/src/alacritty.rs:224-226,329-338`; `crates/terminal/src/terminal.rs:95-101,1903-1938` | `scroll_viewport(ScrollViewport::{Delta,Top,Bottom,Row})` — `lg:terminal.rs:358,841-880` | C | `PageUp/PageDown` = `Delta(±rows)` in the seam (Zed already synthesizes half-page as Delta, `crates/terminal/src/terminal.rs` scroll actions). Note sign convention: alacritty positive delta scrolls up; ghostty "up is negative" (`lg:terminal.rs:846`) — invert in the seam. |
| `display_offset()` — `crates/terminal/src/alacritty.rs:220-222` (offset-from-bottom) | `scrollbar() -> {total, offset, len}` — `lg:terminal.rs:598`, `vt/terminal.h:323-336` (offset-from-top) | C | `display_offset = total - len - offset`. Axis inversion is the only trap. |
| `scrolled_to_top` = `display_offset == history_size`; `scrolled_to_bottom` = `display_offset == 0` — `crates/terminal/src/alacritty.rs:844-845` | Derivable: top = `offset == 0`; bottom = `offset + len == total` | C | — |
| `total_lines` / `screen_lines` / `history_size` — `crates/terminal/src/alacritty.rs:856-862` | `total_rows` / `rows` / `scrollback_rows` — `lg:terminal.rs:638,561,642` | C | Feeds `terminal_scrollbar.rs` unchanged (recon-1 §3). But note G8: these are called off-thread today (`crates/terminal/src/terminal.rs:1842-1848` use `lock_unfair`). |
| `scroll_to_point` (make point visible) — `crates/terminal/src/alacritty.rs:249-251`, used by search activate (`crates/terminal/src/terminal.rs:1862`) | `scroll_viewport(Row(n))` with `n` computed from the point's Screen-space y | C | Seam math. |
| Pixel scroll accumulation / alt-scroll arrows — `crates/terminal/src/terminal.rs:1428,2614+`, `crates/terminal/src/mappings/mouse.rs` | Zed-side (unchanged); ghostty exposes `Mode::ALT_SCROLL` for the gate | C | — |

### F. Selection (7 rows: 6 C, 1 P)

| Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|
| `SelectionType::Simple` click-drag with sides — `crates/terminal/src/terminal.rs:148-153,2509`, `crates/terminal/src/alacritty.rs:387-409` | `Selection::new(start: GridRef, end: GridRef, rectangle: bool)` + `set_selection` — `lg:selection.rs:56,210`; or gesture `Behavior::Cell` (`lg:selection/gesture.rs:623-632`) | P | Recommended: keep Zed's own click-count/threshold logic (`crates/terminal/src/terminal.rs:1421,1505,2504-2527`) and drive plain `Selection` objects; the side nuance is the D-row above. The gesture API (`PressEvent`/`DragEvent`, `lg:selection/gesture.rs:226,388`) is available but duplicates behavior Zed already owns and tests. |
| `SelectionType::Semantic` (double-click word) — `crates/terminal/src/terminal.rs:2388-2398,2510`; semantics = `semantic_escape_chars` (`alacritty:src/term/mod.rs:45`) | `select_word(SelectWordOptions.with_boundary_codepoints(...))` — `lg:selection.rs:264,474-503` | C | Pass alacritty's escape-char set as boundary codepoints for behavior parity (ghostty's default word boundary differs). |
| `SelectionType::Lines` (triple-click) — `crates/terminal/src/terminal.rs:2511` | `select_line(SelectLineOptions)` — `lg:selection.rs:232,431-472` (whitespace + semantic-prompt boundary options) | C | Disable `semantic_prompt_boundary` for alacritty-equivalent whole-logical-line behavior. |
| `SelectionType::Block` | Never constructed by Zed (see §1 non-gaps); ghostty has `rectangle` flag anyway (`lg:selection.rs:56`) | C | Nothing to do; is_block plumbing (`crates/terminal/src/terminal.rs:474-479`) can stay for future use. |
| `selection.update(point, side)` drag update — `crates/terminal/src/alacritty.rs:232-243` | Rebuild `Selection` with new end `GridRef` (selections are cheap value objects), or `Selection::adjust` (`lg:selection.rs:106,600`) | C | — |
| `select_all` — `crates/terminal/src/terminal.rs:1879-1889`, `crates/terminal_view/src/terminal_view.rs:648-649` | `select_all()` — `lg:selection.rs:218` | C | — |
| Selection survival across resize (alacritty drops on column change, rotates otherwise — `alacritty:src/term/mod.rs:680-691`) | ghostty selections are grid-ref based; `OPT_SELECTION` (`vt/terminal.h:839`) holds server-side selection | C | Zed re-derives selection each interaction; minor semantic differences acceptable (selection is cleared on most structural changes today). |

### G. Regex search (4 rows: 4 G — one logical gap)

All four rows are facets of **G1**; deep-dive in §3.1.

| Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|
| `RegexSearch::new(pattern)` — `crates/terminal/src/alacritty.rs:60-62,364-376`; built from user query or `regex::escape`d literal (`crates/terminal_view/src/terminal_view.rs:1242-1244`) | None (no search API anywhere in `vt/*.h`; confirmed recon-3 §A) | G | Rust `regex` crate compile; smart-case (uppercase → case-sensitive) matching alacritty (`alacritty:src/term/search.rs:39-41`). |
| `search_matches` = `RegexIter` over the whole buffer topmost→bottommost — `crates/terminal/src/alacritty.rs:1009-1023` | None | G | Iterate logical lines (rows joined while `is_wrapped`), regex over the joined string, map byte ranges → cell ranges → Screen-space point ranges. |
| `find_matches` runs on a background thread holding the `FairMutex` — `crates/terminal/src/terminal.rs:2683-2689` | None + `!Send` terminal | G | Extract text on the owner thread (cheap; ~rows×cols chars), regex on `background_spawn` over the owned string; or incremental extraction with `regex-cursor`. |
| URL hover regex per logical line (`url_regex: RegexSearch`, `RegexIter` bounded by `line_search_left/right`) — `crates/terminal/src/alacritty/hyperlinks.rs:28,61,124-132`; wrap-aware line bounds `alacritty:src/term/search.rs:593-616` | None | G | Reuse the same logical-line extraction; the path-regex branch already works this way with the Rust `regex` crate (`crates/terminal/src/alacritty/hyperlinks.rs:29,416-467`) — extend it to URLs. |

Semantics the replacement must preserve (verified in `alacritty:src/term/search.rs`):
matches cross soft-wrapped line boundaries but never hard newlines (end-of-input at
non-WRAPLINE row ends, `:409-421`); wide-char spacers are skipped (`skip_fullwidth`, `:403`);
empty matches are discarded (`:340-364`); smart-case (`:39-41`); complexity failures degrade to
no-match with a warning (`:260-268`). Match type is `RangeInclusive<Point>` (`:21`) → Zed `Range`
(`crates/terminal/src/alacritty.rs:756-768`).

### H. Hyperlinks (4 rows: 2 C, 2 G — the regex row belongs to G1)

| Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|
| `Cell::hyperlink()` OSC 8 storage — `crates/terminal/src/alacritty.rs:54,491-493`, `crates/terminal/src/terminal.rs:317-326`; render underline + click-open (`crates/terminal_view/src/terminal_element.rs:623,1634`, `crates/terminal/src/terminal.rs:2590-2605`) | Cell `has_hyperlink` flag (`lg:screen.rs:373`, `vt/screen.h:172`) + `GridRef::hyperlink_uri` (`lg:screen.rs:116`, `vt/grid_ref.h:187`); row-level `has_hyperlink` hint (`lg:screen.rs:311`) | C | `HyperlinkData::Alacritty` variant (`crates/terminal/src/terminal.rs:322-326`) becomes `Owned{id: None, uri}` fetched via `hyperlink_uri`. |
| `Hyperlink::id()` — `crates/terminal/src/alacritty.rs:421-426` | Not exposed (URI only; swept all `vt/*.h`) | G | **G7.** Only consumers: hover-extent equality (next row) and a seam test (`crates/terminal/src/alacritty.rs:1032-1039`). Drop the accessor or return `None`. |
| Hover extent scan: expand while adjacent cells carry the *same* hyperlink (id+uri equality) — `crates/terminal/src/alacritty/hyperlinks.rs:97-122` | Walk adjacent grid refs comparing `hyperlink_uri` | C | Semantic nuance: two adjacent distinct OSC 8 links with identical URIs merge into one hover region. Harmless (same click target). |
| URL/path regex detection over rows — `crates/terminal/src/alacritty/hyperlinks.rs:22,314-481` | See G1 | G→G1 | Counted under G above; the path-regex half already uses the target design. |

### I. Vi mode (3 rows: 3 G — one logical gap)

All facets of **G2**; deep-dive in §3.2.

| Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|
| `toggle_vi_mode` / `vi_motion` / `vi_goto_point` — `crates/terminal/src/alacritty.rs:253-263`; motions dispatched: Up/Down/Left/Right/First/Last/FirstOccupied/High/Middle/Low/WordLeft/WordRight/WordRightEnd/Bracket/ParagraphUp/ParagraphDown (`crates/terminal/src/terminal.rs:2135-2151`) | None | G | Port `alacritty:src/vi_mode.rs` motion engine (16 of its 21 variants; `Semantic*` and `WordLeftEnd` unused — `alacritty:src/vi_mode.rs:15-59`). |
| `ViModeCursor::scroll` follow-on-scroll + selection tie-in — `crates/terminal/src/alacritty.rs:892-922`; `crates/terminal/src/terminal.rs:1633-1644` | None | G | Vi cursor becomes Zed-side state; scroll-follow is simple line arithmetic clamped to the viewport (`alacritty:src/term/mod.rs:694-699` shows the clamping model). |
| `TermMode::VI` flag surfaced as `Modes::VI` — `crates/terminal/src/alacritty.rs:604,697` | None | G | Synthesize in the seam from Zed's `vi_mode_enabled` (`crates/terminal/src/terminal.rs:1433,1701-1704`) — Zed already tracks it independently. |

### J. Direct grid mutations (3 rows: 1 C, 1 P, 1 G)

| Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|
| `append_text_to_term` — unsafe `term.input()`/`term.newline()` after PTY death for task summaries — `crates/terminal/src/alacritty.rs:972-1007`, called `crates/terminal/src/terminal.rs:2893` | `vt_write` of `"\r\n" + line + "\r\n"…` | C | Strict improvement: the whole unsafe/"less public API" contortion exists because alacritty's handler path desyncs grid state; ghostty's single ingest point has no such split. PTY is dead so no interleaving risk. |
| `shrink_to_used` = `grid.truncate()` (free over-allocated scrollback storage) — `crates/terminal/src/alacritty.rs:803-805`, `alacritty:src/grid/storage.rs:118-122`; callers: agent terminals after task end (`crates/acp_thread/src/acp_thread.rs:4641,4678`) | Caller-driven scrollback compression: `compress(CompressionMode)` + `compression_activity` token — `lg:terminal.rs:482-500`, `vt/terminal.h:44-53` | P | Same goal (reclaim memory when idle), different mechanism (incremental, may need repeated calls until no pending work). Wire the acp_thread call sites to a compress-until-done loop. |
| `clear_saved_screen` — `clear_screen(ClearMode::Saved)` + raw `grid_mut()` region resets + cursor-row hoist to line 0 — `crates/terminal/src/alacritty.rs:778-801` (Zed's `ctrl-l`-style Clear, `crates/terminal/src/terminal.rs:1623-1627,2089`) | No `grid_mut`; `reset()` (`lg:terminal.rs:353`) is a full RIS | G | **G5.** Deep-dive §3.3. |

### K. Modes (2 rows: 2 C)

| Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|
| `TermMode` → `Modes(u32)` with 17 flags — `crates/terminal/src/alacritty.rs:609-699`, `crates/terminal/src/terminal.rs:352-396` | Typed `mode(Mode) -> bool` getters — `lg:terminal.rs:447,915-957`; `active_screen()` (`lg:terminal.rs:602`); `is_mouse_tracking()` (`lg:terminal.rs:609`) | C | Full mapping: APP_CURSOR→`DECCKM`(1), APP_KEYPAD→`KEYPAD_KEYS`(66), SHOW_CURSOR→`CURSOR_VISIBLE`(25), LINE_WRAP→`WRAPAROUND`(7), ORIGIN→`ORIGIN`(6), INSERT→`INSERT`(ANSI 4), LINE_FEED_NEW_LINE→`LINEFEED`(ANSI 20), FOCUS_IN_OUT→`FOCUS_EVENT`(1004), ALTERNATE_SCROLL→`ALT_SCROLL`(1007), BRACKETED_PASTE→2004, SGR_MOUSE→1006, UTF8_MOUSE→1005, ALT_SCREEN→1047/`active_screen()`, MOUSE_REPORT_CLICK→`NORMAL_MOUSE`(1000), MOUSE_DRAG→`BUTTON_MOUSE`(1002), MOUSE_MOTION→`ANY_MOUSE`(1003). VI → Zed-side (§I). No unmapped flag. |
| Per-frame `Modes` snapshot in `Content` — `crates/terminal/src/alacritty.rs:834` | ~16 `mode()` calls per sync (or `ghostty_terminal_get_multi` batch, recon-3 §A) | C | Seam builds the same `Modes` bitfield; the u32 layout and all downstream consumers (`to_esc_str` `crates/terminal/src/mappings/keys.rs:47`, mouse reports, paste gate `crates/terminal/src/terminal.rs:2253`) are untouched. |

### L. Cursor (4 rows: 3 C, 1 P)

| Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|
| `RenderableCursor{shape, point}` → `Cursor` — `crates/terminal/src/alacritty.rs:724-741` | Snapshot `cursor_viewport() -> {x, y, is_wide_tail}` + `cursor_visual_style()` — `lg:render.rs:495,486,930,968-977` | C | `CursorVisualStyle::{Bar,Block,Underline,BlockHollow}` maps 1:1 onto Zed's `CursorShape` (`crates/terminal/src/terminal.rs:418-425`); wide-tail is new information Zed can use for wide-char cursor width. |
| Hidden cursor (`CursorShape::Hidden` via SHOW_CURSOR unset) — `crates/terminal/src/alacritty.rs:733-741` | `cursor_visible()` — `lg:render.rs:471`; `Option<CursorViewport>` None when off-viewport | C | — |
| Default cursor style from settings — `crates/terminal/src/alacritty.rs:265-279` | `set_default_cursor_style/blink` — `lg:terminal.rs:697-705` | C | — |
| `CursorBlinkingChange` event → `Event::BlinkChanged(term.cursor_style().blinking)` — `crates/terminal/src/alacritty.rs:314`, `crates/terminal/src/terminal.rs:1553-1556` | No change-callback; state queryable per-sync via `cursor_blinking()` (`lg:render.rs:476`) | P | Seam detects the transition during `sync()` by comparing with the previous snapshot and emits `BlinkChanged` — part of the event-model shim (§3.4). |

### M. Colors (5 rows: 3 C, 2 P)

| Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|
| `pub use vte::ansi::{Color, NamedColor, Rgb}` contract — `crates/terminal/src/terminal.rs:53`; consumers `crates/terminal_view/src/terminal_element.rs:1710` (`convert_color`), `crates/debugger_ui/src/session/running/console.rs:789,855` | vte survives as a workspace dep (`Cargo.toml:850`, recon-1 §1) — no ghostty type reaches downstream crates | C | Keep the vte color model as the domain contract; seam maps ghostty `StyleColor`→vte `Color` (row B5). Debugger console consumes `parse_ansi_text`, which stays pure-vte (row Q2). |
| OSC 4/10/11/12 dynamic set → alacritty `term.colors()[index]` override table — `crates/terminal/src/terminal.rs:1583` | Two-layer color state: embedder defaults + OSC overrides; effective/default getters for fg/bg/cursor/palette — `vt/terminal.h:114-156`, `lg:terminal.rs:647-721` | C | — |
| `ColorRequest(index, formatter)` event answered from override-or-theme (`get_color_at_index`, `crates/terminal/src/terminal.rs:1573-1586,3217`) | ghostty answers OSC color queries itself through `WRITE_PTY` (`ghostty:src/terminal/stream_terminal.zig:656,743` `colorOperation`/`writeXtermColorReport`) | P | Behavioral shift from lazy to eager: Zed must seed theme colors as defaults (`OPT_COLOR_FOREGROUND/BACKGROUND/CURSOR/PALETTE`, `vt/terminal.h:734-762`) at creation **and on every theme change**, else queries report nothing. Ordering guarantee Zed carefully preserves (`crates/terminal/src/terminal.rs:1574-1581` comment) improves: responses are emitted inline during `vt_write`, inherently ordered. |
| `to_vte_rgb` theme→vte conversion — `crates/terminal/src/mappings/colors.rs:2` | Also needed: theme→`RgbColor` for seeding defaults | C | Add a sibling conversion; keep both. |
| Color-scheme query (CSI ?996n) | `COLOR_SCHEME` callback — `lg:terminal.rs:1633`, `vt/terminal.h:695` | C | New optional capability (report light/dark); not required for parity. |

### N. Event model (13 rows: 11 C, 2 P)

Every `AlacTermEvent` variant Zed converts (`crates/terminal/src/alacritty.rs:299-321`) → ghostty:

| `TerminalBackendEvent` (from alacritty `Event`) | Zed handling | ghostty | V | Fill |
|---|---|---|---|---|
| `MouseCursorDirty` | NOOP (`crates/terminal/src/terminal.rs:1562-1564`) | — | C | Drop; pointer style already derived at render time from mode snapshot. |
| `Title(String)` | breadcrumbs (`:1516-1531`) | `TITLE_CHANGED` callback + `title()` getter — `lg:terminal.rs:1588,617` | C | — |
| `ResetTitle` | clear breadcrumbs (`:1532-1535`) | `TITLE_CHANGED` fires on title mutation; title data can be zero-length | P | Verify ghostty invokes the callback (or exposes empty title) on RIS/OSC-reset; if not, detect via `title()` comparison during sync. |
| `ClipboardStore(data)` (OSC 52 write) | write clipboard (`:1536-1538`) | `CLIPBOARD_WRITE` — `lg:terminal.rs:1678`; base64/multipart normalized by core (`vt/terminal.h:449-456`) | C | Simpler: alacritty hands Zed decoded data too; ghostty additionally handles OSC 1337 copies. |
| `ClipboardLoad(formatter)` (OSC 52 read) | answer from clipboard (`:1539-1548`) | Deliberately unsupported (`vt/terminal.h:455-456`) | C | Dead code in Zed (never fires under `OnlyCopy`/`Disabled`; see §1). Delete the variant + handler; add a release-note that OSC 52 paste stays unsupported. |
| `ColorRequest(index, fmt)` | answer from colors/theme (`:1573-1586`) | Internal via `WRITE_PTY` (row M3) | C | With seeding caveat (M3). |
| `PtyWrite(String)` (query responses: DA, DSR, DECRQM…) | forward to PTY (`:1549`) | `WRITE_PTY` / `on_pty_write(&[u8])` — `lg:terminal.rs:1536`, `vt/terminal.h:87` | C | Required registration — vim/tmux hang without it (recon-3 §B). |
| `TextAreaSizeRequest(fmt)` (CSI 14/16/18 t) | reply with bounds (`:1550-1552`) | `SIZE` callback fills `GhosttySizeReportSize` — `lg:terminal.rs:1613`, `vt/terminal.h:685` | C | — |
| `CursorBlinkingChange` | emit `BlinkChanged` (`:1553-1556`) | None | P | Seam-detect per sync (row L4). |
| `Wakeup` (damage) | `sync()` + emit (`:1565-1571`) | None needed | C | Zed's own reader loop knows when it called `vt_write`; emit Wakeup after each fed chunk (exactly what `write_output` already does, `crates/terminal/src/terminal.rs:1839`). |
| `Bell` | emit (`:1558-1560`) | `BELL` — `lg:terminal.rs:1551` | C | — |
| `Exit` | task finished (`:1561`) | n/a — comes from Zed's own PTY loop (kept) | C | Stays Zed-side with G3. |
| `ChildExit(status)` | task finished (`:1587-1589`) | n/a — same | C | Stays Zed-side with G3. |

Delivery-model note: alacritty events arrive on an unbounded channel from the IO thread
(`ZedListener`, `crates/terminal/src/alacritty.rs:323-327`; pump at
`crates/terminal/src/terminal.rs:1341-1365`); ghostty callbacks fire **synchronously inside
`vt_write`** on the owner thread (recon-2 §4). The shim: callbacks push the same
`TerminalBackendEvent` values into the existing channel/queue so `process_event`
(`crates/terminal/src/terminal.rs:1514-1591`) is untouched. See §3.4.

### O. PTY & event loop (4 rows: 4 G — one logical gap)

All facets of **G3**; deliberate libghostty non-goal ("libghostty-vt is pure state/parsing —
zero PTY/process code", recon-3 §B). The design belongs to the architecture ticket; this
confirms scope.

| Zed uses | ghostty | V | Fill |
|---|---|---|---|
| `tty::{new, Pty, Options, Shell}` spawn (unix fork/exec + Windows ConPTY) — `crates/terminal/src/alacritty.rs:48,158-183` | None | G | Port alacritty's `tty` module (`alacritty:src/tty/`) into a Zed-owned crate (it is already self-contained), or adopt an existing PTY crate; keep `pty_options` shape. |
| `EventLoop`/`Notifier`/`Msg::{Resize,Shutdown}` IO thread — `crates/terminal/src/alacritty.rs:84-108,200-214` | None | G | Zed-owned reader/writer: reader thread/task drains PTY → sends bytes to the terminal owner thread → `vt_write`; writer receives input + `on_pty_write` output; resize = `ioctl(TIOCSWINSZ)` + `terminal.resize()` (ghostling pattern, `refs/ghostling/main.c:132,1458`). |
| `SignalMask` (unix), `escape_args` (Windows), `drain_on_exit` — `crates/terminal/src/alacritty.rs:153-175` | None | G | Move with the tty port; all are tty-module features, not emulator features. |
| `ProcessIdGetter` from pty fd/`child_watcher` — `crates/terminal/src/alacritty.rs:64-82` | None | G | Moves with the tty port; `pty_info.rs` (process-table cwd/title tracking) is already alacritty-independent (recon-1 §4). |

### P. Input encoding (5 rows: 4 C, 1 G)

| Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|
| `to_esc_str(keystroke, mode, option_as_meta)` classic xterm encoding — `crates/terminal/src/mappings/keys.rs:47` (alacritty-free) | `key::Encoder` with `set_options_from_terminal` — `lg:key.rs` (full kitty/modifyOtherKeys/DEC-mode aware) | C* | Works as-is **only if** G4 is addressed; see next row. |
| **Kitty keyboard protocol**: alacritty config-gated **off** (`Config.kitty_keyboard` default false; queries ignored — `alacritty:src/term/mod.rs:350,1276-1324`); Zed never enables it, no kitty anywhere (recon-1 §3) | ghostty parses and answers `CSI ? u` progressive-enhancement queries **unconditionally** (`ghostty:src/terminal/stream_terminal.zig:292,553` `queryKittyKeyboard`); flags exposed (`lg:terminal.rs:588`); **no disable option** (`vt/terminal.h:635-890` has none) | G | **G4.** Deep-dive §3.5. Recommended: switch key input to ghostty's `key::Encoder`, which reads the terminal's kitty flags/modes and produces correct encodings in both worlds. |
| Mouse reports (X10/UTF8/SGR from `Modes`) — `crates/terminal/src/mappings/mouse.rs` | Zed-side kept; ghostty `mouse::Encoder` (`lg:mouse.rs`) optional; mode gates map (row K) | C | ghostty adds SGR-Pixels (1016) support if ever wanted. |
| Bracketed paste gate + sanitization — `crates/terminal/src/terminal.rs:2252-2260` | Zed-side kept; `paste_is_safe`/`paste_encode` available (`lg:paste.rs`) | C | — |
| Focus in/out reports — `crates/terminal/src/terminal.rs:2289-2299` | Zed-side kept; `focus_encode` available (`lg:focus.rs`) | C | — |

### Q. Parser & vte fate (3 rows: 3 C)

| Zed uses | After migration | V | Fill |
|---|---|---|---|
| `Processor<StdSyncHandler>` driving `Term` as `vte::ansi::Handler` — `crates/terminal/src/terminal.rs:52,1415` | Deleted; ghostty parses inside `vt_write` | C | The `Terminal.output_processor` field and both feed paths (event loop + `write_output`) collapse to `vt_write`. |
| Standalone `parse_ansi_text`/`strip_ansi_text` with custom vte `Handler`s — `crates/terminal/src/terminal.rs:186-315` (consumed by debugger console) | Unchanged: **vte crate survives** as a direct workspace dependency for these utilities + the `Color/NamedColor/Rgb` re-export (`crates/terminal/src/terminal.rs:53`) | C | ghostty's standalone OSC/SGR parsers (`lg:osc.rs`, `lg:sgr.rs`) are not full-stream ANSI-text handlers; keeping vte here is the cheapest correct option. |
| Seam's vte-via-alacritty imports (`ClearMode`, `CursorShape`, `CursorStyle`, `PrivateMode`, `Handler`) — `crates/terminal/src/alacritty.rs:26-34` | Replaced by ghostty equivalents / deleted with their call sites | C | — |

### R. Tests as migration oracle (1 row: 1 C)

| Suite | Transfers? |
|---|---|
| Seam contract round-trips (hyperlink storage, cell zerowidth, Modes↔TermMode, selection ranges) — `crates/terminal/src/alacritty.rs:1025-1104` | Yes — rewrite the alacritty-typed halves against ghostty types; the Zed-domain assertions stay. |
| Hyperlink grid hit-testing (builds a real `Term<VoidListener>` and feeds it) — `crates/terminal/src/alacritty/hyperlinks.rs:483+` | Yes — replace Term construction with `Terminal::new` + `vt_write`; the assertions (match text/ranges) are backend-agnostic. Best behavioral oracle for G1. |
| Key/mouse encoding suites — `crates/terminal/src/mappings/keys.rs:262-397`, `crates/terminal/src/mappings/mouse.rs:100-137` | Unchanged (alacritty-free); become the spec for the G4 encoder switch. |
| `write_output`/CRLF/OSC52-display-only gpui tests — `crates/terminal/src/terminal.rs:4195-4433` | Yes — already drive the public `write_output` seam. |
| Batching/path-target/agent integration — `crates/terminal_view/src/terminal_element.rs:2078-2159`, `crates/terminal_view/src/terminal_path_like_target.rs:206-1030`, `crates/agent/…/terminal_tool.rs:1932-2239` | Unchanged (domain types only). |

---

## 3. Gap deep-dives

### 3.1 G1 — Regex search over the grid

**Full surface** (all call sites):

1. Find-in-terminal: `Search::new` (`crates/terminal/src/alacritty.rs:364-376`) from
   `terminal_view`'s `SearchableItem` (`crates/terminal_view/src/terminal_view.rs:1242-1244` —
   regex mode passes the raw query, text mode passes `regex::escape`d); `find_matches` →
   `search_matches` → `RegexIter` over `topmost..bottommost` (`crates/terminal/src/alacritty.rs:1009-1023`)
   producing `Vec<Range>` stored in `Terminal.matches` (`crates/terminal/src/terminal.rs:1422`),
   highlighted by the element and activated via selection+scroll (`crates/terminal/src/terminal.rs:1854-1863`).
2. URL hover: `RegexSearches.url_regex` (`crates/terminal/src/alacritty/hyperlinks.rs:28,61`)
   searched per logical line bounded by `line_search_left/right` (`:124-132`).
3. Path hover already uses plain Rust `regex` over seam-extracted line text (`:29,416-467`) —
   the design to generalize.

**Semantics to preserve**: §2.G table note (soft-wrap crossing, hard-newline barrier,
wide-spacer skip, smart-case, empty-match discard, degrade-on-complexity). Matches are
inclusive point ranges in stable (Screen-space) coordinates that survive scrolling; a resize
invalidates them (`crates/terminal/src/terminal.rs:1616-1621`).

**Strategy**: a Zed-owned `grid_search` layer in the seam:
extract logical lines (join rows while `Row::is_wrapped`, `lg:screen.rs:295`) into `String`s
with a per-line `Vec<(byte_offset, cell_x, row_y)>` map that accounts for `SpacerTail/SpacerHead`
(`lg:screen.rs:435-444`) and multi-char graphemes (`lg:render.rs:804`); run `regex::Regex`
(smart-case built at compile time) per logical line — this makes the hard-newline barrier
structural instead of simulated. Byte ranges map back through the offset table to Screen-space
`Range`s.

**Threading**: today `find_matches` locks the FairMutex on a background thread
(`crates/terminal/src/terminal.rs:2683-2689`). Post-migration: extract text on the owner thread
(bounded by scrollback size; Zed default 10k lines ≈ a few MB worst case), then regex on
`cx.background_spawn` over the owned buffer.

**Open questions for the search-engine ticket**: incremental extraction/caching keyed on
ghostty dirty flags; cap on scanned history for interactive latency; whether hover search
should reuse the whole-buffer index or stay per-line (recommended: per-line, as today).

### 3.2 G2 — Vi mode

Zed dispatches exactly 16 motions (`crates/terminal/src/terminal.rs:2135-2151`):
`Left/Down/Up/Right/WordRight/WordLeft/WordRightEnd/Bracket/Last/First/FirstOccupied/High/Middle/Low/ParagraphUp/ParagraphDown`.
Unused alacritty variants: `SemanticLeft/SemanticRight/SemanticLeftEnd/SemanticRightEnd/WordLeftEnd`
(`alacritty:src/vi_mode.rs:15-59`) — the port is scoped to whitespace-word motions (no
semantic-escape-char dependency), `bracket_search` (`alacritty:src/vi_mode.rs:159`),
first-occupied, screen-position (H/M/L), and paragraph jumps.

Supporting behavior to port: scroll-follow (`update_vi_cursor_for_scroll`,
`crates/terminal/src/alacritty.rs:892-914` — Delta moves the cursor by the scroll amount,
Top/Bottom snap it), selection-head tie-in (`update_selection_to_vi_cursor`, `:916-922`),
`vi_goto_point` for search-match navigation (`:253-255`), and viewport clamping after resize.

**Strategy**: `vi_cursor: Option<Point>` state on the seam terminal; a `vi_motion(motion)`
function needing only: cell read at point (char + wide flag), row wrapped flags, cols/rows,
topmost/bottommost, and display offset — all available via `grid_ref`/rows. `Modes::VI` is
synthesized from Zed's own `vi_mode_enabled`. The vi cursor renders as the block cursor when
active (alacritty swaps `RenderableCursor` to the vi point; seam does the same in `make_content`).

**Open question**: whether to keep alacritty's exact whitespace-word semantics or reuse the
G1 logical-line extraction for word hops (recommend exact port first, refactor later).

### 3.3 G5 — `clear_saved_screen`

What it does today (`crates/terminal/src/alacritty.rs:778-801`): clear scrollback
(`ClearMode::Saved`), reset all rows above the cursor row, copy the cursor row to line 0, move
the cursor to line 0, reset everything below — i.e. "clear everything but keep the current
prompt line at the top". Triggered by the `Clear` action (`crates/terminal/src/terminal.rs:1623-1627,2089`).

ghostty has no raw grid mutation. Options, in preference order:

1. **VT-sequence emulation via `vt_write`**: feed `CSI 2J` variants? No — that erases the prompt
   line too. The faithful sequence set is: cursor-save, `CSI H` region scroll tricks — fragile.
   More robust: `ESC[3J` (erase scrollback) is honored by ghostty; combine with scroll-up by
   `cursor_y` lines (`CSI <n> S`) + `CUP 1;<col>` to hoist the prompt row, then `CSI 0J` to clear
   below. Must be injected at a chunk boundary of the reader loop (parser state persists across
   `vt_write` calls, so injecting mid-escape-sequence would corrupt parsing — the owner-thread
   loop makes "between chunks" a well-defined point, unlike today's mutex interleaving).
2. **Accept ghostty-native semantics**: clear = `ESC[H ESC[2J ESC[3J` (what most terminals send
   for clear), losing the "keep prompt line" nicety.

Open question for the seam ticket: pick 1 vs 2 and add a seam test asserting the prompt-line
behavior either way (none exists today).

### 3.4 G8 / event-model impedance (architecture-ticket constraints)

- ghostty `Terminal`, `RenderState`, iterators are `!Send`/`!Sync`; docs recommend a dedicated
  thread + channels (recon-2 §3). `FairMutex` disappears.
- Cross-thread call sites that must reroute through the owner: `find_matches`
  (`crates/terminal/src/terminal.rs:2683`), `total_lines`/`viewport_lines` (`:1842-1848`),
  `get_content`/`last_n_non_empty_lines` (`:2279-2287`), `with_renderable_cells` (`:2273-2277`),
  and the render pull `sync`/`make_content` (`:2262-2271`).
- The two-phase `RenderState` split (`begin_update` touches the terminal, `end` only render-state
  memory — `lg:render.rs:381-405`, `vt/render.h`) exists precisely for an IO-thread/render-thread
  split and is the intended replacement for "renderer reads under FairMutex".
- Callbacks fire synchronously inside `vt_write`; they must not re-enter the terminal mutably.
  Shim: callbacks only enqueue `TerminalBackendEvent`s (and copy out data), preserving Zed's
  existing async `process_event` pipeline (`crates/terminal/src/terminal.rs:1508-1591`) unchanged.
- `Event::Wakeup` is replaced by the reader loop emitting after each `vt_write` batch —
  identical to the existing display-only pattern (`crates/terminal/src/terminal.rs:1829-1840`).

### 3.5 G4 — Kitty keyboard protocol (new finding, not in the ticket)

Today: alacritty's kitty support is compile-present but config-gated, and Zed leaves
`Config.kitty_keyboard = false`, so `CSI ? u` / push/pop-flags queries are ignored
(`alacritty:src/term/mod.rs:1276-1324`) and applications silently fall back to legacy encoding —
which is all `to_esc_str` speaks (`crates/terminal/src/mappings/keys.rs`, "no kitty protocol",
recon-1 §2e).

After migration: ghostty answers the kitty query unconditionally
(`ghostty:src/terminal/stream_terminal.zig:292,553`) via `WRITE_PTY`, and there is no option to
disable it (`vt/terminal.h:635-890`). Neovim, kitty-aware CLIs, fish 4.x etc. will detect
support, push enhancement flags (visible via `kitty_keyboard_flags()`, `lg:terminal.rs:588`),
and then expect kitty-encoded key events that Zed's classic encoder never produces → broken
keyboard input in those apps.

**Fill**: switch keyboard input to ghostty's `key::Encoder`
(`lg:key.rs` — `set_options_from_terminal` syncs cursor/keypad modes and kitty flags each use;
the ghostling embedders demonstrate the loop, recon-2 §5). This is a *feature win* (kitty
protocol support lands for free) but is **mandatory, not optional**, and needs its own mapping
work (gpui `Keystroke` → ghostty `KeyEvent`, `option_as_meta` handling) plus porting the
`keys.rs:262-397` test suite as legacy-mode conformance. Alternative (requesting an upstream
disable option) leaves Zed on the old behavior but adds an upstream dependency; not recommended.

### 3.6 G7 — Hyperlink id

`ghostty_grid_ref_hyperlink_uri` returns only the URI (`vt/grid_ref.h:168-187`); no id accessor
exists in any header. Zed's uses: hover-extent equality on adjacent cells
(`crates/terminal/src/alacritty/hyperlinks.rs:98-122`, compares the full `Hyperlink` including
id) and the `Hyperlink::id()` accessor whose only real caller is a seam test
(`crates/terminal/src/alacritty.rs:1032-1039`); no product feature reads the id
(swept `crates/`: only `terminal_element.rs:623,1634` presence checks and `terminal.rs:2598`
URI open). **Fill**: extent scan compares URIs; `HyperlinkData::Owned.id` stays `None`; delete or
repurpose the test. Edge case accepted: two adjacent distinct links with identical URIs merge
into one hover underline region.

---

## 4. Cross-cutting constraints (not per-row)

1. **Threading model** — see §3.4. The single biggest structural change; every other row assumes
   the owner-thread architecture is settled first (dedicated ticket).
2. **Coordinate-space translation** — one seam module should own all conversions between Zed's
   signed active-relative `Point` (`line: i32`, negative = scrollback) and ghostty's four tagged
   unsigned spaces (`vt/point.h:44-58`): Viewport for hit-testing/rendering, Screen for stable
   ranges, Active for cursor math. Get this wrong and selection, search highlighting, and hover
   all break subtly; the round-trip helpers `grid_ref(point)`/`point_from_grid_ref`
   (`lg:terminal.rs:378,428`) are the canonical converters.
3. **Pre-1.0 API churn** — `vt.h` is explicitly WIP ("definitely going to change") while the
   engine is production-proven (recon-3 §Stability); libghostty-rs tracks a pinned ghostty commit
   (`GHOSTTY_COMMIT`, recon-2 §2,§6) and is itself pre-1.0 with thin binding tests (11 total).
   Vendoring means owning coordinated bumps of commit + bindings; Zed's seam tests (§2.R) are the
   regression net. Also resolve the libghostty-rs license discrepancy (MIT file vs
   `MIT OR Apache-2.0` manifest, recon-2 §1) before vendoring.
4. **Build chain** — Zig 0.15.x + git/network by default; `GHOSTTY_SOURCE_DIR` /
   `GHOSTTY_ZIG_SYSTEM_DIR` enable hermetic builds; fat static `libghostty-vt.a` needs only libc
   (recon-2 §2, recon-3 §Build). Not a parity issue but gates CI.
5. **Query-answering defaults** — by default ghostty *silently drops* sequences that need
   responses (`vt/terminal.h:57-62`); Zed must register `WRITE_PTY` (mandatory), `SIZE`,
   `DEVICE_ATTRIBUTES`, `XTVERSION`, `TITLE_CHANGED`, `BELL`, `CLIPBOARD_WRITE` (PTY terminals
   only), and seed default colors — the display-only terminal registers a subset (no clipboard),
   replacing today's `Osc52::Disabled` config split.
6. **Feature deltas to gate deliberately** — ghostty brings kitty graphics (storage-limit option,
   `vt/terminal.h:775`; disable with limit 0 until Zed renders images), OSC 133 semantic
   prompts/`select_output` (`lg:selection.rs:248`), in-band resize (mode 2048), and sync output
   (mode 2026). None are required for parity; each is a candidate follow-up.
