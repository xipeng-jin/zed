# Alacritty performance baseline (P7)

The pre-swap baseline required by [SPEC.md §7](SPEC.md#7-verification) and
[verification-strategy.md §6](verification-strategy.md#6-performance): the
P8 gate is that no scenario regresses more than **20%** against these
numbers on the same machine, with no cliff-shaped anomaly, and the shipped
native core asserting ReleaseFast.

Recorded 2026-07-17 on the Linux development machine (release build,
`script/terminal-perf-baseline` + `script/terminal-flood-bench`). The four
seam scenarios feed pre-built byte streams headless through
`alacritty::TerminalBackend` in 64 KiB pump-sized chunks with a `Content`
snapshot per chunk (`crates/terminal/src/differential.rs`, `mod perf`); the
sustained-flood scenario drives the full PTY seam (child → reader thread →
bounded channel → foreground ingest) and landed at P4.

| Scenario | Volume | Result |
|---|---|---|
| `colored_dump` (256-color fg+bg per line) | 16.4 MiB | **178.3 MiB/s** |
| `wide_char_cjk` (CJK/kana/Hangul/Cyrillic flood) | 10.4 MiB | **151.7 MiB/s** |
| `alt_screen_churn` (2 000 enter/redraw/exit cycles) | 2.9 MiB | **125.1 MiB/s** |
| `sustained_scroll` (20 000 scroll+snapshot ops over 10 000 history lines) | — | **32 855 ops/s** |
| `sustained_flood` (P4, full PTY seam) | 263.8 MiB | **83.7 MiB/s**; foreground turns mean 24.3 µs, max 1.14 ms; echo latency mean 156 µs, max 366 µs |

Re-record on the same machine before comparing the ghostty backend at P8;
the macOS gate (§8.1) re-records the whole suite on macOS hardware,
including the flood scenario targeting the ~1 KiB master-read cap.

## P8 swap comparison (2026-07-21)

Same machine, same release build, alacritty re-recorded in the same run
(`script/terminal-perf-baseline` + `script/terminal-flood-bench`, which now
run both backends):

| Scenario | alacritty (re-recorded) | ghostty (P8) | Δ | ≤20%? |
|---|---|---|---|---|
| `colored_dump` | 180.4 MiB/s | 72.4 MiB/s | −59.9% | **fails** |
| `wide_char_cjk` | 178.4 MiB/s | 179.5 MiB/s | +0.6% | passes |
| `alt_screen_churn` | 132.9 MiB/s | 174.3 MiB/s | +31.1% | passes |
| `sustained_scroll` | 33 147 ops/s | 10 119 ops/s | −69.5% | **fails** |
| `sustained_flood` | 84.6 MiB/s; turns mean 17.3 µs, max 1.20 ms; echo mean 82 µs | 88.6 MiB/s; turns mean 6.2 µs, max **0.36 ms**; echo mean 97 µs | +4.7% throughput, 3.3× lower stall bound | passes |

Analysis of the two failures (both pending adjudication on ticket #51 —
they exceed the §7 gate and need an owner-signed amendment or further
work; neither is a seam regression the swap PR can absorb):

- `colored_dump` is **write-bound in the core**: feeding the bytes without
  any snapshots measures 87.3 MiB/s, so ~85% of the scenario cost is
  `vt_write` itself. The synthetic stream changes the SGR fg+bg pair every
  line, which stresses ghostty's interned-style machinery
  (`RefCountedSet` insert/lookup per line) where alacritty stores colors
  per cell. The native core is the pinned ReleaseFast prebuilt; the seam
  contributes ~13% of the scenario.
- `sustained_scroll` (20 000 full snapshot rebuilds while scrolling) hits
  the render-state FFI floor: even collecting only the raw cell per cell —
  two FFI calls — costs ~45 µs per 120×40 snapshot against a ~38 µs
  gate target; the full conversion costs ~190 µs. In production the pump
  takes at most one snapshot per rendered frame (≤120/s), where 190 µs
  is ~2% of a 120 Hz frame budget; the scenario's 20 000 snapshots/s
  shape has no production analogue. A pure-scroll row-reuse cache in the
  seam could close most of the gap if the letter of the gate is preferred
  over adjudication.

The seam snapshot path was already tightened for this comparison (the
initial measurement was 68.4 MiB/s / 5 020 ops/s): the per-cell iterator
FFI reads dropped from ~5 to 2 (style fetched once per same-style run,
grapheme buffers only for grapheme cells, per-cell allocations removed),
which took `wide_char_cjk` and `alt_screen_churn` from −19.5% and +8.5%
to +0.6% and +31%.
