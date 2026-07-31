# Codex app-server wire types: sourcing decision

Research date: 2026-07-31. Primary sources:

- Codex repo: `/home/xjin/Projects/refs/codex` @ `d06c7ac055920c7cb140c25ebda3f3db20197b45` (2026-07-29). All `codex-rs/...` paths below are relative to this checkout and rev unless a different rev is named.
- t3code reference: `/home/xjin/Projects/refs/t3code` @ `694f8d1c6eaaabafbf5c2861ae524174919ef625` (2026-07-29).
- Zed workspace: `/home/xjin/Projects/worktrees/zed/hearty-drum/zed` (branch `migration/native-agent-server`).

## TL;DR recommendation

**Option B: generate Rust types from the JSON Schema bundle checked into the codex repo, pinned to release tag `rust-v0.146.0` = `e363b08c9175ac1cbe5893615dd2cb9ddf95043b` (2026-07-28), committing the generated output into a new self-contained Zed crate.**

Decisive facts:

1. **Option A's dependency closure is disqualifying.** `codex-app-server-protocol` transitively pulls ~14 codex workspace crates plus `rmcp = "=3.0.0-beta.3"` (exact-pinned prerelease), `rama-* = "=0.3.0-alpha.4"` (exact-pinned alphas), `starlark`, `reqwest`, `tokio`, `ts-rs`, `icu_*`, and `tree-sitter 0.25` + `tree-sitter-bash` (Zed is on `tree-sitter 0.26.9`) — all for wire types only.
2. **The schema bundle is a first-class, test-enforced artifact.** `codex-rs/app-server-protocol/schema/json/codex_app_server_protocol.v2.schemas.json` (543 definitions, ~492 KB) is kept in lockstep with the Rust types by the test `json_schema_fixtures_match_generated` in `codex-rs/app-server-protocol/tests/schema_fixtures.rs`, so a pinned schema rev is guaranteed to match that rev's server behavior. t3code does exactly this (pinned ref + committed generated output).
3. **Protocol churn is high** (~95 commits touching `codex-rs/app-server-protocol/` in July 2026 alone), which makes Option A's "any upstream Rust/dep change breaks Zed's build" and Option C's "silent drift" both worse than Option B's "breakage surfaces at regeneration time, dep footprint fixed at zero".

