# Find-in-terminal and URL/path detection without alacritty RegexSearch

_Wayfinder ticket [#35](https://github.com/xipeng-jin/zed/issues/35). Resolves the
G1 gap from the [parity matrix](parity-matrix.md) (§3.1): libghostty-vt has no
search API — only grid traversal, row/cell queries, tracked grid refs, and
`GridRef::hyperlink_uri` for OSC 8._

## Decision summary

A Zed-owned `grid_search` layer in the seam. libghostty-vt is used only for
**text and flag extraction**, never for matching:

1. **Engine**: extract text from the ghostty grid, match with the Rust `regex`
   crate, map match byte offsets back to grid points. This generalizes the
   pattern the path-hover branch already uses today
   (`crates/terminal/src/alacritty/hyperlinks.rs:29,314-481`).
2. **Search extraction**: bulk `format_selection_buf` + wrap-flag line map;
   the precise byte↔cell walk runs only on rows containing matches.
3. **Threading**: stateless per-call sandwich — foreground extract →
   background regex → foreground map. No cache; a benchmark gates this.
4. **Bounds**: whole scrollback, no cap (parity).
5. **Match representation**: plain Zed-owned Screen-space point ranges;
   parity invalidation (resize clears; drift self-heals via re-search).
   No `TrackedGridRef`s for match lists.
6. **Hover**: per-logical-line, as today — OSC 8 first, then URL regex, then
   path regexes; line bounds via a wrap-flag walk with alacritty's
   100-wrapped-row cap; all hyperlinks.rs string logic unchanged.
7. **Semantics**: regex per logical-line slice (hard-newline barrier is
   structural); smart-case, empty-match discard, and size-limit /
   compile-failure → no-matches port exactly.
8. **Oracle**: existing `hyperlinks.rs` and `terminal_path_like_target.rs`
   suites re-hosted on a ghostty-backed harness, plus three new test groups
   (characterization, mapping, benchmark).

Alternatives rejected at the root: porting alacritty `search.rs` (grid-shaped
regex-automata machinery would put per-cell FFI calls in the regex inner
loop), and proposing an upstream search API (timeline outside this effort;
would also move Zed's search semantics into someone else's API).

## 1. Search: `find_matches` replacement

### Extraction — bulk text, lazy precise mapping

The naive per-cell extraction walk is too expensive for whole-buffer search:
~3+ FFI calls per cell (`grid_ref` → `cell` → `codepoint`, more for
graphemes) at the spike's measured ~0.35 µs/cell over 10k lines × 200+ cols
is hundreds of ms on the foreground thread. Alacritty never paid this because
`RegexIter` reads the grid in-process.

Instead, extraction is two-phase:

- **Phase 1 (bulk, one FFI call)**: `Terminal::select_all()` +
  `format_selection_buf` with `FormatOptions { unwrap: false, trim: false }`
  (`crates/ghostty_vt/src/selection.rs:218,404,547`) yields the whole
  buffer's text. A per-row pass over `Row::is_wrap_continuation`
  (`crates/ghostty_vt/src/screen.rs:299`, ~1 FFI call per row) builds the
  **logical-line → starting-screen-row map** and joins wrapped rows into
  logical lines. Result: an owned `String` (or `Vec<String>` of logical
  lines) plus the line map.
- **Phase 2 (precise, per match-row only)**: after matching, each logical
  line that contains a match gets a per-cell walk building the
  spacer/grapheme-aware **byte↔cell offset table** — skip `SpacerTail` /
  `SpacerHead` (`crates/ghostty_vt/src/screen.rs:441-443`), expand
  multi-codepoint graphemes via `GridRef::graphemes`, accumulate UTF-8
  lengths. Match byte ranges resolve through the table to inclusive
  Screen-space cell ranges. FFI cost scales with **matches**, not cells.

**Pinned risk**: phase 1's correctness depends on `format_selection`'s exact
semantics (empty-cell rendering, trailing whitespace, spacer skipping,
row separators). Characterization tests (see §4) pin these; if they don't
hold, the fallback is running the phase-2 per-cell walk over the whole
buffer — slower but correct, and the seam interface doesn't change.

### Threading — stateless per-call

The core is `!Send` and foreground-owned (spike verdict #1), so grid access
must happen on the foreground thread; only the regex can leave it. Today's
`find_matches` is a pure `background_spawn` holding the FairMutex
(`crates/terminal/src/terminal.rs:2683-2689`); it becomes a sandwich:

```
find_matches(query) -> Task<Vec<SearchMatch>>:
  foreground: bulk extract + line map        (one FFI call + row flags)
  background: per-line regex over owned text (find_iter per logical line)
  foreground: byte↔cell mapping on match rows
```

No extraction cache. Every call reflects the current grid; there is no
invalidation state to get wrong. **Perf gate**: a benchmark extracts at the
maximum configured scrollback (`max_scroll_history_lines` can reach 100k
lines — tens of MB of transient text). If the bulk extract exceeds a few ms,
retrofit a snapshot cache keyed on a seam-owned generation counter — but only
then.

### Bounds and match representation

- **Whole scrollback, no cap** — parity with `search_matches` iterating
  `topmost..bottommost` (`crates/terminal/src/alacritty.rs:1009-1023`).
  Silently missing old history is a worse failure than a slower search; the
  perf gate is the guard.
- Matches are **plain Zed-owned point ranges in ghostty Screen space**
  (`PointSpace::Screen`: y from the top of scrollback,
  `crates/ghostty_vt/src/terminal.rs:796-805`), inclusive, mirroring the
  Zed-owned type strategy from the color-contract decision (#31). Screen
  coords survive viewport scrolling structurally and — unlike alacritty's
  convention, where every new output line renumbers the grid — only shift
  when scrollback hits capacity and prunes. Drift self-heals through the
  existing search-bar re-query cadence; **resize clears matches** exactly as
  today (`crates/terminal/src/terminal.rs:1616-1621`).
- `TrackedGridRef` is rejected for match lists: two tracked refs per match ×
  potentially thousands of matches taxes every terminal mutation, against
  the API's own "use sparingly" guidance
  (`crates/ghostty_vt/src/screen.rs:159-161`). If QA later shows the *active*
  match drifting during heavy output, a single tracked pair for it is a
  cheap, contained retrofit.

### Regex semantics (parity checklist)

- **Smart-case**: any uppercase in the query → case-sensitive; else
  `RegexBuilder::case_insensitive(true)` (port of
  `alacritty:src/term/search.rs:39-41`).
- **Hard-newline barrier**: structural — `find_iter` runs per logical-line
  slice, so no pattern (including `(?s).`, `[^x]`, literal `\n`) can match
  across logical lines. Soft-wrap crossing is likewise structural: wrapped
  rows are joined into one logical line before matching.
- **Empty-match discard**: filter zero-length matches from `find_iter`.
- **Degrade-on-complexity**: `RegexBuilder::size_limit`/`dfa_size_limit` at
  defaults comparable to alacritty's; compile failure → empty results (as
  `Search::new` errors do today).
- Text mode continues to pass `regex::escape`d literals
  (`crates/terminal_view/src/terminal_view.rs:1242-1244`) — unchanged.

## 2. Hover: URL/path detection under the mouse

`find_from_grid_point` (`crates/terminal/src/alacritty/hyperlinks.rs:90-150`)
keeps its exact shape; only the grid layer changes:

1. **OSC 8 first**: `Cell::has_hyperlink` at the hover point; if set, read
   `GridRef::hyperlink_uri` and walk cells left/right (crossing row
   boundaries, as today's `Boundary::Cursor` stepping does) while the
   neighbor's URI matches, producing the underline range.
2. **Logical-line bounds** via a wrap-flag walk (direct port of
   `line_search_left/right`, `alacritty:src/term/search.rs:593-616`):
   step up while the row above `is_wrapped`, down while the current row
   `is_wrapped`, keeping alacritty's **100-wrapped-row cap** against
   pathological fully-wrapped buffers. Ghostty's `select_line` was rejected
   to avoid coupling a hot hover path to ghostty's uncapped line-selection
   semantics.
3. **Per-cell extraction of that one logical line** (≤100 rows — cheap),
   building the same byte↔cell table as search phase 2; hit-testing
   ("match contains hover point") resolves through it.
4. **Unchanged string pipeline**: URL regex → punctuation sanitization →
   configurable path regexes (with timeout) → `PathWithPosition`. All of it
   is grid-independent today and stays byte-for-byte.

Hover does **not** reuse the whole-buffer extraction (no cache exists under
the stateless design, and a full-buffer extract per mouse-move is waste).
This resolves the parity matrix's open question in favor of per-line, as
today.

## 3. Seam interface sketch

The `grid_search` module owns:

- `extract_logical_lines(&Terminal) -> ExtractedBuffer` — phase-1 bulk
  extract + line map (search) and a single-line variant for hover.
- `cell_map_for_line(&Terminal, logical_line) -> ByteCellMap` — phase-2
  walk, shared by search mapping and hover hit-testing.
- `search(&Terminal, &SearchQuery) -> Vec<SearchMatch>` orchestration split
  across the threading sandwich by the caller (`Terminal::find_matches`).
- Hover entry point mirroring `find_from_grid_point`'s signature with seam
  types.

Public types (`SearchMatch`, point ranges) are Zed-owned mirrors on the
existing `terminal::` re-export paths, per the #31 pattern — no downstream
churn in `terminal_view`'s `SearchableItem` implementation.

## 4. Test plan

- **Oracle suites carry over**: `hyperlinks.rs` tests (pure-string tests
  untouched; grid-dependent tests — including the wide-char/spacer
  regressions like `issue_alacritty_8586` — re-hosted on a ghostty-backed
  `build_test_term` that feeds the same content as bytes via `vt_write`) and
  `terminal_view/src/terminal_path_like_target.rs`.
- **Characterization tests** pin `format_selection_buf` semantics the bulk
  extract depends on: empty cells, trailing whitespace, `unwrap:false` row
  separators, spacer handling around wide chars at wrap boundaries.
- **Byte↔cell map unit tests**: wide chars (CJK), `SpacerHead` at soft-wrap,
  multi-codepoint graphemes, and matches beginning/ending on each.
- **Benchmark** (the perf gate): bulk extract + line map at max configured
  scrollback; the existing `*_hyperlink_benchmark` set carries over for the
  hover path.
