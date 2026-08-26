# libghostty-vt feature-parity matrix (v2)

Status: **Decided for v2** (2026-08-25, ticket [#29](https://github.com/xipeng-jin/zed/issues/29), map [#27](https://github.com/xipeng-jin/zed/issues/27)).
Supersedes: the v1 record of the same name on `migration/libghostty` (resolved 2026-07-15, last at `e537270dac`, verified against ghostty `a887df42c5`). Re-validated in full at the v2 baseline (`migration/libghostty2` @ `38c5dd7c98`, ghostty `8867c37c5`, libghostty-rs `de9fd9b0fa`); §0 lists every row added, dropped, or re-adjudicated so the diff against v1 is legible.

Authoritative mapping of everything Zed's terminal uses from `alacritty_terminal` (and `vte`)
onto what libghostty-vt (C API at `ghostty/include/ghostty/vt*.h`) and the libghostty-rs safe
bindings (`libghostty-rs/crates/libghostty-vt/src/`) provide, with a fill strategy for every gap.

Sources of truth read for this matrix:

- Zed seam (branch `migration/libghostty2` = `main` @ `38c5dd7c98`): `crates/terminal/src/alacritty.rs`
  (1205 lines), `crates/terminal/src/alacritty/hyperlinks.rs`, `crates/terminal/src/terminal.rs`
  (5851 lines), `crates/terminal/src/mappings/{keys,mouse,colors}.rs`,
  `crates/terminal_view/src/terminal_element.rs`, `crates/terminal_view/src/terminal_view.rs`,
  `crates/debugger_ui/src/session/running/console.rs`, `crates/acp_thread/src/acp_thread.rs`.
- alacritty_terminal (Zed fork, rev `4c129667ce` — unchanged since v1, `Cargo.toml:519`;
  checkout `~/.cargo/git/checkouts/alacritty-20195d12a03fa0c5/4c12966/alacritty_terminal/`), cited as
  `alacritty:<path>`. `vte 0.15.0` unchanged (`Cargo.toml:873`).
- libghostty-vt C headers at ghostty **`8867c37c5`** (2026-08-24; `~/Projects/refs/ghostty/include/ghostty/vt/*.h`,
  cited as `vt/<header>`), ghostty core (`~/Projects/refs/ghostty/src/terminal/`, cited as `ghostty:<path>`).
- libghostty-rs **`de9fd9b0fa`** (2026-08-18; `~/Projects/refs/libghostty-rs/crates/libghostty-vt/src/`, cited as
  `lg:<file>`). **Caveat (§4.3):** this crate binds ghostty `22d13172cd`, not `8867c37c5`; `lg:` citations
  are given where the wrapper exists, and rows note where only the C header carries the counterpart.

Citations are `file:line`. Every `Zed uses` claim was verified against the code on branch
`migration/libghostty2` @ `38c5dd7c98` (2026-08-25); every ghostty claim against `8867c37c5`; every
libghostty-rs claim against `de9fd9b0fa`. Claims not verified against code are marked **unverified**.

---

## 0. Changed rows since v1

v1 had 93 rows (65 C / 11 P / 17 G → G1–G8). v2 has **100 rows (70 C / 13 P / 17 G → G1–G8)**. (v1's stated tally double-counted section M as "3 C, 2 P"; its table has one P row, so the true v1 split was 66 C / 10 P / 17 G.)
No v1 row was dropped: every alacritty item v1 recorded is still used at `38c5dd7c98`. No row changed
its status letter; five rows changed their *nature* (what the counterpart is and what the fill does)
because the ghostty API delta moved under them. The divergence-ledger re-fire list this section
refers to is §5.

### 0.1 Added rows (7)

| Row | Zed item (new since v1) | Upstream PR | v2 status | Why it is a row |
|---|---|---|---|---|
| A9 | Two config profiles: `pty_term_config` vs `display_only_term_config` — `crates/terminal/src/alacritty.rs:120-143`; constructed at `crates/terminal/src/terminal.rs:970,1156` | (existed in v1 code, folded into A4 then) | C | Split out because the ghostty side maps it to *which effects are registered*, not to a config struct; A4's OSC 52 flip made the split load-bearing. |
| B12 | Raw grid-shape reads `Grid::{total_lines,columns,screen_lines,display_offset}` + `Dimensions` trait import — `crates/terminal/src/alacritty.rs:12,827-842,899-916`, `crates/terminal/src/terminal.rs:18,494-497` | #54884 `b41505358f` | C | New backend capability feeding `Content.{total_lines,columns,screen_lines}` every frame. |
| B13 | `GridLinesChange` + `adjusted_last_hovered_word` — `crates/terminal/src/alacritty.rs:823-880`, `crates/terminal/src/terminal.rs:504-515,2343-2352` | #54884 | C | "Viewport shifted by N while content unchanged" is derived purely from B12 reads; ghostty's `TrackedGridRef` is a richer native alternative. |
| B14 | `Terminal::used_lines()` — alt-screen-aware `Row::is_clear` scan — `crates/terminal/src/alacritty.rs:810-821`, `crates/terminal/src/terminal.rs:1918-1920`; consumer `crates/terminal_view/src/terminal_view.rs:355` | #62504 `1c9cbd3b24` | P | ghostty has no row-emptiness flag; needs a cell scan (reuse of the v1 P7-010 trailing-blank-row counter). |
| B15 | Per-line cwd history: `history_size()` + raw `grid().cursor.point.line` reads, `CwdHistoryEntry`, `cwd_at_line`, `scrollback_position` — `crates/terminal/src/terminal.rs:1497-1500,1781,1792,2170-2176,2841-2890` | #52454 `184e124bba` | P | Reads are covered (`scrollback_rows`, `cursor_y`); the scrollback-cap heuristic at `:2869-2875` assumes exact line-count eviction and does not transfer to page-granular pruning. |
| H5 | `clear_hyperlink` + `HoveredWord.id` throttle — `crates/terminal/src/terminal.rs:518-522,1841-1850`; `crates/terminal_view/src/terminal_element.rs:1368-1372` | #54884 | C | Zed-owned; listed so the hover pipeline row set is complete. |
| P3 | ctrl+alt+letter → `ESC <ctrl-code>`, alt+shift+letter → `ESC <UPPER>` — `crates/terminal/src/mappings/keys.rs:213-231`, test `:388-404` | #62891 `2bf9e26473` | P | Adds rows to the G4 encoder-conformance oracle; ghostty's legacy encoder output for these combos is **unverified** here (v1 ledger P3-010 precedent). |

Also new but **not parity rows** (Zed does not use them; adjudicated in §6): `CURSOR_AT_PROMPT`,
in-band resize (mode 2048), `TITLE_REPORT`, `UNKNOWN_SEQUENCE`, `TERMINFO_NAME`, desktop-notification
and progress callbacks, `vt_write_until_ground`/`VT_GROUND`, snapshot encode/decode.

### 0.2 Dropped rows

None. Checked: every `alacritty_terminal::`/`vte::` path in `crates/terminal/src/alacritty.rs:9-34`,
`crates/terminal/src/alacritty/hyperlinks.rs:1-10,489-494,1144-1149`, `crates/terminal/src/terminal.rs:18,53-54`,
`crates/terminal/src/mappings/colors.rs:2` has a row; `crates/terminal_view` and `mappings/{keys,mouse}.rs`
have zero alacritty references.

### 0.3 Re-adjudicated rows (status letter unchanged, nature changed)

| Row | v1 → v2 | Why | Citations |
|---|---|---|---|
| A4 / N-ClipboardLoad | C ("read structurally unsupported; delete `ClipboardLoad` handler; release-note OSC 52 paste unsupported") → C ("read **available**, left **unwired by policy**; keep the handler variant") | `OPT_CLIPBOARD_READ` (opt 38) now exists; NULL (default) ignores OSC 52 `?` reads, which is byte-for-byte what alacritty `Osc52::OnlyCopy` does (no reply). Zed's effective policy is unchanged: PTY terminals `OnlyCopy`, display-only `Disabled`, no user setting. Not a regression, not a feature: a switch Zed chooses not to flip. | `vt/terminal.h:1515-1524,727-818`; `alacritty:src/term/mod.rs:1727`; `crates/terminal/src/alacritty.rs:128,133-143`; `crates/terminal/src/terminal.rs:1588-1597` |
| A2 | C (lines→bytes page conversion, ledger P5-001) → C (native `SCROLLBACK_MAX_LINES`) | `OPT_SCROLLBACK_MAX_LINES` (opt 28) + `set_scrollback_max_lines`; page-granular pruning still over-retains "dozens to a hundred or so lines". P5-001 retires; its over-retention waiver survives as a new entry. | `vt/terminal.h:1380-1402`; `lg:terminal.rs:779-806`; `ghostty:src/terminal/PageList.zig:570-593,632-633` |
| N-ClipboardStore | C (plain data callback) → C (sized request + synchronous `reply`) | `GhosttyClipboardWrite` carries MIME representations, location, program name, `granted`/`can_remember`, and must be answered via `write->reply` before returning; OSC 52 discards the reply. The seam picks the first `text/*` representation. libghostty-rs `on_clipboard_write` still has the pre-reply shape (§4.3). | `vt/terminal.h:445-624`; `lg:terminal.rs:2037-2051` |
| K1/K2 | C (typed `mode_get/set`) → C (`DATA_MODE`/`OPT_MODE`/`OPT_MODE_DEFAULT` via `GhosttyTerminalModeConfig`) | Same mapping table; libghostty-rs already re-plumbed `mode()/set_mode()` onto it and added `set_default_mode` (RIS-stable defaults — lets `AlternateScroll::Off` survive a reset, which alacritty's `unset_private_mode` does not guarantee — **unverified** on the alacritty side). | `vt/terminal.h:1068-1083,1453-1476,1909-1918`; `lg:terminal.rs:433-490` |
| P5 (paste) | C (Zed-side kept; `paste_is_safe/encode` available) → C (Zed-side kept **or** adopt the full `ghostty_terminal_paste` path — decision for #32) | New terminal-state-aware paste: mode-2004 framing, unsafe-paste `GHOSTTY_REJECTED` + `allow_unsafe` retry, streaming via `WRITE_PTY`, Kitty paste events (mode 5522) when a clipboard-read callback is installed. Sanitization differs from Zed's (`text.replace('\x1b', "")` in bracketed mode vs replacing every unsafe control byte with a space). | `vt/paste.h:15-56,103-190`; `vt/types.h:108`; `crates/terminal/src/terminal.rs:2324-2332`; v1 ledger P3-007 |

Strategy amendments without a row change: **G5** gains a precise injection point (`vt_write_until_ground`
+ `DATA_VT_GROUND`, §3.3); **G8** is reinforced by the C API's single-threaded runtime (§3.4); **G1**, **G4**,
**G7** re-confirmed unchanged (§3.1, §3.5, §3.6); **G2**, **G3**, **G6** unchanged.

---

## 1. Verdict summary

**Row tally: 100 rows — 70 Covered, 13 Partial, 17 Gap rows, and the 17 gap rows collapse into the
same 8 logical gaps (G1–G8 below).** v1 → v2 delta against the corrected v1 split (66 / 10 / 17): +7 rows
(+4 C, +3 P, +0 G), 0 dropped, 0 status flips.

The libghostty-vt core still covers the entire grid/state/parsing surface Zed needs, and the 918-commit
delta *widened* the covered surface (native scrollback-lines, clipboard read, full paste path, dirty-row
iteration, bulk raw-cell reads, ground-state detection, semantic-prompt state) without closing any of the
eight logical gaps: none of them was ever an emulator feature. The four new Zed capabilities land on
existing ghostty reads (B12/B13/B15) or on a small seam scan (B14).

| # | Gap | One-line fill strategy (v2) |
|---|-----|------------------------|
| G1 | **Regex search over the grid** (find-in-terminal + URL hover) | Unchanged. Zed-owned engine: extract logical-line text via ghostty row/cell iteration (WRAPLINE-aware, spacer/grapheme-aware byte↔cell map), Rust `regex` over it; extraction on the owner thread, matching on a background thread. Re-confirmed: no `search.h`, no search export (`ghostty:src/lib_vt.zig:65` is a Zig module re-export; nothing under `src/terminal/c/`). |
| G2 | **Vi mode** (`ViModeCursor`/`ViMotion`) | Unchanged. Port alacritty `vi_mode.rs` into Zed-owned code over seam grid reads; only the 16 motions Zed dispatches; `Modes::VI` is a Zed-side flag. |
| G3 | **PTY + event loop** (`tty`, `EventLoop`, `Notifier`) | Unchanged. Deliberate ghostty non-goal; Zed-owned PTY crate, reader task feeds `vt_write`, `on_pty_write` back to the writer. Design owned by the architecture ticket. |
| G4 | **Kitty keyboard protocol impedance** | Unchanged and re-confirmed: ghostty still answers `CSI ? u` unconditionally (`ghostty:src/terminal/stream_terminal.zig:492,1516-1523`) and none of options 0–39 disables it (`vt/terminal.h:1093-1549`). Adopt ghostty's `key::Encoder`; P3's new ctrl+alt/alt+shift rows join the conformance oracle. |
| G5 | **`clear_saved_screen` raw grid surgery** | Amended. Still no `grid_mut`; VT-sequence emulation is now injectable at a *provable* stream boundary using `ghostty_terminal_vt_write_until_ground` / `DATA_VT_GROUND` (`vt/terminal.h:2080-2110,1920-1933`) instead of "hope the chunk ended cleanly". Pick emulation vs ghostty-native clear in the seam ticket. |
| G6 | **Point arithmetic / `Boundary` clamping helpers** | Unchanged. ~50-line seam utility over ghostty's tagged coordinate spaces. |
| G7 | **Hyperlink OSC 8 *id*** | Unchanged and re-confirmed: `vt/grid_ref.h:168-187` exposes URI only; `vt/screen.h:192-196,291-295` expose presence flags only; swept all `vt/*.h` for `hyperlink` — no id accessor. Compare adjacent cells by URI; keep `id: None`. |
| G8 | **`FairMutex` shared-lock model vs `!Send`/`!Sync` core** | Amended. The C API now explicitly runs single-threaded (`init_single_threaded`, `TinyIo`: `ghostty:src/lib_vt.zig:45-50`, `ghostty:src/terminal/c/terminal.zig:56-58`) and documents that the caller "must serialize it with writes, rendering, searches" (`vt/terminal.h:2260`); libghostty-rs is still `!Send`/`!Sync`. Single-owner-thread architecture stands; every cross-thread lock site reroutes through the owner. |

**Non-gaps the ticket suspected (v2 refinements):**

- **OSC 52 read**: no longer "structurally impossible" — see §0.3. Zed's effective policy stays
  write-only; the read callback is left NULL. ~~`TerminalBackendEvent::ClipboardLoad` stays in the enum~~
  **Superseded by #31 (2026-08-26):** the variant is deleted — a ghostty read is a synchronous callback
  that must reply before returning, so a future setting wires a seam callback, not an event
  ([color-and-clipboard-contract.md §5](color-and-clipboard-contract.md)).
- **Block selection**: unchanged — Zed's `SelectionType` has no `Block` variant
  (`crates/terminal/src/terminal.rs:151-156`); `SelectionRange.is_block` is always false in practice.
- **Alt-screen reflow**: unchanged — alacritty skips reflow on the alt screen
  (`alacritty:src/term/mod.rs:676-678`); ghostty documents the same (`vt/terminal.h:2005-2007`).
- **Shift+drag under mouse tracking** (#60880, `crates/terminal/src/terminal.rs:2615-2626`): Zed policy
  over `mouse_mode(shift)`; no backend involvement; migrates unchanged.
- **Scrollback units** (v1 ledger P5-001): a lines API now exists natively; the remaining delta is
  page-granular over-retention, which is a documented ghostty property, not a seam conversion.

---

## 2. The matrix

Verdicts: **C** = Covered, **P** = Partial (equivalent exists, seam must translate/verify),
**G** = Gap (nothing on the ghostty side; fill strategy required). Rows are numbered per section so §0
and §5 can refer to them; v1 row order is preserved, new rows are appended to their section.

### A. Terminal core lifecycle & config (9 rows: 7 C, 1 P, 1 G)

| # | Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|---|
| A1 | `Term::new(config, &bounds, ZedListener)` — `crates/terminal/src/alacritty.rs:188-201` | `Terminal::new(cols, rows)` — `lg:terminal.rs:252`; `ghostty_terminal_new(alloc, out, cols, rows)` — `vt/terminal.h:1972-1975` (`GhosttyTerminalOptions` removed; every option is set post-construction via `ghostty_terminal_set`, `:1960-1962`) | C | Constructor no longer takes scrollback; A2 sets it immediately after. |
| A2 | `Config.scrolling_history` — `crates/terminal/src/alacritty.rs:121,125,134,138`; cap check reads the config value at `crates/terminal/src/terminal.rs:2874` | `OPT_SCROLLBACK_MAX_LINES` (opt 28, runtime-settable, NULL = unlimited) — `vt/terminal.h:1380-1402`; `set_scrollback_max_lines(Option<usize>)` — `lg:terminal.rs:801`; `DATA_SCROLLBACK_MAX_LINES` — `vt/terminal.h:1887-1896`. Byte limit `OPT_SCROLLBACK_MAX_BYTES` (opt 27, `:1358-1378`) coexists, first-reached wins (`:1366-1370`) | C | Set lines = `scrolling_history`, bytes = NULL (unlimited) so only the line limit applies; zero → set bytes = 0 ("disables scrollback and erases retained history", `:1373`). Page-granular pruning over-retains (`:1383-1389`) — ledger re-fire §5. `apply_config` is still only invoked for cursor-style changes (`crates/terminal/src/terminal.rs:1892-1895`). |
| A3 | `Config.default_cursor_style` + `set_options` — `crates/terminal/src/alacritty.rs:145-154,268-282` | `set_default_cursor_style` / `set_default_cursor_blink` — `lg:terminal.rs:964,972`; `OPT_DEFAULT_CURSOR_STYLE/BLINK` — `vt/terminal.h:1314,1324` | C | — |
| A4 | `Config.osc52` gate — display-only `Osc52::Disabled` (`crates/terminal/src/alacritty.rs:128`), PTY implicit `OnlyCopy` default (`alacritty:src/term/mod.rs:372-381`; paste denied at `:1727`, copy allowed at `:1706`) | `OPT_CLIPBOARD_WRITE` (opt 26) — `vt/terminal.h:1356`, `lg:terminal.rs:2037`; `OPT_CLIPBOARD_READ` (opt 38, default NULL = "ignore OSC 52 read requests and refuse OSC 5522 reads with EPERM") — `vt/terminal.h:1515-1524` | C | **Re-adjudicated (§0.3).** PTY profile: register write, leave read NULL = `OnlyCopy`. Display-only profile: register neither = `Disabled`. Read stays unwired by policy; wiring it later is one callback that calls `cx.read_from_clipboard()` synchronously (the callback contract is synchronous, `:785-791`). |
| A5 | `unset_private_mode(AlternateScroll)` on new — `crates/terminal/src/alacritty.rs:196-198` | `set_mode(Mode::ALT_SCROLL, false)` — `lg:terminal.rs:453,1220` via `OPT_MODE` (`vt/terminal.h:1468-1476`); or `set_default_mode` (`lg:terminal.rs:478`, `OPT_MODE_DEFAULT` `:1453-1466`) so the value also survives RIS | C | Use `set_default_mode` — strictly better than alacritty (whether alacritty re-enables 1007 on RIS is **unverified**). |
| A6 | vte `Processor::advance(&mut *term, bytes)` — PTY thread and `write_output` (`crates/terminal/src/terminal.rs:1897-1908,1452`) | `vt_write(&[u8])` — `lg:terminal.rs:301`; `ghostty_terminal_vt_write` — `vt/terminal.h:2076`; plus `ghostty_terminal_vt_write_until_ground(term, data, len, &consumed)` + `DATA_VT_GROUND` — `vt/terminal.h:2080-2110,1920-1933` (no `lg:` wrapper) | C | Display-only path maps 1:1; PTY path moves parsing to the owner thread (§3.4). `until_ground` is the G5 injection primitive (§3.3). |
| A7 | `Config.semantic_escape_chars = format!("{SEMANTIC_ESCAPE_CHARS}─")` — `crates/terminal/src/alacritty.rs:20,127,140`; constant `alacritty:src/term/mod.rs:45` (`,│\`|:"' ()[]{}<>\t`); pinned by test `crates/terminal/src/alacritty.rs:1188-1204` | Per-call `with_boundary_codepoints` on `select_word` — `lg:selection.rs:506`; `vt/selection.h:123-140` (NULL = ghostty defaults) | P | No global config; the seam owns a copy of alacritty's constant **plus `─`** and passes it on every `select_word`. The constant import is a leak that becomes a Zed-owned `const`. |
| A8 | `FairMutex<Term>` shared across threads — `crates/terminal/src/alacritty.rs:18,52,200`, `crates/terminal/src/terminal.rs:1452-1460`; off-thread lock sites `:1910-1920,2355-2369,2793-2799` | None: all ghostty types are `!Send`/`!Sync`; C runtime is single-threaded by construction (`ghostty:src/lib_vt.zig:45-50`, `ghostty:src/terminal/c/terminal.zig:56-58`); `vt/terminal.h:2260` | G | **G8.** §3.4. |
| A9 | Two config profiles — `display_only_term_config` (`crates/terminal/src/alacritty.rs:120-131`) vs `pty_term_config` (`:133-143`); constructed at `crates/terminal/src/terminal.rs:970` (display-only, `write_output` only) and `:1156` (PTY) | One `Terminal` type; the profile is the set of registered effects (`vt/terminal.h:55-102`) + default colors | C | PTY profile registers `WRITE_PTY`, `BELL`, `TITLE_CHANGED`, `SIZE`, `DEVICE_ATTRIBUTES`, `XTVERSION`, `CLIPBOARD_WRITE`; display-only registers `BELL`/`TITLE_CHANGED` only (no PTY to answer, no clipboard). Replaces the `Osc52::Disabled` split; pinned today by `test_display_only_write_output_ignores_osc52` (`crates/terminal/src/terminal.rs:4675-4700`). |

### B. Grid/cell read path (15 rows: 12 C, 3 P)

| # | Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|---|
| B1 | `renderable_content()` snapshot per frame in `make_content` — `crates/terminal/src/alacritty.rs:882-930`; fields `display_iter/selection/cursor/display_offset/mode` (`alacritty:src/term/mod.rs:2393-2400`) | `RenderState` `update` or two-phase `begin_update`+`end` — `lg:render.rs:327,356,380`; `vt/render.h:429-479`. **New:** `ghostty_render_state_clean` (`:496`), `row_iterator_next_dirty` (`:625-627`), bulk `ROW_DATA_CELLS_RAW` → `GhosttyCellsView` (`:252-266`) — no `lg:` wrappers yet | C | Zed still takes a full snapshot per frame; dirty iteration + raw-cell bulk read are the v2 answer to v1's "~2 FFI/cell" perf floor and are a perf-gate (#39) option, not a parity need. |
| B2 | `display_iter` → `Indexed<&Cell>` with grid `Point` per cell — `crates/terminal/src/alacritty.rs:885-891,546-565` | `RowIterator`/`CellIterator` lending iterators — `lg:render.rs:251-300,570,691`; viewport row index tracked by the caller | C | `Content.cells: Vec<IndexedCell>` (`crates/terminal/src/terminal.rs:492`) rebuilt in the seam with synthesized points. |
| B3 | `Cell.c` char + `cursor_char` — `crates/terminal/src/alacritty.rs:464-466,922` | Cell codepoint — `lg:screen.rs:347`; graphemes — `lg:render.rs:779-815`; cursor cell via `grid_ref(cursor)` (`lg:terminal.rs:364`); cursor position via sized `DATA_CURSOR` (`vt/render.h:207-209,316-343`) or the scalar reads libghostty-rs still uses (`lg:render.rs:470-480`) | C | The scalar cursor reads (render data 11–17) survive in the header, so libghostty-rs's wrapper still links; `DATA_CURSOR` is one call instead of five. |
| B4 | `Cell.zerowidth()` combining chars — `crates/terminal/src/alacritty.rs:483-491` | Grapheme APIs (`graphemes_len/buf/utf8`) — `lg:render.rs:789-815`; `lg:screen.rs:94` | C | Richer: mode 2027 grapheme clustering (`vt/modes.h:93`). |
| B5 | `Cell.fg/bg: vte Color` — `crates/terminal/src/alacritty.rs:473-481`; consumed by `convert_color` (`crates/terminal_view/src/terminal_element.rs:1988`) | `Style.fg_color/bg_color: StyleColor::{None,Palette,Rgb}` — `lg:style.rs:31,72`; flattened RGB (`lg:render.rs:749-777`, `vt/render.h:747-761`) | P | Seam maps `StyleColor` → vte `Color` (`None`→`Named(Foreground/Background)`, `Palette(0-15)`→`Named` ANSI, `Palette(16-255)`→`Indexed`, `Rgb`→`Spec`) so theme-driven resolution and the `terminal.rs:54` re-export survive. Do **not** use ghostty's flattened RGB for cells. Also read the bg-color *content tags* (`lg:screen.rs:389-395`, `vt/screen.h:218-226`) — v1 ledger P7-011. |
| B6 | Flags `INVERSE/DIM/BOLD/ITALIC/ALL_UNDERLINES/UNDERCURL/STRIKEOUT` — `crates/terminal/src/alacritty.rs:498-537` | `Style{inverse,faint,bold,italic,underline: Underline::{…}, strikethrough,…}` — `lg:style.rs:31-44,122` | C | — |
| B7 | `WIDE_CHAR_SPACER`/`LEADING_WIDE_CHAR_SPACER` — `crates/terminal/src/alacritty.rs:503-506`, `crates/terminal/src/alacritty/hyperlinks.rs:23-25,357,388-391` | `CellWide::{Narrow,Wide,SpacerTail,SpacerHead}` — `lg:screen.rs:435`; `vt/screen.h:104-115` | C | — |
| B8 | `WRAPLINE` on last cell of row — `crates/terminal/src/alacritty.rs:1025-1036` | `Row::is_wrapped` / `is_wrap_continuation` — `lg:screen.rs:295-299`; `vt/screen.h:263-274` | C | Row-level flag instead of a cell flag. |
| B9 | Selection membership in snapshot (`content.selection: SelectionRange`) — `crates/terminal/src/alacritty.rs:918-920,773-779` | Row-local selection range — `lg:render.rs:616`, `vt/render.h:249-302`; per-cell `is_selected` — `lg:render.rs:885`; whole selection via `DATA_SELECTION` (`vt/terminal.h:1848`) / `Terminal::selection()` (`lg:selection.rs:211`) | C | — |
| B10 | `bottom_row_occupied` heuristic — `crates/terminal/src/alacritty.rs:902-908`; consumed with `ALT_SCREEN` for bottom anchoring (`crates/terminal_view/src/terminal_element.rs:1309-1313`) | Derived from iterated cells (pure seam logic) | C | Ports verbatim. |
| B11 | `Cell::has_visible_style_modifier` etc. — `crates/terminal/src/alacritty.rs:539-543` | Derived from `Style` + `has_styling` (`lg:render.rs:894`) | C | — |
| B12 | **New.** Raw `Grid` shape reads: `columns()`, `screen_lines()`, `total_lines()`, `display_offset()` — `crates/terminal/src/alacritty.rs:827-842,899,913-916`; `Dimensions` trait import at `crates/terminal/src/terminal.rs:18` (for `term.history_size()`, `alacritty:src/grid/mod.rs:486,516`); stored in `Content.{total_lines,columns,screen_lines,display_offset}` (`crates/terminal/src/terminal.rs:494-497`) | `DATA_COLS`/`DATA_ROWS`/`DATA_TOTAL_ROWS`/`DATA_SCROLLBACK_ROWS` — `vt/terminal.h:1568,1575,1684,1691`; `lg:terminal.rs:722,726,905,909`; `display_offset` from `DATA_SCROLLBAR` (`vt/terminal.h:1619-1635`, amortized O(1); `lg:terminal.rs:847`) | C | Four cheap reads per frame; `display_offset = total − len − offset` (axis inversion, E2). `get_multi` (`vt/terminal.h:2323`) batches them. |
| B13 | **New.** `GridLinesChange::{Unchanged,Changed}` + `adjusted_last_hovered_word(grid, last_content)` (grid-size change, `total_lines_delta != display_offset_delta`, shift hovered-word lines by the offset delta) — `crates/terminal/src/alacritty.rs:823-880`; `sync()` re-hovers on `Changed` and emits `Wakeup` (`crates/terminal/src/terminal.rs:2343-2352`); `usize::checked_signed_diff` (rustc ≥ 1.95, `rust-toolchain.toml` = 1.97.1) | Pure seam arithmetic over B12. Native alternative: `TrackedGridRef` survives scroll/reflow/eviction — `lg:screen.rs:163-255`, `vt/terminal.h:2386-2418` | C | Port the arithmetic verbatim first (it is the tested contract: `crates/terminal/src/terminal.rs:5040-5400` hover suites). Anchoring `HoveredWord.word_match` with two `TrackedGridRef`s is the follow-up that would make the delta math unnecessary. |
| B14 | **New.** `Terminal::used_lines()` — `ALT_SCREEN` → `total_lines()`; else `history_size() + (last row ≥ cursor whose `Row::is_clear()` is false) + 1` — `crates/terminal/src/alacritty.rs:810-821`; `Row::is_clear` = all cells `GridCell::is_empty` (`alacritty:src/grid/row.rs:153-160`); consumer: inline agent-terminal height (`crates/terminal_view/src/terminal_view.rs:355`) | `active_screen()` (`lg:terminal.rs:851`, `DATA_ACTIVE_SCREEN` `vt/terminal.h:1603`), `rows`, `scrollback_rows`, `cursor_y` (`vt/terminal.h:1589`, active space). **No row-emptiness flag**: `GHOSTTY_ROW_DATA_*` (`vt/screen.h:260-317`) has wrap/grapheme/styled/hyperlink/semantic/dirty only; emptiness needs per-cell `has_text` (`vt/screen.h:175`, `lg:screen.rs:361`) via `grid_ref(Active{x,y})` | P | Scan active rows from the bottom up to `cursor_y + 1`, per row iterate cells until one `has_text` — early-exits on the first occupied row, worst case rows×cols `grid_ref` calls (one-off after task end, not per frame). v1's ghostty backend already has a trailing-blank-row counter for P7-010 (`content_text`); reuse it. |
| B15 | **New.** Per-line cwd history — `CwdHistoryEntry{working_directory, scrollback_position}` (`crates/terminal/src/terminal.rs:1497-1500`), `scrollback_position = history_size + cursor_line` (`:2887-2890`) read at `\r` input (`:2170-2176`), at `record_cwd_change` (`:2841-2854`), and at hover (`:1781,1792` — must be read while `sync()` holds the lock); `cwd_at_line` gives up once `history_size >= scrolling_history` (`:2869-2875`); reset on column change (`:1667-1669`) and clear (`:1680,2164`) | `scrollback_rows` + `cursor_y` (`vt/terminal.h:1691,1589`; `lg:terminal.rs:909,811`) — `history + line` is exactly a Screen-space `y` (`vt/point.h:55`). Anchor alternative: `TrackedGridRef` per entry (`lg:screen.rs:163,255`). Incidental: `OPT_PWD_CHANGED`/`DATA_PWD` (OSC 7) — `vt/terminal.h:1344,1667-1677` | P | Reads are covered and the lock-ordering constraint at `:1790-1791` vanishes on the owner thread. **Not transferable as-is:** the `>= scrolling_history` cap heuristic assumes exact-line eviction; ghostty prunes whole pages (`vt/terminal.h:1383-1389`), so `scrollback_rows` dips then regrows at the cap and the test would trust stale positions. Anchor each entry with a `TrackedGridRef` (dead ref ⇒ entry evicted) and drop the heuristic + the column-change reset. Zed's cwd source (process table, `pty_info.rs`) is unchanged; OSC 7 is a possible later improvement, not parity. |

### C. Content/text extraction (5 rows: 5 C)

| # | Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|---|
| C1 | `content_text` / `bounds_to_string` — `crates/terminal/src/alacritty.rs:932-936`; `alacritty:src/term/mod.rs:558`; consumer `get_content` (`crates/terminal/src/terminal.rs:2361-2364`) | `Formatter` (plain/VT/HTML, `unwrap`/`trim`) — `lg:fmt.rs:18-135`; `vt/formatter.h:107-111`; **new** streaming `ghostty_formatter_format(fmt, writer)` (`:161`) | C | Formatter redesign (`2ed67cadd`, `79aa256fa`) — re-fire P5-002/P7-010 (§5). The tab-compression quirk (`crates/terminal/src/alacritty/hyperlinks.rs:335-338`) does not apply to cell iteration. |
| C2 | `last_non_empty_lines` (WRAPLINE-aware logical lines) — `crates/terminal/src/alacritty.rs:952-972,1016-1052`; consumer `last_n_non_empty_lines` (`crates/terminal/src/terminal.rs:2366-2369`) | Reimplement over `Row::is_wrapped` + cell iteration from the bottom | C | Straight port. |
| C3 | `full_content_range` (topmost→bottommost) — `crates/terminal/src/alacritty.rs:946-950`; drives `select_all` (`crates/terminal/src/terminal.rs:1951-1961`) | `total_rows` (`lg:terminal.rs:905`) + Screen-space points; or `select_all` directly (`lg:selection.rs:233`) | C | — |
| C4 | `selection_to_string` — `crates/terminal/src/alacritty.rs:248-250,893-897` | `format_selection_alloc/buf` — `lg:selection.rs:381-445`; `vt/selection.h:870-903` | C | Ledger P5-002 (mid-row trailing space) re-fires on the formatter redesign. |
| C5 | Cell-accurate line text with tab preservation for path matching — `crates/terminal/src/alacritty/hyperlinks.rs:335-368` | Row/cell iteration (codepoint + wide/spacer info) | C | Prototype for the G1 extraction layer. |

### D. Coordinates & points (5 rows: 2 C, 2 P, 1 G)

| # | Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|---|
| D1 | `Point{Line(i32), Column}`: signed, active-relative, negative = scrollback — `crates/terminal/src/alacritty.rs:746-757`, `crates/terminal/src/terminal.rs:438-448`; mouse mapping `crates/terminal/src/mappings/mouse.rs:194-234` | Tagged unsigned points `Point::{Active,Viewport,Screen,History}` — `lg:terminal.rs:1031-1110`; `vt/point.h:49-59` | P | Seam translation layer: Zed line `L` ↔ `Viewport(y = L + display_offset)` for hit-testing, `Screen(y = history_rows + L)` for stable ranges. Zed's public `Point` stays. |
| D2 | `Boundary::{Grid,Cursor,None}` + `point.add/sub(term, boundary, n)` + `grid_clamp` — `crates/terminal/src/alacritty.rs:13,1012`, `crates/terminal/src/alacritty/hyperlinks.rs:102,112,280,382,402`; semantics `alacritty:src/index.rs:34-44,65-92,141` | None — ghostty points are plain data | G | **G6.** ~50 lines in the seam over `(cols, total_rows, viewport)` with wide-char expansion via `cell.wide()`. |
| D3 | `Direction`/`Side` (half-cell) in selections — `crates/terminal/src/alacritty.rs:381-388,400-412`, `crates/terminal/src/mappings/mouse.rs:202-234` | Selection is `GridRef`-granular (`lg:selection.rs:56`); gesture API takes fractional positions (`lg:selection/gesture.rs:253`) | P | Fold the half-cell side into the chosen cell when building `GridRef` endpoints. |
| D4 | `Dimensions` trait impl on `TerminalBounds` — `crates/terminal/src/alacritty.rs:284-300` | Not needed: `resize(cols, rows, cell_w_px, cell_h_px)` — `lg:terminal.rs:315`; `vt/terminal.h:2023-2027` (pixel size also feeds mode-2048 reports, `:2009-2012`) | C | `TerminalBounds::num_lines/num_columns/cell_width/line_height` (`crates/terminal/src/alacritty.rs:111-118`) feed it directly; the trait impl is deleted. Pass real cell pixel sizes (Zed has them) so size reports are correct. |
| D5 | Point stability across scroll/reflow (alacritty: none; Zed re-derives every frame, B13) | `TrackedGridRef` — `lg:screen.rs:163-255`; `vt/terminal.h:2386-2418`; `vt/grid_ref.h:78` | C | Richer than alacritty; B13/B15 candidates. |

### E. Scrolling & viewport (6 rows: 6 C)

| # | Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|---|
| E1 | `Scroll::{Delta,PageUp,PageDown,Top,Bottom}` → `scroll_display` — `crates/terminal/src/alacritty.rs:227-229,332-342`; `crates/terminal/src/terminal.rs:1975-2010` | `scroll_viewport(ScrollViewport::{Top,Bottom,Delta(isize),Row(usize)})` — `lg:terminal.rs:344,1120-1135`; `vt/terminal.h:223-274` | C | `PageUp/PageDown` = `Delta(±rows)`. Sign: alacritty positive = up; ghostty "up is negative" (`vt/terminal.h:230,256`) — invert. |
| E2 | `display_offset()` — `crates/terminal/src/alacritty.rs:223-225` (offset-from-bottom) | `scrollbar() -> {total, offset, len}` — `lg:terminal.rs:847`; `vt/terminal.h:319-328,1619-1635` (offset-from-top, amortized O(1), poll per frame) | C | `display_offset = total − len − offset`. |
| E3 | `scrolled_to_top = display_offset == history_size`; `scrolled_to_bottom = display_offset == 0` — `crates/terminal/src/alacritty.rs:926-927` | Derivable: top = `offset == 0`; bottom = `offset + len == total`; or `DATA_VIEWPORT_ACTIVE` (`vt/terminal.h:1858`, `lg:terminal.rs:859`) for the bottom case | C | — |
| E4 | `total_lines` / `screen_lines` / `history_size` — `crates/terminal/src/alacritty.rs:938-944`; called off-thread via `lock_unfair` (`crates/terminal/src/terminal.rs:1910-1916`) | `total_rows` / `rows` / `scrollback_rows` — `lg:terminal.rs:905,726,909` | C | Feeds the scrollbar unchanged; the off-thread reads reroute (G8). |
| E5 | `scroll_to_point` — `crates/terminal/src/alacritty.rs:252-254`; search activate (`crates/terminal/src/terminal.rs:1934`) | `scroll_viewport(Row(n))`, `n` from the point's Screen-space y (`vt/terminal.h:233-246`) | C | Seam math. |
| E6 | Pixel scroll accumulation / alt-scroll arrows — `crates/terminal/src/terminal.rs:2724-2760`, `crates/terminal/src/mappings/mouse.rs:141-151` | Zed-side (unchanged); `Mode::ALT_SCROLL` for the gate (`lg:terminal.rs:1220`) | C | — |

### F. Selection (7 rows: 6 C, 1 P)

| # | Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|---|
| F1 | `SelectionType::Simple` click-drag with sides — `crates/terminal/src/terminal.rs:151-156,2609-2640`, `crates/terminal/src/alacritty.rs:231-246,400-412` | `Selection::new(start, end, rectangle)` + `set_selection` — `lg:selection.rs:56,225`; `vt/selection.h:80-116`; gesture API (`lg:selection/gesture.rs:226-460`) | P | Keep Zed's click-count/threshold/shift-extend logic (`crates/terminal/src/terminal.rs:2590-2640`) and drive plain `Selection` objects; the half-cell nuance is D3. |
| F2 | `SelectionType::Semantic` (double-click) — `crates/terminal/src/terminal.rs:2495,2610`; boundary set = `SEMANTIC_ESCAPE_CHARS` + `─` (A7); pinned by `crates/terminal/src/alacritty.rs:1188-1204` | `select_word(SelectWordOptions.with_boundary_codepoints(...))` — `lg:selection.rs:279,506`; `vt/selection.h:744,123-140` | C | Pass the A7 set (including `─`) on every call; ghostty's defaults differ. |
| F3 | `SelectionType::Lines` (triple-click) — `crates/terminal/src/terminal.rs:2611` | `select_line(SelectLineOptions)` — `lg:selection.rs:247,476`; `vt/selection.h:799,174-194` | C | Disable `semantic_prompt_boundary` for whole-logical-line behavior. |
| F4 | `SelectionType::Block` | Never constructed by Zed; `rectangle` flag exists (`lg:selection.rs:56`) | C | `is_block` plumbing (`crates/terminal/src/terminal.rs:474-479`) can stay. |
| F5 | `selection.update(point, side)` drag — `crates/terminal/src/alacritty.rs:235-246` | Rebuild `Selection` with a new end, or `Selection::adjust` (`lg:selection.rs:106,614`) | C | — |
| F6 | `select_all` — `crates/terminal/src/terminal.rs:1951-1961`, `crates/terminal_view/src/terminal_view.rs:654-655` | `select_all()` — `lg:selection.rs:233`; `vt/selection.h:818` | C | — |
| F7 | Selection survival across resize (alacritty drops on column change — `alacritty:src/term/mod.rs:680-691`) | Grid-ref based; `OPT_SELECTION` (`vt/terminal.h:1305`) | C | Zed re-derives selection each interaction; differences acceptable. |

### G. Regex search (4 rows: 4 G — one logical gap)

All four rows are facets of **G1**; deep-dive §3.1.

| # | Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|---|
| G-1 | `RegexSearch::new(pattern)` — `crates/terminal/src/alacritty.rs:22,61-63,367-379`; built from user query or `regex::escape`d literal (`crates/terminal_view/src/terminal_view.rs:1244-1252`) | None: no `search.h`; `grep -i 'search\|regex' vt.h vt/*.h` hits only doc prose (`vt/grid_ref.h:78`, `vt/selection.h:157-160,752`, `vt/terminal.h:2260`, a key name `vt/key/event.h:278`); `ghostty:src/terminal/search.zig` is re-exported as a Zig module only (`ghostty:src/lib_vt.zig:65`), no `@export`, nothing in `ghostty:src/terminal/c/` | G | Rust `regex` compile; smart-case (`alacritty:src/term/search.rs:39-40`). |
| G-2 | `search_matches` = `RegexIter` topmost→bottommost — `crates/terminal/src/alacritty.rs:1091-1105` | None | G | Logical-line extraction, regex, byte→cell→Screen-space ranges. |
| G-3 | `find_matches` on a background thread holding the `FairMutex` — `crates/terminal/src/terminal.rs:2793-2799` | None + `!Send` terminal | G | Extract on the owner thread, regex on `background_spawn` over the owned string. |
| G-4 | URL hover regex per logical line (`url_regex`, `RegexIter` bounded by `line_search_left/right`) — `crates/terminal/src/alacritty/hyperlinks.rs:28,62,125-133`; `alacritty:src/term/search.rs:593-616` | None | G | Same extraction; the path branch already uses Rust `regex` (`crates/terminal/src/alacritty/hyperlinks.rs:29,315-410`). Upstream search work in the delta (`9659167ec`, `bc8bb6c0f`) is internal viewport-fingerprint reuse, not an API. |

Semantics the replacement must preserve (`alacritty:src/term/search.rs`): matches cross soft-wrapped
rows but never hard newlines (`:302,405,550-595`); wide-char spacers skipped (`skip_fullwidth`, `:436`);
empty matches discarded; smart-case (`:39-40`); complexity failures degrade to no-match with a warning
(`:264`). Match type `RangeInclusive<Point>` (`:21`) → Zed `Range` (`crates/terminal/src/alacritty.rs:759-771`).

### H. Hyperlinks (5 rows: 3 C, 2 G — the regex row belongs to G1)

| # | Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|---|
| H1 | `Cell::hyperlink()` OSC 8 storage — `crates/terminal/src/alacritty.rs:55,414-456,493-496`, `crates/terminal/src/terminal.rs:317-326`; render + click (`crates/terminal_view/src/terminal_element.rs:892,1912`) | Cell `has_hyperlink` (`lg:screen.rs:373`, `vt/screen.h:192-196`) + `GridRef::hyperlink_uri` (`lg:screen.rs:116`, `vt/grid_ref.h:168-187`); row hint `has_hyperlink` (`lg:screen.rs:311`, `vt/screen.h:291-295`) | C | `HyperlinkData::Alacritty` becomes `Owned{id: None, uri}`. |
| H2 | `Hyperlink::id()` — `crates/terminal/src/alacritty.rs:424-429`; only real caller is the seam test `:1114-1121` | Not exposed — swept all `vt/*.h` for `hyperlink`: URI accessor and presence flags only | G | **G7.** Drop or return `None`. |
| H3 | Hover extent: expand while adjacent cells carry the same hyperlink (id+uri equality) — `crates/terminal/src/alacritty/hyperlinks.rs:97-123` | Walk adjacent grid refs comparing `hyperlink_uri` | C | Adjacent distinct links with identical URIs merge (ledger P6-001). Re-fire on `9313d580c` (§5). |
| H4 | URL/path regex detection — `crates/terminal/src/alacritty/hyperlinks.rs:22,125-133,315-410` | See G1 | G→G1 | Counted under G above. |
| H5 | **New.** `clear_hyperlink` + `HoveredWord.id` throttle keyed by `terminal_element.rs` — `crates/terminal/src/terminal.rs:518-522,1841-1850`; `crates/terminal_view/src/terminal_element.rs:1368-1372`; `#[cfg(test)] suppress_hyperlink_throttle_once` | Zed-owned; no backend involvement | C | Migrates unchanged; B13 supplies the "content unchanged" signal it relies on. |

### I. Vi mode (3 rows: 3 G — one logical gap)

All facets of **G2**; deep-dive §3.2.

| # | Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|---|
| I1 | `toggle_vi_mode` / `vi_motion` / `vi_goto_point` — `crates/terminal/src/alacritty.rs:256-266,344-365`; 16 motions dispatched at `crates/terminal/src/terminal.rs:2216-2232` | None | G | Port `alacritty:src/vi_mode.rs` (16 of 21 variants; `Semantic*`/`WordLeftEnd` unused — `:15-59`). |
| I2 | `ViModeCursor::scroll` follow-on-scroll + selection tie-in — `crates/terminal/src/alacritty.rs:974-1004`; `crates/terminal/src/terminal.rs:1683-1700,2243-2280` | None | G | Vi cursor becomes Zed-side state (ledger P6-004 records the rotation caveat). |
| I3 | `TermMode::VI` surfaced as `Modes::VI` — `crates/terminal/src/alacritty.rs:607,700` | None | G | Synthesize from `vi_mode_enabled` (`crates/terminal/src/terminal.rs:1470,3070-3072`). |

### J. Direct grid mutations (3 rows: 1 C, 1 P, 1 G)

| # | Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|---|
| J1 | `append_text_to_term` — unsafe `term.input()`/`term.newline()` after PTY death — `crates/terminal/src/alacritty.rs:1054-1089`, called `crates/terminal/src/terminal.rs:3054` | `vt_write` of `"\r\n" + line + "\r\n"…` | C | Strict improvement; PTY is dead so no interleaving. |
| J2 | `shrink_to_used` = `grid.truncate()` — `crates/terminal/src/alacritty.rs:806-808`, `alacritty:src/grid/mod.rs:406`; callers `crates/acp_thread/src/acp_thread.rs:4673,4710` | Caller-driven compression: `compress(CompressionMode)` + `compression_activity` — `lg:terminal.rs:513,531`; `vt/terminal.h:44-53,187-216,2240-2271` | P | Same goal, incremental mechanism; compress-until-`COMPLETE` loop at the acp_thread call sites. |
| J3 | `clear_saved_screen` — `ClearMode::Saved` + raw `grid_mut()` region resets + cursor-row hoist — `crates/terminal/src/alacritty.rs:781-804`; callers `crates/terminal/src/terminal.rs:1677-1682` (Clear action) and `:2159-2165` (`clear_for_init_command`), both followed by `reset_cwd_history` | No `grid_mut`; `reset()` is a full RIS (`lg:terminal.rs:339`). **New injection primitive:** `vt_write_until_ground` + `DATA_VT_GROUND` (`vt/terminal.h:2080-2110,1920-1933`) | G | **G5.** §3.3 (amended). |

### K. Modes (2 rows: 2 C)

| # | Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|---|
| K1 | `TermMode` → `Modes(u32)` with 17 flags — `crates/terminal/src/alacritty.rs:567-713`, `crates/terminal/src/terminal.rs:352-396` | `mode(Mode) -> bool` via `DATA_MODE`/`GhosttyTerminalModeConfig` — `lg:terminal.rs:433-449`, `vt/terminal.h:1068-1083,1909-1918`; `Mode` consts `lg:terminal.rs:1199-1235`; `vt/modes.h:50-97`; `active_screen()` (`lg:terminal.rs:851`); `is_mouse_tracking()` (`:866`) | C | Mapping unchanged: APP_CURSOR→`DECCKM`(1), APP_KEYPAD→`KEYPAD_KEYS`(66), SHOW_CURSOR→`CURSOR_VISIBLE`(25), LINE_WRAP→`WRAPAROUND`(7), ORIGIN→6, INSERT→ANSI 4, LINE_FEED_NEW_LINE→ANSI 20, FOCUS_IN_OUT→1004, ALTERNATE_SCROLL→1007, BRACKETED_PASTE→2004, SGR_MOUSE→1006, UTF8_MOUSE→1005, ALT_SCREEN→1047/`active_screen()`, MOUSE_REPORT_CLICK→1000, MOUSE_DRAG→1002, MOUSE_MOTION→1003; VI → Zed-side. New modes 2033/2048/5522 (`vt/modes.h:95-97`) have no Zed flag and need none. Ledger P7-006 (non-exclusive mouse flags) stands. |
| K2 | Per-frame `Modes` snapshot in `Content` — `crates/terminal/src/alacritty.rs:912` | 16 `DATA_MODE` reads or one `ghostty_terminal_get_multi` (`vt/terminal.h:2323`) | C | u32 layout and consumers (`to_esc_str` `crates/terminal/src/mappings/keys.rs:47-51`, mouse reports, paste gate `crates/terminal/src/terminal.rs:2325`, focus `:2371-2381`) untouched. |

### L. Cursor (4 rows: 3 C, 1 P)

| # | Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|---|
| L1 | `RenderableCursor{shape, point}` → `Cursor` — `crates/terminal/src/alacritty.rs:727-744` | Sized `DATA_CURSOR` (`GhosttyRenderStateCursor{viewport_x/y, wide_tail, visible, blinking, password_input, visual_style}`) — `vt/render.h:207-209,304-343`; libghostty-rs still reads the scalars (`lg:render.rs:446-480`) | C | `CursorVisualStyle::{Bar,Block,Underline,BlockHollow}` (`lg:render.rs:943`) maps 1:1 onto `CursorShape` (`crates/terminal/src/terminal.rs:418-425`). |
| L2 | Hidden cursor (`CursorShape::Hidden` via SHOW_CURSOR unset) — `crates/terminal/src/alacritty.rs:736-743` | `visible` / `viewport_has_value` — `vt/render.h:319-333`; `lg:render.rs:446,470` | C | — |
| L3 | Default cursor style from settings — `crates/terminal/src/alacritty.rs:268-282`, `crates/terminal/src/terminal.rs:1892-1895` | `set_default_cursor_style/blink` — `lg:terminal.rs:964-976` | C | — |
| L4 | `CursorBlinkingChange` → `Event::BlinkChanged(term.cursor_style().blinking)` — `crates/terminal/src/alacritty.rs:317`, `crates/terminal/src/terminal.rs:1602-1606` | No change-callback (effects table `vt/terminal.h:87-102` has none); state per-sync via `blinking` (`vt/render.h:334-336`, `lg:render.rs:451`) | P | Seam detects the transition during `sync()` and emits `BlinkChanged` (§3.4). |

### M. Colors (5 rows: 4 C, 1 P)

| # | Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|---|
| M1 | `pub use vte::ansi::{Color, NamedColor, Rgb}` — `crates/terminal/src/terminal.rs:54`; consumers `crates/terminal_view/src/terminal_element.rs:1988` (`convert_color`), `crates/debugger_ui/src/session/running/console.rs:789,855-856` | vte survives as a workspace dep (`Cargo.toml:873`) — no ghostty type reaches downstream crates | C | Keep the vte color model as the domain contract; seam maps `StyleColor` → vte `Color` (B5). |
| M2 | OSC 4/10/11/12 dynamic set → alacritty override table — `crates/terminal/src/terminal.rs:1622-1635` | Two-layer color state (embedder defaults + OSC overrides), effective/default getters — `vt/terminal.h:125-183,1199-1227,1718-1777`; `lg:terminal.rs:914-993` | C | — |
| M3 | `ColorRequest(index, fmt)` answered from override-or-theme (`get_color_at_index`, `crates/terminal/src/terminal.rs:1622-1635,3378`) | ghostty answers OSC color queries itself through `WRITE_PTY` (`ghostty:src/terminal/stream_terminal.zig` `colorOperation`/xterm color report path — line numbers **unverified** at 8867c37c5; v1 cited `:656,743`) | P | Lazy → eager: seed theme colors as defaults (`OPT_COLOR_FOREGROUND/BACKGROUND/CURSOR/PALETTE`, `vt/terminal.h:1199-1227`) at creation **and on every theme change** (ledger P8-003). Palette set preserves OSC overrides (`:148-150`). |
| M4 | `to_vte_rgb` — `crates/terminal/src/mappings/colors.rs:2-10` | Also needed: theme → `RgbColor` (`lg:style.rs:84`) for seeding defaults | C | Add a sibling conversion; keep both. |
| M5 | Color-scheme query (CSI ?996n) — Zed: none (alacritty silent) | `COLOR_SCHEME` callback — `vt/terminal.h:914-930,1160`; `lg:terminal.rs:1993` | C | Optional capability (ledger P7-012 accepted). |

### N. Event model (13 rows: 11 C, 2 P)

Every `AlacTermEvent` variant Zed converts (`crates/terminal/src/alacritty.rs:302-324`; alacritty enum
`alacritty:src/event.rs:14-58`) → ghostty:

| # | `TerminalBackendEvent` | Zed handling (`crates/terminal/src/terminal.rs`) | ghostty | V | Fill |
|---|---|---|---|---|---|
| N1 | `MouseCursorDirty` | NOOP (`:1611-1613`) | — | C | Drop. |
| N2 | `Title(String)` | breadcrumbs (`:1565-1580`) | `TITLE_CHANGED` (`vt/terminal.h:990-1003,1142`; `lg:terminal.rs:1948`) + `DATA_TITLE` (`vt/terminal.h:1656-1665`; `lg:terminal.rs:884`) | C | **Lifetime tightened:** the borrowed title is valid only "until the next mutating terminal call" (`:1659-1660`) — copy inside the callback, never store the `&str`. |
| N3 | `ResetTitle` | clear breadcrumbs (`:1581-1584`); alacritty emits it from `set_title(None)` (`alacritty:src/term/mod.rs:507,2221-2228`) | `TITLE_CHANGED` fires on title mutation; empty title reads as len 0 (`vt/terminal.h:1660-1661`) | P | Whether the callback fires on RIS/OSC-reset is **unverified**; fall back to `title()` comparison during sync. |
| N4 | `ClipboardStore(data)` (OSC 52 write) | write clipboard (`:1585-1587`) | `CLIPBOARD_WRITE` sized request + synchronous `reply` — `vt/terminal.h:445-624`; `lg:terminal.rs:2037` (pre-reply shape, §4.3) | C | **Re-adjudicated (§0.3).** Pick the first `text/*` representation, call `reply` with `SUCCESS`; OSC 52 discards the reply (`:498-501`). Also covers OSC 1337 and 5522 writes for free. |
| N5 | `ClipboardLoad(formatter)` (OSC 52 read) | answer from clipboard (`:1588-1597`) | `OPT_CLIPBOARD_READ` (`vt/terminal.h:626-818,1515-1524`), default NULL = ignore | C | **Re-adjudicated (§0.3).** Leave NULL for parity (matches `OnlyCopy`/`Disabled`: no reply). Keep the enum variant; do not delete the handler. |
| N6 | `ColorRequest(index, fmt)` | answer from colors/theme (`:1622-1635`) | Internal via `WRITE_PTY` (M3) | C | With seeding caveat (M3). |
| N7 | `PtyWrite(String)` | forward to PTY (`:1598`) | `WRITE_PTY` — `vt/terminal.h:1030-1048,1101-1109`; `lg:terminal.rs:1896` | C | Mandatory registration. |
| N8 | `TextAreaSizeRequest(fmt)` (CSI 14/16/18 t) | reply with bounds (`:1599-1601`) | `SIZE` callback — `vt/terminal.h:970-988,1150`; `lg:terminal.rs:1973`; **also** fires for mode-2048 in-band reports on enable and on every `resize` (`vt/terminal.h:973-976,2009-2012`; `ghostty:src/terminal/stream_terminal.zig:254-266`) | C | Register once; in-band resize needs nothing more than real cell pixel sizes in `resize` (D4). |
| N9 | `CursorBlinkingChange` | emit `BlinkChanged` (`:1602-1606`) | None | P | Seam-detect per sync (L4). |
| N10 | `Wakeup` | `sync()` + emit (`:1614-1620`) | None needed | C | Reader loop emits after each `vt_write` batch (`write_output` already does: `:1907`). |
| N11 | `Bell` | emit (`:1607-1609`) | `BELL` — `vt/terminal.h:330-341,1117`; `lg:terminal.rs:1911` | C | — |
| N12 | `Exit` | task finished (`:1610`) | n/a — Zed's own PTY loop (G3) | C | — |
| N13 | `ChildExit(status)` | task finished (`:1636`) | n/a — same | C | — |

Delivery-model note (unchanged): alacritty events arrive on an unbounded channel from the IO thread
(`ZedListener`, `crates/terminal/src/alacritty.rs:326-330`; pump `crates/terminal/src/terminal.rs:1402,1559`);
ghostty callbacks fire **synchronously inside `vt_write`** on the owner thread and must not re-enter it
(`vt/terminal.h:78-83`). Shim: callbacks enqueue the same `TerminalBackendEvent` values so
`process_event` (`crates/terminal/src/terminal.rs:1563-1640`) is untouched. See §3.4.

### O. PTY & event loop (4 rows: 4 G — one logical gap)

All facets of **G3**; deliberate libghostty non-goal. Design belongs to the architecture ticket.

| # | Zed uses | ghostty | V | Fill |
|---|---|---|---|---|
| O1 | `tty::{new, Pty, Options, Shell}` — `crates/terminal/src/alacritty.rs:24,49,161-186` | None | G | Port alacritty's `tty` module or adopt a PTY crate; keep the `pty_options` shape. (`portable-pty = "0.9.0"`, `Cargo.toml:741`, sole consumer `acp_thread`; whether 0.9.x carries the Windows `kill()` fix is the PTY ticket's question.) |
| O2 | `EventLoop`/`Notifier`/`Msg::{Resize,Shutdown}` — `crates/terminal/src/alacritty.rs:11,85-109,203-217` | None | G | Zed-owned reader/writer; resize = `TIOCSWINSZ` + `terminal.resize()`. |
| O3 | `SignalMask` (unix), `escape_args` (Windows), `drain_on_exit` — `crates/terminal/src/alacritty.rs:156-178` | None | G | Moves with the tty port. |
| O4 | `ProcessIdGetter` from pty fd / `child_watcher` — `crates/terminal/src/alacritty.rs:65-83` | None | G | Moves with the tty port; `pty_info.rs` is alacritty-independent. |

### P. Input encoding (6 rows: 4 C, 1 P, 1 G)

| # | Zed uses | libghostty equivalent | V | Fill |
|---|---|---|---|---|
| P1 | `to_esc_str(keystroke, mode, option_as_meta)` classic xterm encoding — `crates/terminal/src/mappings/keys.rs:47-234` (alacritty-free); caller `crates/terminal/src/terminal.rs:2295-2302` | `key::Encoder` + `set_options_from_terminal` — `lg:key.rs:27,136-212` | C* | Works as-is **only if** G4 is addressed. |
| P2 | **Kitty keyboard protocol**: alacritty config-gated **off** (`Config.kitty_keyboard` default false — `alacritty:src/term/mod.rs:350,363`; queries ignored `:1275-1324`); Zed never enables it | ghostty answers `CSI ? u` unconditionally (`ghostty:src/terminal/stream_terminal.zig:492,1516-1523`); flags via `DATA_KITTY_KEYBOARD_FLAGS` (`vt/terminal.h:1612-1617`, `lg:terminal.rs:830`); **no disable option** — enumerated options 0–39 (`vt/terminal.h:1093-1549`), none touches kitty keyboard; the only `kitty_keyboard` symbol outside `terminal.h` is a formatter flag (`vt/formatter.h:58`) | G | **G4.** §3.5. Re-confirmed at 8867c37c5. |
| P3 | **New.** ctrl+alt+lowercase → `ESC` + ctrl code; alt+shift+letter → `ESC` + uppercase; plain alt → `ESC` + key (macOS gated by `option_as_meta`) — `crates/terminal/src/mappings/keys.rs:213-231`; test `:388-404`; also `f5` fix `:185` | `key::Encoder` legacy mode with `set_alt_esc_prefix` (`lg:key.rs:174`) and `set_macos_option_as_alt` (`:199`); whether ghostty's legacy encoder emits `ESC \x01` for ctrl+alt+a is **unverified** here | P | These become rows of the G4 conformance oracle (keys.rs suite); v1 ledger P3-010 (ctrl-shift-letter) shows the pattern for adjudicating an encoder delta. |
| P4 | Mouse reports (X10/UTF8/SGR from `Modes`) — `crates/terminal/src/mappings/mouse.rs:79-100,153-330` | Zed-side kept; `mouse::Encoder` optional (`lg:mouse.rs:32,128`) | C | Ledger P3-009/P8-002 stand. |
| P5 | Bracketed paste gate + sanitization — `crates/terminal/src/terminal.rs:2324-2332` (bracketed: strip `\x1b`; else `\r\n`/`\n` → `\r`) | Zed-side kept; **or** `ghostty_terminal_paste` — `vt/paste.h:159-190` (mode-aware framing, unsafe → `GHOSTTY_REJECTED` `vt/types.h:108`, `allow_unsafe` retry, streams via `WRITE_PTY`, mode-5522 paste events when `CLIPBOARD_READ` is installed `:24-31`); building blocks `paste_is_safe/encode` (`vt/paste.h:209,241`; `lg:paste.rs:49,69` — no `terminal_paste` wrapper) | C | **Re-adjudicated (§0.3).** Parity is satisfied either way; adopting `terminal_paste` changes sanitization (every unsafe control byte → space, `vt/paste.h:214-218`, vs Zed's ESC-only strip) and adds an unsafe-paste confirmation UX. Decision folded into #32. |
| P6 | Focus in/out reports — `crates/terminal/src/terminal.rs:2371-2381` | Zed-side kept; `focus_encode` (`lg:focus.rs:27-43`) | C | — |

### Q. Parser & vte fate (3 rows: 3 C)

| # | Zed uses | After migration | V | Fill |
|---|---|---|---|---|
| Q1 | `Processor<StdSyncHandler>` driving `Term` — `crates/terminal/src/terminal.rs:53,982,1259,1452,1904` | Deleted; ghostty parses inside `vt_write` | C | Both feed paths collapse to `vt_write`. |
| Q2 | Standalone `parse_ansi_text`/`strip_ansi_text` with custom vte `Handler`s — `crates/terminal/src/terminal.rs:180-315`; consumer `crates/debugger_ui/src/session/running/console.rs:180` | Unchanged: **vte crate survives** for these + the `Color/NamedColor/Rgb` re-export (`:54`) | C | ghostty's `osc.rs`/`sgr.rs` are not full-stream handlers. |
| Q3 | Seam's vte-via-alacritty imports (`ClearMode`, `CursorShape`, `CursorStyle`, `NamedPrivateMode`, `PrivateMode`, `Handler`) — `crates/terminal/src/alacritty.rs:26-29,34` | Replaced by ghostty equivalents / deleted with their call sites | C | — |

### R. Tests as migration oracle (1 row: 1 C)

`crates/terminal/src` carries **102** tests at `38c5dd7c98` (86 at v1's fork point; +14 `terminal.rs`,
+1 `alacritty.rs`, +1 `keys.rs`).

| Suite | Transfers? |
|---|---|
| Seam contract round-trips (hyperlink storage, cell zerowidth, Modes↔TermMode, selection ranges, **semantic selection stops at `─`**) — `crates/terminal/src/alacritty.rs:1107-1205` | Yes — rewrite the alacritty-typed halves; the `─` test (`:1188-1204`) becomes the A7 pin. |
| Hyperlink grid hit-testing (real `Term<VoidListener>`) — `crates/terminal/src/alacritty/hyperlinks.rs:489-1149`, perf `:1144-1340` | Yes — replace Term construction with `Terminal::new` + `vt_write`; best behavioral oracle for G1. |
| Key/mouse encoding suites — `crates/terminal/src/mappings/keys.rs:262-430` (incl. `test_ctrl_alt_codes` `:388-404`), `crates/terminal/src/mappings/mouse.rs:100-140` | Unchanged; spec for the G4 encoder switch (P3). |
| `write_output`/CRLF/OSC52-display-only gpui tests — `crates/terminal/src/terminal.rs:4545-4700` | Yes — drive the public `write_output` seam. |
| **New:** hover-with-changing-content suites (`GridLinesChange`, wakeup counting) — `crates/terminal/src/terminal.rs:5040-5400`; cwd-history unit tests `:5720-5850`; shift+drag `:3914-4013` | Yes — B13/B15/F1 oracles; the wakeup-count assertions (`:5320-5391`) pin the "emit Wakeup on Changed" contract exactly. |
| Batching/path-target/agent integration — `crates/terminal_view/src/terminal_element.rs`, `terminal_path_like_target.rs`, `agent/.../terminal_tool.rs` | Unchanged (domain types only). |

---

## 3. Gap deep-dives

### 3.1 G1 — Regex search over the grid (re-confirmed)

Surface unchanged from v1: find-in-terminal (`Search::new` `crates/terminal/src/alacritty.rs:367-379` ←
`crates/terminal_view/src/terminal_view.rs:1244-1252`; `find_matches` → `search_matches` → `RegexIter`
`crates/terminal/src/alacritty.rs:1091-1105`), URL hover (`crates/terminal/src/alacritty/hyperlinks.rs:125-133`),
path hover already on Rust `regex` (`:315-410`).

Re-verification at 8867c37c5: no `search.h`; header grep for `search`/`regex` hits doc prose only;
`ghostty:src/terminal/search.zig` exists and is re-exported to Zig consumers via `ghostty:src/lib_vt.zig:65`,
but `lib_vt.zig`'s `@export` list and `ghostty:src/terminal/c/` contain no search symbol. Upstream search
commits in the delta (`9659167ec`, `bc8bb6c0f`, `659a60ae5`) are internal. **G1 stands; strategy unchanged.**

New helpers the v2 engine can use: `ROW_DATA_CELLS_RAW` (`vt/render.h:252-266`) for one-call-per-row
viewport extraction (hover), and `TrackedGridRef` to anchor match ranges across scroll (D5).

### 3.2 G2 — Vi mode (unchanged)

16 motions (`crates/terminal/src/terminal.rs:2216-2232`); port scope and strategy as v1. Ledger P6-004
(vi cursor does not ride content rotation) remains the accepted caveat.

### 3.3 G5 — `clear_saved_screen` (amended)

What it does (`crates/terminal/src/alacritty.rs:781-804`): clear scrollback, reset rows above the cursor,
hoist the cursor row to line 0, reset below. Two callers, both followed by `reset_cwd_history`
(`crates/terminal/src/terminal.rs:1677-1682,2159-2165`).

ghostty still has no raw grid mutation. Options, in preference order:

1. **VT-sequence emulation** — `CSI 3 J` (erase scrollback), `CSI <cursor_y> S` (scroll up to hoist the
   prompt row), `CUP 1;<col>`, `CSI 0 J`. **New in v2:** the injection point is now well-defined. Before
   each injection, drain the pending PTY chunk with `ghostty_terminal_vt_write_until_ground`
   (`vt/terminal.h:2080-2110`: consumes the shortest prefix that reaches ground, returns `NO_VALUE` if the
   slice ended mid-sequence) or check `DATA_VT_GROUND` (`:1920-1933`) — both exist precisely so "insert
   out-of-band VT sequences" is safe (`:1926-1929`). This removes v1's "parser state persists across
   `vt_write` calls" hazard.
2. **Accept ghostty-native semantics** (`ESC[H ESC[2J ESC[3J`), losing the keep-prompt-line nicety.

The seam ticket picks 1 vs 2 and adds a seam test asserting the prompt-line behavior (none exists today).
Ledger P7-003 (destructive clears do not rotate into scrollback on ghostty) is adjacent and should be
re-fired together with this (`9313d580c` touched scroll-clear state, §5).

### 3.4 G8 / event-model impedance (amended)

- ghostty `Terminal`, `RenderState`, iterators are `!Send`/`!Sync` (libghostty-rs unchanged, recon 5/6);
  the C runtime is now explicitly single-threaded (`init_single_threaded`, `TinyIo` —
  `ghostty:src/lib_vt.zig:45-50`, `ghostty:src/terminal/c/terminal.zig:56-58`; `vt/terminal.h:2260`).
  `FairMutex` disappears.
- Cross-thread call sites that reroute through the owner: `find_matches` (`crates/terminal/src/terminal.rs:2793-2799`),
  `total_lines`/`viewport_lines`/`used_lines` (`:1910-1920`), `get_content`/`last_n_non_empty_lines`
  (`:2361-2369`), `with_renderable_cells` (`:2355-2359`), the render pull `sync` (`:2334-2354`), and the
  new `\r`-input cursor read (`:2170-2176`) and `record_cwd_change` read (`:2846-2849`).
- Two-phase `RenderState` (`vt/render.h:42-53,433-479`) is the intended render-path replacement.
- Callbacks fire synchronously inside `vt_write` and must not re-enter (`vt/terminal.h:78-83`); the
  clipboard-write/read callbacks additionally require a synchronous `reply` before returning
  (`:548-552,719-723`). Shim: enqueue `TerminalBackendEvent`s; for clipboard write, reply `SUCCESS`
  immediately and enqueue the copied text.
- `Event::Wakeup` is emitted by the reader loop after each `vt_write` batch, plus on
  `GridLinesChange::Changed` (`crates/terminal/src/terminal.rs:2343-2352`).

### 3.5 G4 — Kitty keyboard protocol (re-confirmed)

Unchanged finding: ghostty answers `CSI ? u` (`ghostty:src/terminal/stream_terminal.zig:492` dispatch,
`:1516-1523` reply) and there is still no option to suppress it (options 0–39 enumerated at
`vt/terminal.h:1093-1549`; `OPT_MODE_DEFAULT` cannot help because kitty flags are not a mode). Ledger
P7-008 pins the reply differentially. **Fill unchanged:** adopt `key::Encoder`
(`lg:key.rs:136` `set_options_from_terminal` syncs DECCKM/keypad/kitty flags/modifyOtherKeys each use;
ledger P8-001). P3 adds ctrl+alt / alt+shift rows to the `keys.rs` conformance oracle.

### 3.6 G7 — Hyperlink id (re-confirmed)

`ghostty_grid_ref_hyperlink_uri` (`vt/grid_ref.h:168-187`) returns the URI only; `vt/screen.h` exposes
`GHOSTTY_CELL_DATA_HAS_HYPERLINK` (`:192-196`) and `GHOSTTY_ROW_DATA_HYPERLINK` (`:291-295`); no id
accessor anywhere in `vt/*.h`. Zed's only id consumers: the hover-extent equality
(`crates/terminal/src/alacritty/hyperlinks.rs:98-118`) and the seam test (`crates/terminal/src/alacritty.rs:1114-1121`).
**Fill unchanged** (URI equality; ledger P6-001).

---

## 4. Cross-cutting constraints (not per-row)

1. **Threading model** — §3.4. Settle first; every other row assumes it.
2. **Coordinate-space translation** — one seam module owns all conversions between Zed's signed
   `Point` and ghostty's tagged spaces (`vt/point.h:49-59`); `grid_ref`/`point_from_grid_ref`
   (`lg:terminal.rs:364,414`) are the canonical converters. B15's `scrollback_position` is a Screen-space
   `y` in disguise.
3. **libghostty-rs is behind the v2 ghostty pin (new).** `de9fd9b0fa` binds `GHOSTTY_COMMIT =
   22d13172cd` (`libghostty-rs/crates/libghostty-vt-sys/build.rs:7`; 2026-08-06), an ancestor of
   `8867c37c5` by 498 commits, 30 of which touch `include/ghostty/vt/`. Concretely: the sys bindings
   still declare `ghostty_render_state_colors_get` (`bindings.rs:3162`) which `b4079f00c` removed from
   `render.h` (`Snapshot::colors()`, `lg:render.rs:483`, would fail to link); options 35–39
   (`UNKNOWN_SEQUENCE`, `UNKNOWN_MAX_BYTES`, `TERMINFO_NAME`, `CLIPBOARD_READ`,
   `CLIPBOARD_WRITE_MAX_BYTES`), data 38–40 (`VT_GROUND`, `CURSOR_AT_PROMPT`, `CLIPBOARD_WRITE_MAX_BYTES`),
   `ghostty_terminal_paste`, `ghostty_terminal_vt_write_until_ground`, `ghostty_render_state_clean`,
   `row_iterator_next_dirty`, `ROW_DATA_CELLS_RAW`/`GhosttyCellsView`, and the reply-based
   `GhosttyClipboardWrite` are absent or stale. Every row above cites the C header as the counterpart of
   record; `lg:` citations are present only where the wrapper exists at `de9fd9b0fa`. The vendoring
   ticket (#33) must either re-generate bindings against `8867c37c5` (v1's `tools/gen_bindings.rs` +
   `headers_sha256` guard) or pin ghostty to `22d13172cd` and forgo everything in §0.3/§6 that landed
   after it. This matrix assumes the former.
4. **Pre-1.0 API churn** — `vt.h` is still "in flux" (no libghostty-vt version tag, recon 6/6 §1);
   `GhosttyTerminalOptions` removal, `mode_get/set` removal and `colors_get` removal all happened inside
   this delta. Zed's seam tests (§2.R) are the regression net.
5. **Build chain** — Zig **0.16.0** (was 0.15.2); `-Dvt-features` (`ghostty:src/build/Config.zig:65-67,428-445`)
   can compile out `snapshot`, `kitty_graphics`, etc. for a smaller artifact — a #28/#39 decision, not parity.
6. **Query-answering defaults** — ghostty *silently drops* sequences that need responses unless effects
   are registered (`vt/terminal.h:55-62`). PTY profile registers `WRITE_PTY` (mandatory), `SIZE`,
   `DEVICE_ATTRIBUTES`, `XTVERSION`, `TITLE_CHANGED`, `BELL`, `CLIPBOARD_WRITE`, plus `TERMINFO_NAME =
   "xterm-256color"` (§6) and seeds default colors; display-only registers the subset (A9).
7. **Feature deltas to gate deliberately** — kitty graphics (`OPT_KITTY_IMAGE_STORAGE_LIMIT`,
   `vt/terminal.h:1230-1271`; set limit 0 until Zed renders images), sync output (2026), grapheme
   clustering (2027), visibility reports (2033), Kitty clipboard (5522, gated on `CLIPBOARD_READ`
   being NULL), `APC_MAX_BYTES`/`UNKNOWN_MAX_BYTES` (leave 0). None required for parity.

---

## 5. Divergence-ledger re-fire candidates (v2 harness)

From `docs/ghostty-migration/divergence-ledger.md` (v1, carried as `unverified` per salvage-policy rule 4).
Commits are from `git -C ~/Projects/refs/ghostty log --oneline a887df42..8867c37c5 -- src/terminal/`
(278 commits). "Expected outcome" is a prediction to be confirmed by the harness, not a verdict.

| Ledger id | v1 status | Upstream commit(s) in the delta | Expected outcome in v2 |
|---|---|---|---|
| **P5-001** scrollback page-granular (bytes conversion) | accepted | `5b2d3b7df` limit by physical lines, `86f81fb5b`, `10bc43420` byte limit optional, `f4c68d65e` runtime limits, `03d5fa268`/`a27e04e8f` C set/get, `739603b8a` | **Obsolete** — the `lines.div_ceil(215) × 512 KiB` conversion is deleted (A2). Replace with a new entry recording native page-granular over-retention (`vt/terminal.h:1383-1389`); the `scrollback_trim_past_limit` corpus row's "ghostty retains more" waiver survives, its arithmetic does not. |
| P5-002 mid-row selection trailing-space trim | accepted | `2ed67cadd` formatter pin-map redesign, `79aa256fa` formatter speedup, `997a2aff2` pending wrap in VT formatter, `f024d21fc` HTML newline fix | Re-fire `simple_selection_over_wide_chars_matches_alacritty`; `trim` semantics unchanged in the header (`vt/formatter.h:110-111`) so expected unchanged, but the formatter was rewritten. |
| P6-001 adjacent identical-URI links merge | accepted | `9313d580c` stale cursor style/hyperlink state after scroll clear, `d5c7e54ae` hyperlink reflow capacity | Re-fire the OSC 8 hover suite; G7 unchanged so the merge stands; `9313d580c` may remove a latent stale-hyperlink extra after `CSI 3 J` (relevant to G5 emulation). |
| P7-001 tabs occupy cells vs blanks | accepted | `e523cf810` cursor home after formatting tabstops, `7a9c369cf` preserve cursor when formatting tabstops, `bed20eb36` tabstop bit clear, `908961f8a` | Re-fire `tab_stops`; rendering unchanged, formatter tab output may differ — verdict expected to stand. |
| P7-003 destructive clears rotate into scrollback on alacritty | accepted | `9313d580c` | Re-fire `erase_operations`/`insert_delete_chars_lines`; expected unchanged verdict, possibly one fewer "extra" on the ghostty side. |
| P7-005 column-shrink reflow anchoring | accepted | `89b103dd5` more full-featured resize, `dde3d4d6b`/`a3c1caba5` resize failures safe, `88ed6bebf` faster wide-char reflow, `ec5b36961`/`d4e446c48` vectorized run scan, `46276d046` page recycling, `179161c08`/`c249b9de3`/`c5ca2db1b`/`4a88cc594` reflow memoization | Re-fire `reflow_shrink_and_grow`; the memoization commits are perf-only, but `89b103dd5` changes what `resize` does (cell geometry, sync-output reset, `vt/terminal.h:2009-2012`) — anchoring semantics must be re-adjudicated, not assumed. |
| P7-007 ANSI-mode DECRQM unanswered | accepted | `39ae85f04` DECRQSS, `cb2fef390`/`f973bd53b` DECRQSS SGR | Re-fire `decrqm_ansi_mode_reports`; DECRQSS ≠ DECRQM, expected unchanged. **New entry expected:** DECRQSS now answered by ghostty (alacritty silent) → extend `ghostty_extra_is_ignored_query_response`. |
| P7-008 kitty keyboard query answered | accepted | none (re-confirmed §3.5) | Stands. |
| P7-010 `content_text` trailing blank rows | fixed in seam | `2ed67cadd`, `79aa256fa` | Re-fire every corpus row's `ContentText` probe; the seam fix is expected to still be needed. |
| P7-011 BCE background lost (seam bug) | fixed in seam | `8838c37f4` fast print styles | Re-fire `bce_erase_with_background`; the content-tag read (B5) is expected to still be the fix. |
| P7-012 color-scheme report + XTVERSION | accepted | none for those two; **new** `74efadb44` XTGETTCAP answered, `39ae85f04` DECRQSS | Stands; **new entries expected** for XTGETTCAP (`TN` reply gated on `TERMINFO_NAME`, `vt/terminal.h:1499-1513`) and DECRQSS. |
| P7-013 erased cells keep SGR flags on alacritty | accepted | `8838c37f4` | Re-fire `erased_cells_drop_attribute_flags`; expected unchanged. |
| P7-014 cursor column after column-grow with pending wrap | accepted | `89b103dd5`, `997a2aff2` | Re-fire `recorded_top_process_viewer` 80→110; the one-column delta may have moved — re-adjudicate. |
| P7-015 alt-screen resize anchoring | accepted | `89b103dd5`…`a3c1caba5` | Re-fire `recorded_vi_editing_session`/`recorded_tmux_split_scroll` shrinks; transient-frame verdict expected to stand but the anchoring may differ. |
| P7-016 fuzz lane classes | open | `0aa71d02e` parser log size, `b53728241` APC unknown reporting, `6b990de5b` unknown-sequence C API | Re-run; the APC class may now be observable via `UNKNOWN_SEQUENCE` for triage. |
| P8-001 encoder options live from terminal state | verified (P8) | none (`lg:key.rs` unchanged) | Stands. |
| P8-003 theme-change color answers | verified (P8) | none | Stands. |
| **New (expected)** in-band resize report | — | `07af4612b` | ghostty emits a mode-2048 size report on `resize` when enabled; alacritty never does → new accepted entry (capability gain, mirrors P7-008). |
| **New (expected)** title report | — | `38e891e6c`/`ad27c989a` opt-in | Off by default = alacritty (no `CSI 21 t` handler in the fork; vte 0.15's `push_title/pop_title` are the only title ops, `vte-0.15.0/src/ansi.rs:689-692`) → no entry needed unless Zed enables it. |

Not re-fire candidates (no relevant upstream change): P2-001/002, P3-001…P3-010, P4-001…P4-005,
P6-002, P6-003, P6-004, P7-002, P7-004, P7-006, P7-009, P8-002.

---

## 6. Incidental new capabilities — verdicts (not parity rows)

These exist at 8867c37c5, have no alacritty counterpart, and Zed uses none of them today. Each is
classified as **settled here** (the matrix fixes the seam behavior; no ticket), **decision ticket**
(a concrete either/or with known trade-offs), or **fog** (no Zed consumer, no decision to make yet).

| Capability | Where | Zed today | Verdict | Rationale |
|---|---|---|---|---|
| `TERMINFO_NAME` (XTGETTCAP `TN`) | `vt/terminal.h:1499-1513`; `ghostty:src/terminal/c/terminal.zig:1341-1346`; `74efadb44` | Zed exports `TERM=xterm-256color` (`crates/terminal/src/terminal.rs:662`) | **Settled** | Set the option to the exported `TERM` value at creation; leaving it unset makes ghostty stay silent on `TN` (`:1507-1509`), which is also acceptable. One line in the PTY profile. |
| In-band resize (mode 2048) | `vt/modes.h:96`; `vt/terminal.h:973-976,2009-2012`; `ghostty:src/terminal/stream_terminal.zig:254-266`; `07af4612b` | none (alacritty lacks 2048) | **Settled** | Fully covered by registering `SIZE` (already mandatory, N8) and passing real cell pixel sizes to `resize` (D4). Capability gain; new ledger entry expected (§5). |
| `TITLE_REPORT` (CSI 21 t) | `vt/terminal.h:1441-1451`; `ghostty:src/terminal/stream_terminal.zig:1404` | none; alacritty fork has no `CSI 21 t` handler (silent) | **Settled** | Leave off (default) = parity and avoids the documented injection vector (`:1444-1446`). |
| Clipboard read (`OPT_CLIPBOARD_READ`) | `vt/terminal.h:626-818,1515-1524`; `e03475c0c`, `4f49dc2b8` | `ClipboardLoad` handler exists but never fires (`OnlyCopy`/`Disabled`) | **Settled for migration; product follow-up** | Leave NULL = exact parity (§0.3). Wiring it is a one-callback change gated on a user setting Zed does not have; that is a product decision outside the migration and not blocked by anything here. |
| Paste path (`ghostty_terminal_paste`, `REJECTED`, `allow_unsafe`, mode 5522) | `vt/paste.h:15-56,159-190`; `60a1ae2df`, `da27e6c90`, `876032316` | Zed's 8-line `paste` (`crates/terminal/src/terminal.rs:2324-2332`) | **Decision ticket (fold into #32)** | Sharp either/or: keep Zed's paste (byte-identical today, ledger P3-007 stays) **vs** adopt `terminal_paste` (gains unsafe-paste confirmation + mode-5522 events + streaming, changes sanitization semantics, needs a confirm dialog). Mode 5522 is moot while `CLIPBOARD_READ` is NULL (`vt/paste.h:24-31`). |
| `CURSOR_AT_PROMPT` (OSC 133) | `vt/terminal.h:1935-1944`; `ghostty:src/terminal/Terminal.zig:2240-2253`; `bdb566068` | Zed has no OSC 133 handling and no shell integration (grep `133`/`semantic_prompt` in `crates/terminal/src`: none); command boundaries are inferred from `\r` input (`crates/terminal/src/terminal.rs:2170-2176`) | **Fog** | No producer (Zed does not inject shell integration) and no consumer. Could later replace the `\r` heuristic for B15 and enable `select_output`/prompt-boundary line selection, but that is a feature, not a migration decision. |
| `UNKNOWN_SEQUENCE` (APC only) + `UNKNOWN_MAX_BYTES` | `vt/terminal.h:343-423,1478-1497`; `6b990de5b`, `b53728241` | vte/alacritty drop unknown sequences silently | **Fog** | Only APC is reported (`:344-347`); Zed has nothing to do with it beyond optional debug logging. Leave `UNKNOWN_MAX_BYTES = 0` (capture off). Useful for the P7-016 fuzz triage only. |
| Desktop notification (OSC 9/777) + progress (OSC 9;4) | `vt/terminal.h:820-912,1404-1419`; `lg:terminal.rs:2053,2066`; `c3655ba25`, `47d602c42` | none (alacritty ignores both) | **Fog** | No Zed UI exists; registering nothing = parity. A notifications/progress feature would be its own product ticket after the migration. |
| `vt_write_until_ground` / `DATA_VT_GROUND` | `vt/terminal.h:2080-2110,1920-1933`; `a69a591af` | n/a | **Settled (absorbed into G5)** | It is the injection primitive §3.3 needed; no separate decision. |
| Snapshot encode/decode (`vt/snapshot.h`) | `vt/snapshot.h:284-515`; `lg:snapshot.rs:124-491` | none | **Fog** | Relevant to the differential harness (state serialization) and possibly terminal persistence; neither is parity. Harness ticket may pick it up. |
| Dirty-row iteration + `ROW_DATA_CELLS_RAW` | `vt/render.h:496,625-627,252-266`; `ad6e72ddc`, `0d37f2d34`, `0e8b7bea6` | full snapshot per frame | **Settled (perf gate option)** | Not parity; the #39 perf gate decides whether to adopt them against v1's "~2 FFI/cell" floor. Requires re-generated bindings (§4.3). |

---

**Final tally: 100 rows — 70 Covered / 13 Partial / 17 Gap rows → G1–G8 logical gaps** (v1: 93 — 66 / 10 / 17
as tabulated; 65 / 11 / 17 as stated → G1–G8). Re-validated 2026-08-25 on `migration/libghostty2` @ `38c5dd7c98` against ghostty `8867c37c5` and
libghostty-rs `de9fd9b0fa`.