Caveats: the minimal type closure Zed needs is ~135 definitions (not 543) — generate only that subtree; add client-side forward-compat fallbacks (upstream's `ThreadItem` enum is internally tagged with no catch-all, so new upstream item types would otherwise fail deserialization); carry Apache-2.0 attribution (header comment + upstream LICENSE copy) in the generated crate.

---

## 1. Option A footprint: git-depend on `codex-app-server-protocol`

### Direct dependencies

From `codex-rs/app-server-protocol/Cargo.toml` (versions resolved via `codex-rs/Cargo.toml` `[workspace.dependencies]`):

| Dep | Version | Notes |
|---|---|---|
| anyhow | 1 | |
| clap | 4 (features: derive) | CLI dep in a *protocol* crate |
| codex-experimental-api-macros | path | proc-macro: proc-macro2/quote/syn only |
| codex-extension-items | path (`ext/items`) | |
| codex-protocol | path | the heavy one, see closure below |
| codex-shell-command | path | pulls tree-sitter |
| codex-utils-absolute-path | path | |
| codex-utils-path-uri | path | |
| schemars | 0.8.22 | Zed's lock already has 0.9.0 **and** 1.0.4 → third copy |
| serde / serde_json | 1 (serde features: rc) | |
| serde_with | 3.17 | |
| strum_macros | 0.28.0 | Zed's lock already has 0.27.2 → second copy |
| thiserror | 2.0.17 | Zed workspace: 2.0.12, compatible |
| rmcp | **`=3.0.0-beta.3`** (exact pin; features base64, macros, schemars, server) | not in Zed's lock; prerelease exact pin |
| ts-rs | 11 | not in Zed's lock; TypeScript export machinery |
| inventory | 0.3.19 | Zed lock has 0.3.21, compatible |
| tracing | 0.1.44 | |
| uuid | 1 (serde, v7) | Zed workspace uuid already has v7 |

No `rust-version`/MSRV is declared in `codex-rs/Cargo.toml`; `edition = "2024"` (so effectively Rust ≥ 1.85). Zed is also `edition = "2024"` (`Cargo.toml` line 267), so no MSRV problem. `codex-app-server-protocol` has **no build.rs** and no unstable features; version is `0.0.0` via `version.workspace = true`.

### Workspace-crate closure

Git-depending on one workspace member makes cargo resolve its path deps from the same git checkout. The closure (all declared `{ path = "..." }` only in `codex-rs/Cargo.toml` lines 158–263 — **no `version =` on path deps, so the git dep resolves cleanly without crates.io fallback**):

- `codex-app-server-protocol` → `codex-protocol`, `codex-extension-items`, `codex-shell-command`, `codex-utils-absolute-path`, `codex-utils-path-uri`, `codex-experimental-api-macros`
- `codex-protocol` (`codex-rs/protocol/Cargo.toml`) additionally pulls: `codex-async-utils` (→ tokio rt-multi-thread), `codex-execpolicy` (→ **starlark**), `codex-network-proxy` (→ **rama-core/rama-http/rama-socks5 `=0.3.0-alpha.4`**, rustls-native-certs, `codex-utils-home-dir`, `codex-utils-rustls-provider`), `codex-utils-image` (→ `codex-utils-cache` → tokio), `codex-utils-string`; plus external: **reqwest 0.12**, **tokio**, chrono, chardetng, encoding_rs, globset, **icu_decimal/icu_locale_core/icu_provider**, quick-xml, sys-locale, wildmatch
- `codex-shell-command` (`codex-rs/shell-command/Cargo.toml`) pulls: **tree-sitter** (workspace pins 0.25.10) + **tree-sitter-bash 0.25**, which duplicates Zed's `tree-sitter = "0.26.9"` (Zed `Cargo.toml` line 838), plus libc, regex, url, which

Total: **~14 codex workspace crates** compiled into Zed, plus heavy externals Zed does not currently have at all (checked `Cargo.lock`: no `rmcp`, no `starlark`, no `ts-rs`, no `rama-core`, no `icu_decimal`) and duplicated majors it does have (schemars third copy, strum_macros second copy, tree-sitter second copy, reqwest — Zed's lock already carries 0.11.27 and 0.12.24).

### Precedent for git deps in Zed

Zed's root `Cargo.toml` has ~34 real `git =` pinned deps (e.g. `alacritty_terminal`, `dap-types`, `lsp-types`, `pet` — lines visible via `grep 'git = ' Cargo.toml`). So git-pinning per se is established practice; the objection to Option A is the closure, not the mechanism.

## 2. Option B mechanics: generate from checked-in JSON schemas

### The schema artifacts in the codex repo

- Bundle: `codex-rs/app-server-protocol/schema/json/codex_app_server_protocol.v2.schemas.json` — 504,253 bytes, draft-07 style document with **543 `definitions`** (90 `*Params`, 88 `*Response`, 72 `*Notification`, 293 supporting types). A v1 bundle (`codex_app_server_protocol.schemas.json`) and per-type files also exist: `schema/json/v2/` holds **238 individual `*.json`** files (one per Params/Response/Notification), plus `schema/typescript/` for TS.
- Producer: in-crate binaries `codex-rs/app-server-protocol/src/bin/export.rs` and `src/bin/write_schema_fixtures.rs`; user-facing command `codex app-server generate-json-schema --out DIR` documented in `codex-rs/app-server/README.md` ("Message Schema" section), which states each output "is guaranteed to match that version".
- Freshness guarantee: `codex-rs/app-server-protocol/tests/schema_fixtures.rs` contains `json_schema_fixtures_match_generated` and `typescript_schema_fixtures_match_generated`, which regenerate in-memory and diff against the checked-in tree. **The committed schema cannot drift from the Rust types at any given commit.** Commit log on `schema/` shows it is updated continuously alongside protocol changes (latest 2026-07-29, `9f23e97797`).

### How t3code consumes it

t3code @ `694f8d1c6eaaabafbf5c2861ae524174919ef625`:

- Generator script: `packages/effect-codex-app-server/scripts/generate.ts`. It pins `const UPSTREAM_REF = "678157acaa819d5510adfe359abb5d0392cfe461"` (a real codex commit, 2026-07-19) and downloads the per-type schema files from `api.github.com/repos/openai/codex/contents/codex-rs/app-server-protocol` at that ref. It uses `@effect/openapi-generator`'s `JsonSchemaGenerator` to emit Effect Schema TypeScript, with a small `ManualSchemas` table patching a handful of legacy/v1 types.
- Committed output: `packages/effect-codex-app-server/src/_generated/` — `schema.gen.ts` (42,860 lines), `meta.gen.ts` (790 lines: the method-name → params/response tables for `CLIENT_REQUEST_METHODS` etc.), `namespaces.gen.ts` (255 lines). Each file header records the upstream ref. Hand-written wrapper code on top is thin: `src/client.ts` (269 lines) + `src/protocol.ts` (423 lines).

So the t3code pattern is: **pin a rev, fetch schemas, generate, commit the output, keep the runtime dependency-free.** For Zed the analogue is a `node`-free Rust codegen (e.g. [typify] against the v2 bundle, or a small custom generator run offline) with output committed into a new crate; no build.rs needed.

### Coverage

The v2 bundle covers the full v2 API surface — string-method scan of `codex-rs/app-server-protocol/src/protocol/v2/*.rs` finds ~170 distinct wire methods/notifications including `initialize`, all `thread/*` (start/resume/fork/rollback/list/read/compact), `turn/*` (start/interrupt/steer/completed/aborted/diff/plan), all `item/*` notifications, approvals (`item/commandExecution/requestApproval`, `item/fileChange/requestApproval`, `item/permissions/requestApproval`), `model/list`, `skills/list`, `account/*` auth/rate-limit endpoints, `thread/tokenUsage/updated`, `thread/compacted` — plus much more Zed doesn't need (realtime audio, plugins/marketplace, apps, fs watch, process control, Windows sandbox, attestation).

## 3. Option C scope: hand-rolled minimal module

Method surface a Zed client needs (from `codex-rs/app-server/README.md` lifecycle sections and t3code's actual usage in `apps/server/src/provider/Layers/CodexAdapter.ts` / `CodexSessionRuntime.ts`):

- Handshake: `initialize` (req) + `initialized` (client notification)
- Thread: `thread/start`, `thread/resume`, `thread/rollback`, `thread/list`, `thread/read`, `thread/compact/start` (~6 reqs)
- Turn: `turn/start`, `turn/interrupt`, `turn/steer` (~3 reqs)
- Misc reqs: `model/list`, `skills/list`, `account/read`, `account/login/start`, `account/login/cancel`, `account/logout` (~6)
- Server → client requests (approvals): `item/commandExecution/requestApproval`, `item/fileChange/requestApproval`, `item/permissions/requestApproval` (~3)
- Server notifications: `thread/started`, `turn/started`, `turn/completed`, `turn/aborted`, `turn/diff/updated`, `turn/plan/updated`, `item/started`, `item/completed`, `item/agentMessage/delta`, `item/reasoning/*` (3 delta forms), `item/commandExecution/outputDelta`, `thread/tokenUsage/updated`, `thread/compacted`, `account/updated`, `account/rateLimits/updated`, `model/rerouted`, `account/login/completed` (~18)

That is ~36 methods/notifications. Computing the transitive `$ref` closure of the corresponding definitions in the v2 bundle gives **~135 type definitions** (14 Params + 13 Response + 17 Notification + ~91 supporting structs/enums) — i.e. the honest hand-roll is ~3.7x the method count, dominated by the `ThreadItem` item-type family and its nested payloads (commandExecution, fileChange, reasoning, agentMessage, mcpToolCall, webSearch, todoList/plan, collab tool calls). For calibration, upstream's full v2 module tree (`codex-rs/app-server-protocol/src/protocol/v2/`) is ~26k lines with 631 `pub struct`/`pub enum` declarations; a 135-type subset is plausibly 3–5k lines of Rust — writable, but every one of those lines is a transcription that can silently diverge (see §6).

## 4. Licensing

- Codex: **Apache-2.0** (`/home/xjin/Projects/refs/codex/LICENSE`), with a `NOTICE` file ("OpenAI Codex, Copyright 2025 OpenAI"; also notes Ratatui-derived MIT code, irrelevant to the protocol crate).
- t3code itself: MIT (`/home/xjin/Projects/refs/t3code/LICENSE`, T3 Tools Inc.) — relevant only if we copy its generator approach, which we are not copying verbatim.
- Zed: dual-tree licensing — `LICENSE-APACHE` and `LICENSE-GPL` at repo root; the crates this work touches declare `license = "GPL-3.0-or-later"` (`crates/agent_servers/Cargo.toml`, `crates/acp_thread/Cargo.toml`).

Compatibility: Apache-2.0 is one-way compatible with GPL-3.0 — all three options are fine.

- **A** (dependency): no vendoring; nothing to do beyond normal dependency licensing.
- **B** (committed generated output): generated output derived from Apache-2.0 schemas — include a header comment in each generated file naming the source repo + pinned rev, and carry a copy of the upstream LICENSE (and the relevant NOTICE line) in the new crate directory. Keep the NOTICE attribution per Apache-2.0 §4(d).
- **C** (hand transcription from Apache-2.0 sources): same attribution posture as B; hand-rolled-from-spec is arguably not a derivative of the code, but attributing costs nothing.

## 5. Concrete pinned rev

In `/home/xjin/Projects/refs/codex`:

- HEAD: `d06c7ac055920c7cb140c25ebda3f3db20197b45` (2026-07-29)
- Latest release tag: **`rust-v0.146.0` → `e363b08c9175ac1cbe5893615dd2cb9ddf95043b` (2026-07-28)**; alphas `rust-v0.146.0-alpha.1..16` precede it.
- Churn in the protocol crate: `git log --oneline -- codex-rs/app-server-protocol/` shows **306 commits since 2026-05-01** and **95 commits in July 2026 alone** (~3/day). Recent subjects are mostly additive (new fields/notifications: session titles, plugin metadata, thread sections, enterprise plans).

**Recommended pin: tag `rust-v0.146.0` (`e363b08c9175ac1cbe5893615dd2cb9ddf95043b`)** — a released, user-shipped protocol snapshot one day behind HEAD, and the natural rev to name in docs and the regeneration script. (t3code pinned an arbitrary mid-cycle commit `678157ac…`, 2026-07-19; a release tag is strictly better for auditability.)

## 6. Pin-advance breakage per option

- **A (git dep):** every pin advance re-resolves the whole closure. Breaks when: (i) any breaking Rust API change in `codex-app-server-protocol` or its ~14-crate closure; (ii) upstream bumps exact-pinned prerelease deps (`rmcp =3.0.0-beta.3` was bumped 2026-07-28 in commit `61de0d8fe8`; `rama =0.3.0-alpha.4`) — `codex-rs/Cargo.toml` itself had **93 commits since 2026-05-01 (21 in July)**, so dep churn is roughly weekly; (iii) a new duplicated-major conflict with Zed's tree appears. With ~3 protocol commits/day, holding a stale pin quickly diverges from the shipping CLI. Highest ongoing cost.
- **B (schema regen):** pin advance = re-run the generator and diff. Breaking type changes surface **at generation time as compile/diff errors**, additive changes are visible in review, and the external dependency footprint stays at zero regardless of what upstream does to its Cargo.toml. The only runtime risk is shared with C: new enum variants (see below).
- **C (hand-rolled):** nothing breaks at pin advance because there is no pin enforcement — types **silently drift**. Client-side deserialization: serde ignores unknown fields by default, so added fields are safe, but new variants of internally-tagged enums fail — `ThreadItem` (`codex-rs/app-server-protocol/src/protocol/v2/item.rs` line 224–227) is `#[serde(tag = "type")]` with **no catch-all variant**, so a new upstream item type turns every `item/started`/`item/completed` for that item into a runtime error. Server-side: codex does use `deny_unknown_fields` in places (`app-server-protocol/src/protocol/v2/permissions.rs`, `v2/mcp.rs`, `protocol/src/plan_tool.rs`, `protocol/src/request_permissions.rs`), so a hand-rolled client that grows extra/renamed fields in those params gets hard server rejections. Mitigation (applies to B too, and is a reason to own the types): add `#[serde(other)]`-style `Unknown` fallback variants on notification-side enums — something Option A cannot do because upstream owns the types.

## 7. Recommendation

**Option B**, pinned to `rust-v0.146.0` (`e363b08c9175ac1cbe5893615dd2cb9ddf95043b`):

- **Dependency footprint:** zero new external deps in Zed's tree (serde/serde_json only, already present). Option A adds ~14 codex crates + rmcp beta, rama alphas, starlark, ts-rs, icu, a third schemars, a second tree-sitter major — unacceptable for wire types, and directly contrary to the fork's standing decision to minimize upstream-rebase friction via self-contained crates (a fat git-dep closure is exactly the kind of Cargo.lock churn that makes rebases painful).
- **Correctness:** the schema is test-enforced to match the server at the pinned rev (`tests/schema_fixtures.rs`), giving B the same source-of-truth guarantee as A — which C lacks entirely.
- **Pin-advance:** breakage surfaces at regeneration/review time, not at build time (A) or runtime (C). Given ~3 protocol commits/day upstream, that containment matters.
- **Mechanics:** follow the t3code shape — a committed regeneration script that names the pinned rev, generated Rust committed under a new self-contained crate (e.g. `crates/codex_app_server_protocol/`), generated only for the ~135-definition closure Zed needs (§3), not all 543. Reasonable generators: `typify` on the v2 bundle, or a small purpose-built script; either way the output is reviewed and committed, no build.rs.
- **Licensing:** Apache-2.0 → GPL-3.0-or-later is fine; put the upstream LICENSE + NOTICE attribution and the pinned rev in the generated crate.

Caveats:

1. Generated code from JSON Schema will not perfectly reproduce upstream's serde nuances (internally-tagged enums, `serde_with` helpers); the schema is the wire truth, but spot-check round-trips against `schema/json/v2/*.json` examples and add fallback `Unknown` variants for forward compatibility.
2. Trimming to the minimal closure means advancing the pin can pull new referenced definitions into scope; the regen script must recompute the closure, not use a frozen name list.
3. If the closure-trimmed generation proves fiddly, the fallback is not Option A — it is Option C *seeded from* B's generated output (generate once, curate by hand, keep the schema diff as the drift detector), which preserves the zero-dep and pin-audit properties.
