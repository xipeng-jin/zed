# Salvage policy: what v1 carries into v2, and how

**Status:** decided 2026-08-25 (ticket [#83](https://github.com/xipeng-jin/zed/issues/83), map [#27](https://github.com/xipeng-jin/zed/issues/27)).
**Applies to:** every artifact on `migration/libghostty` (v1, tip `e537270dac`, 50 commits over `af7de9a03c`) with respect to `migration/libghostty2` (v2 = `main` @ `38c5dd7c98`).

## Context

v1 executed P0–P8 of SPEC v1 and closed the perf gate. v2 restarts on a moved baseline: upstream Zed rewrote the seam files it touches most (`terminal.rs` +1,213/−309, `alacritty.rs` +107/−6 in exactly the region P1 restructures), libghostty-rs broke its constructor / scrollback / `Error` surface (`51bf4bf732 → de9fd9b0fa`, +1,815/−277), and ghostty moved 918 commits (Zig 0.16, native scrollback-lines, dirty/raw-cell render reads, clipboard read).

Evidence gathered for this decision:

| Class | On v1 | Drift vs the v2 baseline |
|---|---|---|
| 1. Decision records + SPEC | 11 files, 3,599 lines, none on `main` | Prose; specific sections invalidated by recon 4/6–6/6 |
| 2. Vendored crates | `ghostty_vt` 9,267 lines, `ghostty_vt_sys` 5,143 lines | v1's delta to the safe crate is **55 lines**; its real asset is `build.rs` (697 changed lines) + `ghostty_pin.toml`. Upstream crate cannot be rebased mechanically |
| 3. Seam code | 12,839 insertions in `crates/terminal`; 3 workflows; 6 scripts; nix package | `git merge-tree main migration/libghostty` conflicts **only** in `terminal.rs`, `alacritty.rs`, `Cargo.toml`, `Cargo.lock`; every phase P1–P8 conflicts in the first two; all new files auto-merge. `ghostty.rs` uses 6 broken libghostty-rs APIs |
| 4. Ledger + perf baseline | 42 ledger entries; 3 perf tables | P5-001 obsolete; P7 reflow rows need re-adjudication; perf was host-native, upstream default is now `-Dcpu=baseline` |
| 5. Prebuilt repo | 2 releases (`ghostty-a887df42c5`, `ghostty-636ce3a46f`), 4 Linux targets, attested | Workflow pins zig 0.15.2 |
| 6. Fork patch `636ce3a46f` | 1 commit, `std_capacity.styles` 128→512 | Applies cleanly on upstream `8867c37c5`; upstream still 128; ghostty landed print/style/render speedups that may make it moot |

## Decision — one rule per class

**Rule 1 — Decision records and SPEC.md: re-adopt as the v2 starting text.**
All of `docs/ghostty-migration/*` is salvage-imported as-is with a `v2 status` banner on each file. Each re-opened ticket amends its own file in place and removes the banner on resolution. Nothing under a banner is locked.

**Rule 2 — Vendored crates: re-fork fresh, re-apply v1's contract on top.**
`crates/ghostty_vt` / `crates/ghostty_vt_sys` are re-created from libghostty-rs `de9fd9b0fa` (or whatever the re-opened vendoring ticket pins), then v1's `build.rs` prebuilt contract (four resolution paths, `[sha256]` trust root, `headers_sha256` guard), `ghostty_pin.toml`, `tools/gen_bindings.rs`, and the 55-line safe-crate delta are ported onto it. v1's crates are never rebased forward.

**Rule 3 — Seam code: salvage by file, not by commit.**
Files that are *new* on v1 are salvage-imported at the start of the phase that introduced them: `pty.rs`, `ghostty.rs`, `ghostty/{grid_search,hyperlinks,vi_mode}.rs`, `differential.rs`, `mappings/{paste,focus}.rs`, the `.ztrx` transcripts, `script/terminal-*`, `script/lib/terminal-smoke-reel.py`, `script/licenses/ghostty-vt-LICENSES`, `nix/ghostty-vt/package.nix`, and the three workflows. Their integration hunks in `terminal.rs`, `alacritty.rs`, `keys.rs`, `mouse.rs`, `colors.rs`, `Cargo.toml` are **re-derived against the v2 baseline**, using the v1 diff as reference only. Imported files are fixed up in place for the moved APIs (constructor, scrollback-lines, `Error` arms, kitty temp-file, `active_screen`) and for the new upstream seam capabilities (#54884 grid-shape reads, #52454 history/cursor reads, #62504 `used_lines`, #62076 escape chars).
Carried but flagged **must re-validate** before use: `ghostty_source_build.yml` and the prebuilt workflow (zig 0.16, CPU policy), `pty_integration.yml` (advisory Windows lane), `nix/ghostty-vt/package.nix` (new pin), and the `portable-pty` git pin (whether 0.9.x now carries the Windows `kill()` fix is the PTY ticket's question).

**Rule 4 — Divergence ledger and perf baseline: carry as unverified.**
Both files are salvage-imported. Every ledger entry gets a `v2 status` line: `unverified` until the v2 differential harness re-fires it and the responsible ticket re-adjudicates it to `verified` (or retires it); entries the reconnaissance already invalidated are marked `obsolete` now with the reason (P5-001). Perf tables are kept as history, marked host-native; no v2 gate reading may cite them until alacritty *and* ghostty are re-run under the v2 CPU policy.

**Rule 5 — Prebuilt repo: append.**
v2 releases are appended to `xipeng-jin/libghostty-vt-prebuilt` under the existing `ghostty-<commit10>` scheme. The workflow moves to zig 0.16 and adopts the CPU policy the artifact-pipeline ticket decides. The v1 releases stay.

**Rule 6 — Fork patch: v2's pin is upstream; re-measure before any fork.**
v2 pins upstream ghostty (`8867c37c5` or later as the build ticket decides), not a fork commit. The perf gate is re-run on that pin under the v2 CPU policy. Only if `colored_dump` / `sustained_scroll` fail does the executor re-apply `636ce3a46f` on a fork branch and re-pin; the patch applies cleanly today. Upstreaming the patch is a separate decision, not a prerequisite.

## Mechanics

- A salvage import is one commit per class, `git checkout migration/libghostty -- <paths>`, message naming the v1 source commit; no cherry-picks, no v1 history.
- Classes 1 and 4 are imported **now** (this decision's commit), because tickets amend them.
- Classes 2, 3, 5, 6 are execution: the executor applies them per phase after SPEC v2 is locked.

## Ordering

1. **Rule 6** (pin) — root: the crate re-fork and the release tag are both keyed on it.
2. **Rule 2** (vendoring) and **Rule 5** (prebuilt) — need the pin and zig 0.16; only then can the vendoring task start.
3. **Rules 1 and 4** — imported immediately; amended ticket by ticket in parallel with 1–2.
4. **Rule 3** — applied per phase by the executor, after the seam/phase-plan ticket re-locks the phases.

## Consequences

- v1's ~10k lines of new seam files are not rewritten; only their integration points are.
- Nothing on v2 claims v1's verification status: every ledger entry, perf number and decision record is visibly provisional until its ticket or harness says otherwise.
- The fork branch `zed/perf-style-capacity` stays available but is not on the critical path.
- `v1 execution tickets #41–#55` remain frozen records; this policy supersedes them rather than resolving them.
