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
