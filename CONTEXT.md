# Glossary — libghostty-vt migration (v2)

Vocabulary shared by the migration map (`xipeng-jin/zed#27`) and the decision records under `docs/ghostty-migration/`. Terms only; decisions live in the records.

- **v1** — the first migration attempt, frozen on branch `migration/libghostty` (tip `e537270dac`). Reference material, never a merge base.
- **v2** — the re-validated migration on `migration/libghostty2`, started from `main` @ `38c5dd7c98`.
- **Baseline** — the upstream Zed / libghostty-rs / ghostty commits a v2 decision is validated against.
- **Salvage** — carrying a v1 artifact into v2. Governed by `docs/ghostty-migration/salvage-policy.md`.
- **Salvage import** — a commit that copies v1 files into v2 *by path*, with no v1 history, whose message names the v1 source commit. The only way v1 code or prose enters v2.
- **Pin** — the single ghostty commit named in `ghostty_pin.toml` from which the vendored bindings and the prebuilt artifacts are produced. In v2 the pin is an upstream ghostty commit unless a decision record says otherwise.
- **Fork patch** — a Zed-owned commit on `xipeng-jin/ghostty` layered over the pin. v1 carried one (`636ce3a46f`); v2 starts with none.
- **Prebuilt** — a release of `libghostty-vt` static archives on `xipeng-jin/libghostty-vt-prebuilt`, tagged `ghostty-<commit10>`, append-only.
- **Divergence ledger** — the record of every observable behaviour difference between the alacritty and ghostty backends, one entry per difference with the reason it is accepted.
- **Unverified** — ledger/perf status: recorded under v1, not yet re-fired by the v2 differential harness or benchmark. Carries no weight in a v2 gate.
- **Obsolete** — ledger status: invalidated by the re-charting reconnaissance before any re-firing; kept for the record, never re-fired.
- **Verified** — ledger status: re-fired under v2 and re-adjudicated by the responsible ticket.
- **Re-charting reconnaissance** — the delta surveys (comments 4/6–6/6 on the map) that replaced the charting-time surveys; where they conflict, the deltas win.
- **Platform gate** — the per-platform acceptance list (macOS, Windows) that must be green before the Alacritty-removal phase may execute. Recorded in `docs/ghostty-migration/verification-strategy.md §9`.
- **Binding evidence** — evidence a platform gate accepts toward discharge. Each gate item names who can produce its binding evidence (the fork, or upstream at upstreaming time).
- **Advisory** — CI evidence that is diagnostic only and never binding, regardless of its result.
