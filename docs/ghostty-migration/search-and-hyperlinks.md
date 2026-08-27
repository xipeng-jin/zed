# Find-in-terminal and URL/path detection without alacritty RegexSearch (v2)

**Status: resolved for v2** — wayfinder ticket
[#35](https://github.com/xipeng-jin/zed/issues/35), map
[#27](https://github.com/xipeng-jin/zed/issues/27). Re-validated 2026-08-27
against `main` @ `38c5dd7c98`, ghostty `8867c37c5`, libghostty-rs `de9fd9b0fa`
(vendored as `crates/ghostty_vt{,_sys}` by #33). The v1 record (grilling
2026-07-15; landed as P6 `afa593b2bd` / `a24e214438` on `migration/libghostty`,
execution tickets #47 / #48) is the starting text; every section states whether
it is re-confirmed or amended. Resolves the G1 gap of the
[parity matrix](parity-matrix.md) (§3.1, re-confirmed: still no search C API —
`ghostty:src/terminal/search/*.zig` is unexported; `grep -ri search include/`
matches only prose).

## 0. Per-point re-validation

| # | v1 decision | v2 |
|---|---|---|
| 1 | Engine: extract text via ghostty, match with the Rust `regex` crate, map byte offsets back to grid points | **Re-confirmed** |
| 2 | Bulk extract + wrap-flag line map; byte↔cell walk on match rows only | **Re-confirmed**; extraction re-measured (§1.1) |
| 3 | Stateless per-call sandwich, cache is a retrofit behind a perf gate | **Re-confirmed; gate amended to numbers** (§1.2) |
| 4 | Whole scrollback, no cap | **Re-confirmed** |
| 5 | Plain Zed-owned Screen-space ranges; no `TrackedGridRef` per match | **Re-confirmed** |
| 6 | Hover per logical line: OSC 8 walk → URL regex → path regexes; 100-wrapped-row cap | **Re-confirmed; amended for the #54884 contract** (§2); cap kept, P6-002 re-adjudicated (§2.3) |
| 7 | Regex semantics ported exactly | **Re-confirmed** |
| 8 | Oracle suites re-hosted byte-identical | **Amended**: +#54884 hover suites, +#52454 cwd suites, +page-eviction scenario (§4) |
| — | (new) per-line cwd inputs as seam reads | **New** (§3) |
| — | (new) formatter entry point | **`format_buf` re-confirmed over the streaming writer** (§1.1) |

Alternatives rejected at the root are unchanged: porting alacritty `search.rs`
(per-cell FFI calls in the regex inner loop) and proposing an upstream search
API (still absent at `8867c37c5`; outside this effort's timeline).

## 1. Search: `find_matches` replacement — re-confirmed

### 1.1 Extraction — bulk text, lazy precise mapping (re-confirmed, re-measured)

Two phases, as v1 built them (`ghostty/grid_search.rs`, salvaged by file under
[salvage-policy.md](salvage-policy.md) rule 3):

- **Phase 1 (bulk)**: `Terminal::select_all()` + one formatter call with
  `Format::Plain, unwrap: false, trim: false`, split on `\n`; a per-row
  `Row::is_wrap_continuation()` walk (`grid_ref` → `row` → `row_get`, 3 FFI
  calls/row) builds the logical-line map (`LineGeometry { first_row,
  first_column, row_count }`). Hard newlines are a structural barrier.
- **Phase 2 (precise, match rows only)**: the spacer/grapheme-aware byte↔cell
  table (`SpacerTail` extends the previous span, `SpacerHead` skipped,
  never-written interior cells emit one space each only when text follows on
  the row, graphemes via `GridRef::graphemes`), `partition_point` lookup,
  per-line `total_bytes != text_len` drift guard. FFI cost scales with
  matches, not cells.

**Cheaper extraction on the new core — investigated, none found.** Headers at
`8867c37c5`: `ghostty_formatter_format(writer)` is the same formatter with a
streaming sink (`GhosttyWriter { write, userdata }`, called synchronously,
may not re-enter the terminal) and exposes no pin-map / byte-offset facility;
`GHOSTTY_RENDER_STATE_ROW_DATA_CELLS_RAW` is a **render-state** row datum —
viewport only — so it cannot feed a scrollback extract. Measured 2026-08-27
(scratch crate over the vendored `ghostty_vt_sys`, release, byte cap lifted,
`SCROLLBACK_MAX_LINES = 100 000` → 99 765 total rows × 120 cols, 10 runs):

| Stage | v2 @ `8867c37c5` | v1 (#47, `MAX_SCROLL_HISTORY_LINES` + 50 rows) |
|---|---|---|
| `format_buf` (select_all, unwrap=false, trim=false) | **27.1 ms** median | ~87 ms |
| `format_buf` (selection = NULL, whole screen) | 27.3 ms | — |
| streaming `ghostty_formatter_format` (writer → `Vec<u8>`) | 27.6 ms | — |
| wrap-flag walk (3 FFI calls/row) | **15.6 ms** | ~12 ms |
| **foreground total (extract + line map)** | **≈43 ms** | 105.4 ms |
| per-cell walk of the whole buffer (fallback shape) | 1.87 s | — |

The core's bulk format is ≈3.2× faster; the streaming writer costs the same
and only removes the `OUT_OF_SPACE` retry. Linear scaling puts the foreground
stall at **≈4.3 ms at the 10k default scrollback**.

Decisions:

- **Formatter entry point**: `format_buf` through the safe `Formatter`
  wrapper (pre-sized `total_rows × (cols + 1)`, resize-and-retry on
  `OUT_OF_SPACE`). The streaming writer stays unbound in `ghostty_vt` (#33
  left it raw); binding it buys no time.
- **Wrap-flag walk kept** (36 % of the cost). Rejected: a second
  `unwrap: true` pass (+27 ms) and regexing physical rows first (misses
  soft-wrap-crossing matches). A bulk row-flag export upstream is noted as a
  nice-to-have, not on the route.
- The pinned risk (formatter semantics) stays pinned by the v1
  characterization tests; the fallback (per-cell walk over the whole
  buffer) is now measured at 1.87 s and remains a correctness fallback only.

### 1.2 Threading — stateless per call (re-confirmed; gate amended)

The core is `!Send` and foreground-owned ([pty-threading-architecture.md](pty-threading-architecture.md)),
so the v1 sandwich stands: foreground `prepare_search` → background
`PreparedSearch::find_matches` (Send) → foreground `search_matches`. No
extraction cache; every call reflects the current grid.

**Cache-retrofit gate, restated in numbers** (v1's "a few ms" was exceeded at
105 ms and is retired):

- Accepted: **≤ 5 ms foreground at the default scrollback (10k rows)** and
  **≤ 50 ms at the maximum (100k rows)**, measured by the extraction
  benchmark (`script/terminal-search-bench`, re-armed at these thresholds).
- Retrofit trigger: the v2 harness measures **> 16 ms at the default
  scrollback** on the v2 CPU policy. The retrofit is a snapshot-keyed
  extraction cache (key = a seam-owned generation counter bumped per
  `vt_write` batch and per resize), invalidating on any PTY byte.

Rationale: a search is user-initiated and rarer than a frame; at the default
scrollback the stall is under a frame, and during a running command any cache
is invalidated by every batch anyway.

### 1.3 Bounds, match representation, regex semantics (re-confirmed)

- Whole scrollback, no cap (parity with `topmost..bottommost`).
- Matches are plain Zed-owned inclusive point ranges; Screen space stays
  internal to `grid_search` (`ScreenPoint`), converted to the grid convention
  (`row − scrollback_rows`) only at the exit; a failed `scrollback_rows()`
  read drops the result rather than leaking Screen space. Resize clears
  matches above the seam, as today; drift self-heals via the re-search
  cadence. `TrackedGridRef` per match remains rejected ("use sparingly"); a
  tracked pair for the *active* match is the contained retrofit if QA shows
  drift.
- Smart-case (`!pattern.chars().any(char::is_uppercase)` →
  `case_insensitive`), empty-match discard, `RegexBuilder` size limit /
  compile failure → no matches, `regex::escape`d text mode — all as v1
  landed them.

## 2. Hover: URL/path detection under the #54884 contract — amended

### 2.1 What #54884 changed (baseline `main` @ `38c5dd7c98`)

- `Content` carries raw grid-shape reads every `sync()`:
  `total_lines, display_offset, columns, screen_lines`
  (`crates/terminal/src/terminal.rs:489-506`, filled by `make_content`,
  `crates/terminal/src/alacritty.rs:881-931`).
- `GridLinesChange::{Unchanged, Changed}` (no payload) is derived in
  `adjusted_last_hovered_word` (`alacritty.rs:823-879`): `Changed` iff
  `columns`/`screen_lines` differ from `last_content` **or**
  `total_lines_delta != display_offset_delta` (both `checked_signed_diff`;
  overflow ⇒ `Unchanged`). Semantics: "content grew by exactly what the
  viewport scrolled ⇒ the visible lines did not move".
- `adjusted_last_hovered_word` keeps `last_hovered_word` only on
  `Unchanged`; if `total_lines_delta != 0` it shifts `word_match` lines by
  `−display_offset_delta`, preserving `id` and `word`; otherwise `None`.
- `HoveredWord.id` is minted by `next_link_id()` inside `update_selected_word`
  only when `(word, word_match)` differ from the previous word;
  `terminal_element.rs:1370` paints on `id` equality only. `clear_hyperlink`
  emits `NewNavigationTarget(None)`. `schedule_find_hyperlink` throttles at
  5 px / 100 ms (`FIND_HYPERLINK_THROTTLE*`), distinct from the per-regex
  `path_hyperlink_timeout: Duration`. `sync()` calls `refresh_hovered_word`
  on `Changed` and emits `Wakeup` (`terminal.rs:2343-2352`); the wakeup
  counts are asserted by the hover suites (`terminal.rs:5293-5460`).

### 2.2 Seam contract (amended)

The seam supplies two things and nothing else:

1. **The four grid-shape reads**, one `ghostty_terminal_get_multi` per
   `sync()`: `total_lines = TOTAL_ROWS`, `columns/screen_lines = COLS/ROWS`,
   `display_offset = scrollbar.total − scrollbar.len − scrollbar.offset`
   (`DATA_SCROLLBAR`, axis inversion per parity row B12).
2. **`HyperlinkMatch { text, is_url, range }`** from
   `find_from_terminal_point`, in the grid convention.

**`GridLinesChange` is derived by porting the #54884 arithmetic verbatim**
over those reads (parity row B13). Ids, the throttle, `clear_hyperlink`,
`update_selected_word`, and the wakeup-on-`Changed` path are Zed-owned and
migrate unchanged. Known consequence, accepted: ghostty prunes scrollback at
page granularity, so at the cap `TOTAL_ROWS` drops by a page while the pinned
viewport keeps `display_offset` — the arithmetic reports `Changed`, hover is
invalidated and re-run (one extra `Wakeup`); it can never produce a wrong
shift. Anchoring `HoveredWord.word_match` with two `TrackedGridRef`s (which
would make the delta math unnecessary) is the follow-up the parity matrix
names, out of this spec.

### 2.3 Pipeline (re-confirmed; cap re-adjudicated)

`find_from_grid_point` keeps its shape on the ghostty backend
(`ghostty/hyperlinks.rs`, salvaged by file):

1. **OSC 8 first**: `Cell::has_hyperlink` → `GridRef::hyperlink_uri`, extent
   walk with `PointBoundary::Cursor` stepping, equality by URI (ghostty
   exposes no OSC 8 `id`; ledger P6-001).
2. **Logical-line bounds** via the wrap-flag walk (`line_search_left/right`)
   with the **100-wrapped-row cap** (`MAX_SEARCH_LINES`). **Baseline
   finding**: `main`'s `alacritty/hyperlinks.rs` calls alacritty's
   `line_search_*`, which Zed's fork walks *uncapped* — the cap is a
   divergence v1's spec introduced, not one it inherited. **Kept**: the
   whole-buffer per-cell walk measures 1.87 s at 100k rows (§1.1), and a
   fully soft-wrapped buffer would pay it on every throttled mouse move.
   P6-002 moves from `unverified` to **accepted by design**, citing that
   number. `select_line` stays rejected for the hot path.
3. **Per-cell extraction of that one logical line** (≤ 100 rows) into
   `HoverLine { text, spans, hovered_offset }`; base chars only (P6-003).
4. **Unchanged string pipeline**: `URL_REGEX` → `sanitize_url_punctuation`
   (byte-range form) → `path_match` with the configurable regexes and
   `path_hyperlink_timeout` → `normalize_hyperlink_match` / `PathWithPosition`.

Hover does not reuse the whole-buffer extraction (no cache under §1.2).

## 3. Per-line cwd inputs (#52454) as seam reads — new

`scrollback_position(line, history_size) = history_size + line`,
`cwd_at_line`, `record_cwd_change`, `pending_cwd_boundary`, and
`process_hyperlink(…, history_size)` (`terminal.rs:1497-1500, 1798-1812,
2170-2176, 2841-2890`) are Zed-owned and migrate unchanged. The seam supplies:

- `history_size` ≡ `GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS`, cursor line ≡
  `GHOSTTY_TERMINAL_DATA_CURSOR_Y` (terminal-level, active space), read at
  the same three moments as today: `\r` input before `write_to_pty`,
  `record_cwd_change`, and hover. With no lock the "must be read inside
  `sync()`" constraint disappears; **`process_hyperlink` keeps its
  `history_size` parameter** so the function stays pure for its tests.
- **Eviction guard (locks the #29 / #30 split)**: keep the
  `history_size >= scrolling_history` heuristic **and** treat any
  non-monotonic decrease of `SCROLLBACK_ROWS` between reads as "cap
  reached" (page-granular pruning, [pty-threading-architecture.md](pty-threading-architecture.md) §3.5).
  The 9 cwd tests never parse bytes and stay byte-identical. Anchoring each
  `CwdHistoryEntry` with a `TrackedGridRef` and dropping the heuristic (parity
  row B15) is the same follow-up as §2.2's — one ticket, after the swap.

## 4. Test plan and ledger — amended

Re-hosted **byte-identical in expectations** on the ghostty-backed harness
(bytes via `vt_write`, cursor from seam queries, CSI E for
`move_down_and_cr`):

1. The 36 `alacritty/hyperlinks.rs` scenarios (incl. the three `should_panic`
   messages with grid coordinates), v1's 4 OSC 8 walk tests, and the 5
   `#[perf]` hover benchmarks.
2. The #54884 hover suites: `test_ctrl_hover_with_changing_content`,
   `…_changing_bounds`, `…_modifier_change_only` (wakeup counts exact) plus
   the 8 ctrl-click / mouse-mode tests (`terminal.rs:4766-4953`).
3. The 9 #52454 cwd tests (`terminal.rs:5720-5850`) — Class B, untouched.
4. `terminal_view/src/terminal_path_like_target.rs` (15 GPUI tests) — Class B.
5. v1's 15 engine tests (oracle-compared against the alacritty backend on
   identical bytes), the byte↔cell map set, and the formatter
   characterization tests.
6. **New**: a page-eviction scenario — fill past `SCROLLBACK_MAX_LINES`,
   assert `GridLinesChange::Changed` with a re-hover and no shifted
   `word_match`, and `cwd_at_line` falling back to `working_directory()`
   after the non-monotonic `SCROLLBACK_ROWS` dip.
7. **Benchmark** re-armed at the §1.2 thresholds (≤ 5 ms @ 10k, ≤ 50 ms @
   100k; retrofit trigger > 16 ms @ 10k).

Divergence ledger (carried per [salvage-policy.md](salvage-policy.md) rule 4):

| Entry | v2 status | Owner |
|---|---|---|
| P6-001 adjacent distinct OSC 8 links with identical URIs merge | unverified → re-fire | this ticket |
| P6-002 100-wrapped-row hover cap | **accepted by design** (§2.3) | this ticket |
| P6-003 URL hover text drops zerowidth combining marks | unverified → re-fire | this ticket |
| P6-004 vi cursor does not ride content rotation | unverified | seam ticket (#36) |
| **P6-005 (new)** trailing never-written cells do not match (`"ab "` vs a row ending in `ab`: alacritty feeds them as spaces, the ghostty extract stops at the last text cell) | accepted; pinned by `trailing_unwritten_cells_do_not_match_unlike_alacritty` | this ticket |

Padding rows to width was rejected for P6-005 (adds `rows × cols` bytes to
every extract to reproduce an alacritty quirk on purpose). The grapheme
improvement #47 noted (combining marks matchable on ghostty, never on
alacritty) is unobservable regression-wise and is not an entry.

## 5. Seam interface (re-confirmed)

`grid_search` owns `SearchQuery::new`, `prepare_search` /
`PreparedSearch::find_matches` / `search_matches`, `extract_logical_lines`,
`cell_map_for_line`, `resolve_matches`; `hyperlinks` owns
`find_from_grid_point` and the ported string pipeline; the backend exposes
`find_from_terminal_point` and the four grid-shape reads. Public types stay
Zed-owned mirrors on the existing `terminal::` paths (#31 pattern) — no churn
in `terminal_view`'s `SearchableItem`.
