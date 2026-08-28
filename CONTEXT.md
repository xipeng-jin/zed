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
- **Retired** — ledger status: the divergence no longer exists at the v2 pin; the entry's waiver never fired on the v2 harness and was deleted. Kept for the record.
- **Waiver** — the harness-side permit for one ledger entry: names the entry, pins the divergence's shape, and fails the run if it never fires. The only way a divergence passes the differential corpus.
- **Attribution A/B** — the perf-gate procedure that explains a failing scenario by re-running it with exactly one candidate change applied (v2: the v1 fork patch) rather than by instrumenting the core.
- **Fork-patch trigger** — the single stated condition under which v2 abandons the upstream pin for a fork commit. Recorded in `docs/ghostty-migration/verification-strategy.md §6.3`.
- **Chunking equivalence** — the harness property that a transcript yields the same seam state and PTY responses regardless of how its bytes are split into writes.
- **Harness probe** — a ghostty-side observation installed only in the differential harness, never in the product seam (e.g. the unknown-sequence callback). Triage or invariant, never a comparison field.
- **Re-charting reconnaissance** — the delta surveys (comments 4/6–6/6 on the map) that replaced the charting-time surveys; where they conflict, the deltas win.
- **Platform gate** — the per-platform acceptance list (macOS, Windows) that must be green before the Alacritty-removal phase may execute. Recorded in `docs/ghostty-migration/verification-strategy.md §9`.
- **Binding evidence** — evidence a platform gate accepts toward discharge. Each gate item names who can produce its binding evidence (the fork, or upstream at upstreaming time).
- **Advisory** — CI evidence that is diagnostic only and never binding, regardless of its result.
- **Logical line** — one or more grid rows joined across soft wraps up to a hard newline. The unit search and hover operate on: a match never crosses a hard newline and always sees a soft wrap as no boundary.
- **Cache-retrofit gate** — the measured foreground extraction cost above which the stateless search design is replaced by a snapshot-keyed extraction cache. Thresholds live in `docs/ghostty-migration/search-and-hyperlinks.md §1.2`.
