# libghostty-vt prebuilt artifact pipeline (v2)

Status: **Decided for v2** (2026-08-26, ticket [#39](https://github.com/xipeng-jin/zed/issues/39), map [#27](https://github.com/xipeng-jin/zed/issues/27)).
Supersedes: the v1 record of the same name on `migration/libghostty` (resolved 2026-07-15, executed as P0 `67d967c2a8`, last at `e537270dac`). v1 text is retained where re-confirmed; every amended point is marked **[v2 amended]**.
Scope: where prebuilt `libghostty-vt` artifacts are built, hosted, and owned; the URL/tag scheme `build.rs` downloads from; supply-chain posture; nix wiring; artifact variants; and how the artifacts flow into Zed's license attribution.
Related: [build-strategy.md](build-strategy.md) (v2) defines the build contract this pipeline serves and fixes the toolchain/CPU/feature inputs (§0 rows 2–5); [salvage-policy.md](salvage-policy.md) rules 5–6 fix that v2 appends to the same prebuilt repo and pins upstream.

Sources for the v2 pass: the live repo [`xipeng-jin/libghostty-vt-prebuilt`](https://github.com/xipeng-jin/libghostty-vt-prebuilt) (2 commits, `.github/workflows/publish.yml`, releases `ghostty-a887df42c5` and `ghostty-636ce3a46f`, attestation of the latter inspected with `gh attestation verify --format json`); ghostty upstream `main` @ `8867c37c5` (`src/build/GhosttyLibVt.zig`, `GhosttyZig.zig:137-152`, `LibsystemOverrideStep.zig`, `build.zig.zon`); libghostty-rs `de9fd9b0fa` `build.rs`. Claims are **[verified]** unless marked otherwise.

---

## 0. v2 re-validation summary (2026-08-26)

Point-by-point against the "Re-validation required" comment on #39.

| # | Question | v2 answer |
|---|---|---|
| 1 | Dedicated append-only repo + `ghostty-<commit10>` release contract, attestation, minisign, transfer-not-republish | **Re-confirmed** verbatim (§1, §2, §4, §5, §9). The repo exists with two append-only attested releases; salvage rule 5 appends v2 releases to it. |
| 2 | Zig 0.16 workflow bump | **Amended**: `mlugg/setup-zig` → `0.16.0`, **hardcoded** in the workflow (a toolchain bump is a reviewed commit in the prebuilt repo mirroring `zig = "0.16.0"` in `ghostty_pin.toml`), echoed into the release body and `provenance.json` (§3). |
| 3 | CPU target and `-Dvt-features` per artifact | **Amended, taking #28's values**: `-Dcpu=baseline` and `-Dvt-features=-kitty_graphics,-glyph_protocol`. Both are **workflow inputs** with those defaults, so a future change is a dispatch parameter — but the asset name stays **flat** (one canonical variant per commit; a variant change is a new pin commit). Recorded on the release body and `provenance.json`, enforced by the verification step (§3). |
| 4 | Provenance format for the source repo / fork pins | **Amended**: today's fork provenance is only a hardcoded `repository:` line in the workflow (`32e227e`); neither release body nor attestation names the repo, so the a887 (upstream) and 636ce (fork) releases are indistinguishable from their metadata. v2 makes `source_repo` a validated **workflow input** (allowlist `ghostty-org/ghostty`, `xipeng-jin/ghostty`; default upstream), stamps repo + commit into the release title/body, and publishes an **attested `provenance.json`** asset (§5). v2 keeps **no fork** (salvage rule 6 / build-strategy §0 row 5): the first v2 release is `ghostty-8867c37c5` from `ghostty-org/ghostty`. |
| 5 | Day-one target matrix | **Re-confirmed**: `x86_64`/`aarch64` × `linux-gnu`/`linux-musl`, cross-compiled from one x86_64 runner (§3). |
| 6 | Widening to macOS/Windows | **Deferred to the platform-gate ticket**, with constraints recorded now (§3.1): macOS assets must be built on a Darwin host; Windows is x86_64 MSVC only with link-only smoke. |
| 7 | Nix wiring | **Re-confirmed**: fixed-output `fetchurl` → `GHOSTTY_VT_LIB_DIR`, Linux-gated, hashes mirrored from the pin in the same PR; salvaged by file with `commit10`/hashes re-stamped (§6). |
| 8 | Licenses vs bundled deps at HEAD (wuffs) | **Re-confirmed set, guard added**: wuffs is wired into the vt module only when `kitty_graphics` is on (`GhosttyZig.zig:140-152`), so the trimmed artifact bundles ghostty, simdutf, highway, uucode, zig compiler-rt/ubsan-rt — the v1 set; the v1 archives also carry 0 wuffs symbols. The workflow now asserts **zero wuffs symbols** so the license file cannot silently go stale (§3, §8). Texts re-pulled from the `8867c37c5` tree at implementation. |
| 9 | Variants | **Re-confirmed both rejected** (§7); `-Dsimd=false` archive at HEAD is 15.09 MB (build-strategy §0), still no consumer. |

Execution ordering for v2: publish `ghostty-8867c37c5` (workflow bump lands first, §10) → stamp hashes into the v2 pin file (schema owned by the vendoring ticket #33) → nix derivation and license file re-stamped.

## 1. Hosting and ownership — re-confirmed

Artifacts and the publishing workflow live in a **dedicated repository**: `xipeng-jin/libghostty-vt-prebuilt` today, `<zed-owner>/libghostty-vt-prebuilt` as the upstream shape.

- The workflow lives **in** that repo and publishes releases in its own repo with the default `GITHUB_TOKEN` (`contents: write`) — no cross-repo PAT or standing secret exists anywhere in the pipeline.
- Zed's release namespace stays clean: no `ghostty-*` tags or releases on the Zed repo, whose releases are app releases consumed by release automation.
- The repo coordinate is data, not code: `ghostty_pin.toml` carries `prebuilt_repo = "<owner>/libghostty-vt-prebuilt"`, and `build.rs` derives the download URL from it. Moving owners is a one-line pin edit.
- v2 **appends** to the existing repo (salvage rule 5); the two v1 releases stay forever (§4) even though no v2 commit will pin them.

Rejected: in-tree workflow publishing releases on the Zed repo (namespace pollution); workflow in the Zed tree publishing cross-repo (requires a standing cross-repo token).

## 2. Release tag, asset naming, URL scheme — re-confirmed

- **Release tag:** `ghostty-<commit[0..10]>`, e.g. `ghostty-8867c37c55` for the v2 pin. The full 40-char commit is recorded in the pin file, the release body, and `provenance.json`.
- **Asset name:** `libghostty-vt-<commit10>-<target-triple>.tar.gz`, e.g. `libghostty-vt-8867c37c55-x86_64-unknown-linux-musl.tar.gz`.
- **Flat name — no variant dimension [v2 decision].** The CPU target and feature set are *not* encoded in the asset name. One canonical variant per commit; the pin's sha256 already disambiguates, and a different trim or CPU is a new pin commit, never a second asset under the same tag. If a real second consumer ever appears, a suffix (`libghostty-vt-<commit10>-<variant>-<triple>`) can be added without breaking existing names.
- **Archive contents:** `libghostty-vt.a` plus ghostty's MIT `LICENSE`. No headers — bindings are pre-generated and `GHOSTTY_VT_LIB_DIR` only needs the archive.
- **Format is `.tar.gz`**: `build.rs` fetches with a `curl -fL` shell-out (ConPTY-download precedent), verifies sha256 over the archive bytes in Rust via the existing `sha2` build-dep **before unpacking**, then unpacks with `tar -xzf` (bsdtar on macOS / Windows 10+ handles it for later gates). Zero new build dependencies.
- **Download URL, fully derived:** `https://github.com/{prebuilt_repo}/releases/download/{release}/libghostty-vt-{commit10}-{triple}.tar.gz`
- Each release also carries **`SHA256SUMS`** (bump convenience, independent verification; not consumed by `build.rs`) and, new in v2, **`provenance.json`** (§5).

## 3. Publishing workflow — amended

- **Trigger:** `workflow_dispatch` only. Inputs **[v2 amended]**:
  - `ghostty_commit` — full 40-char, format-validated (unchanged);
  - `source_repo` — `owner/name` the commit is checked out from; validated against the allowlist `ghostty-org/ghostty` (default) and `xipeng-jin/ghostty`; the checkout step uses the input, never a hardcoded repository;
  - `cpu` — default `baseline`, passed as `-Dcpu=<cpu>`;
  - `vt_features` — default `-kitty_graphics,-glyph_protocol`, passed as `-Dvt-features=<…>`;
  - `targets` — optional subset of the day-one matrix (unchanged).
  No scheduled or push triggers — pin bumps are deliberate, so publishing is too (build-strategy §7 step 1: publish precedes the pin-bump PR). The dispatcher copies `cpu`/`vt_features` from the v2 pin file, so the workflow and every source build use the same values by construction.
- **Runner:** one x86_64 `ubuntu-latest` job. **Zig 0.16.0** via `mlugg/setup-zig@v2`, **hardcoded** in the workflow (a zig bump is a reviewed commit here that mirrors the `zig = "0.16.0"` field in `ghostty_pin.toml`; the required-version rule is ghostty's `requireZig`, build-strategy §0 row 2). Checkout of `source_repo` at exactly `ghostty_commit`.
- **Build recipe: direct zig**, per target, recorded in the release body **[v2 amended]**:

  ```
  zig build -Demit-lib-vt=true -Doptimize=ReleaseFast -Demit-xcframework=false \
      -Dapp-runtime=none -Dtarget=<zig triple> -Dcpu=<cpu> -Dvt-features=<vt_features>
  ```

  The workflow does **not** check out Zed or go through the sys crate — that would couple the prebuilt repo to monorepo branch state. Recipe drift between the workflow and `build.rs`'s source path is guarded by Zed's in-tree source-build CI job (`ghostty_source_build.yml`), which builds at the pin with the same pin-file `cpu`/`vt_features`.
- **Day-one matrix** (cross-compiled by zig from the single runner): `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`.
- **Per-target verification in the same job [v2 amended — the feature decision becomes a published invariant]:**
  1. Symbol sanity via `nm`: `simdutf` and `hwy` symbols present (ghostty's own `libghostty-vt.nix` sanity grep); `ghostty_*` entry points present (`> 100` defined `T` symbols, `ghostty_key_encoder_new` in particular).
  2. Trim invariants: **zero** `ghostty_kitty_graphics_*` exports, **zero** wuffs symbols (license-file guard, §8), `ghostty_snapshot_*` exports present (snapshot stays on, build-strategy §0 row 4). These assertions are derived from the `vt_features` input, so a deliberate change of the default updates them in the same commit.
  3. Link smoke test: compile a minimal C consumer against the `.a` with `zig cc -target <triple>`; execute natively for x86_64 targets only; the executed smoke asserts via `ghostty_build_info` that the archive is ReleaseFast and reports the expected feature set. Link success is the real gate for aarch64.

Rejected: native runner matrix (`ubuntu-24.04-arm`) — buys only arm-native test execution; making `zig` a dispatch input — the toolchain is a property of the pinned ghostty commit, not of a publish run.

### 3.1 Widening to macOS/Windows — constraints recorded, decision deferred [v2]

The platform-gate ticket decides *when*; these facts bound *how*:

- **macOS assets must be built on a Darwin runner.** Upstream's `LibsystemOverrideStep` (`src/build/LibsystemOverrideStep.zig`) rebinds libc/libm symbols in the static archive from bundled compiler-rt to libSystem, but it needs Apple's `nmedit` and is a no-op on non-Darwin hosts ("functional, just slower"). A cross-built Apple archive would silently ship the slow variant, so the workflow must **refuse** Apple targets on a Linux runner and build them in a `macos-*` job. Universal vs per-arch archives and `-Demit-xcframework` are that ticket's call.
- **Windows: MSVC, x86_64 and aarch64** (superseded 2026-08-26 by the platform-gate ticket, [verification-strategy.md §9.2.2](verification-strategy.md#922-binding-on-the-fork-artifact-side-blocks-p10): the fork bundles `aarch64-pc-windows-msvc`, so it joins the matrix, artifact-only). Originally recorded as x86_64 only. Upstream now links `ntdll`+`kernel32` and disables the stack protector for static MSVC consumers (`GhosttyLibVt.zig:252-268`, commits `1fe1b2d23`/`84254a9d8`); the archive is named `ghostty-vt-static.lib`; libghostty-rs dropped `aarch64-pc-windows-msvc` as unsupported. Cross-compile from the Linux runner is fine (no host-only post-processing), but smoke is **link-only**, and the asset name keeps `.tar.gz` while the inner file name follows upstream's per-OS static name (`build.rs`'s `static_archive_name`).
- Matrix widening is the legitimate append case (§4): same tag gains new assets; no Linux asset is touched.

## 4. Immutability and retention — re-confirmed

**Append-only releases:**

- The workflow creates the `ghostty-<commit10>` release if absent, and may **add** assets for targets not yet present (platform gates extending the matrix at the same pin).
- If an asset for a requested target already exists, the workflow **hard-fails** for that target. Assets are never deleted or re-uploaded — rebuilding the same commit does not produce byte-identical archives, so an overwrite would silently invalidate every stamped pin.
- `SHA256SUMS` and `provenance.json` are the files that update: regenerated to cover the union of assets, and the workflow asserts existing entries are unchanged (a `provenance.json` re-upload must keep every previously recorded per-asset record byte-identical).
- GitHub's immutable-releases repo setting stays **off** — it locks the whole release at creation, which conflicts with the append case; the discipline lives in the workflow.

**Retention: indefinite, delete-nothing.** Any release referenced by `ghostty_pin.toml` in any merged Zed commit must remain downloadable forever. The two v1 releases stay as well.

## 5. Supply-chain posture — amended (provenance content)

- **sha256 pins in `ghostty_pin.toml` remain the sole build-time gate.** `build.rs` verifies the archive hash and refuses to link on mismatch; every hash change is a reviewed PR diff. `build.rs` never verifies attestations — no `gh` or network-trust machinery at build time.
- **GitHub artifact attestation is ON** (`actions/attest-build-provenance`, permissions `id-token: write`, `attestations: write`) over every asset. Verification is a **documented bump-procedure step** (`gh attestation verify <asset> -R <prebuilt_repo>` when stamping hashes), not a CI gate.
- **What the attestation actually proves — corrected [v2].** Inspecting the `ghostty-636ce3a46f` attestation: the SLSA predicate's `resolvedDependencies` names only the prebuilt repo's own workflow commit (`32e227e06e`) and `externalParameters` only the workflow path; the ghostty commit and source repository appear nowhere. It proves "built by this workflow on GitHub-hosted CI", and only indirectly (via the workflow file at that commit) which ghostty repo was checked out. The a887 (upstream) and 636ce (fork) releases carry identical bodies and equivalent attestations.
- **`provenance.json` [v2 amended]:** every publish run writes and uploads an attested `provenance.json` asset — one record per asset with `source_repo`, `ghostty_commit`, `zig`, `cpu`, `vt_features`, `target`, `asset`, `sha256`, and the workflow run URL — and the release title/body state `source_repo@commit` explicitly. Because the JSON is in the attestation's subject set, `gh attestation verify provenance.json` now proves the origin claim §5 always intended: *this* archive was built from *that* repo and commit with *those* flags. Fork pins, if a later decision ever re-instates one, are recorded the same way — no hardcoding.
- **minisign rejected** (unchanged): it reintroduces a standing private key; attestation is keyless.

## 6. Nix wiring — re-confirmed

Nix consumes the **prebuilt**, not a source build:

- Fixed-output derivation `nix/ghostty-vt/package.nix` (~15 lines): `fetchurl` the release asset for the host platform with the sha256 mirrored from `ghostty_pin.toml`, unpack, expose the directory. Salvaged by file from v1 (`67d967c2a8`), re-stamped with the v2 `commit10` and hashes.
- Wired in `nix/build.nix` beside the `LK_CUSTOM_WEBRTC` line, Linux-gated: `GHOSTTY_VT_LIB_DIR = pkgs.callPackage ./ghostty-vt/package.nix { };`.
- Nothing extra to touch on a pin bump beyond the derivation's mirrored hash/URL (same PR as the pin file).

This deliberately diverges from importing ghostty's `nix/libghostty-vt.nix` + `build.zig.zon.nix` store: our artifact is a static, PIC, libc-only archive with bundled compiler-rt, so nix links the byte-identical, attestation-covered artifact every other build links; no zig-in-nixpkgs pin, no zon-store churn. Source-insisting packagers use ghostty's upstream `libghostty-vt.nix` + `GHOSTTY_VT_LIB_DIR`. Accepted gap: `nix build` on the branch stays broken until `ghostty-8867c37c55` is published.

## 7. Variants — both rejected, re-confirmed

- **No `-Dsimd=false` artifact.** All four targets build the fat SIMD archive; exotic targets are what `GHOSTTY_VT_FROM_SOURCE` / `GHOSTTY_VT_LIB_DIR` exist for. Still no consumer at HEAD (archive 15.09 MB, build-strategy §0).
- **No Debug archive.** Zig Debug cores degrade `vt_write` ~3000× with non-empty scrollback (v1 spike). Debugging into ghostty is `GHOSTTY_SOURCE_DIR` + `LIBGHOSTTY_VT_SYS_OPTIMIZE=Debug`, deliberately and locally.
- Source-path builds default to **ReleaseFast regardless of Cargo profile** (v1 amendment, kept; `LIBGHOSTTY_VT_SYS_OPTIMIZE` remains the override).
- The `-Dvt-features` trim is **not** a variant: it is the single canonical artifact (§2, build-strategy §0 row 4).

## 8. License attribution flow — re-confirmed, guarded

- Every archive ships ghostty's MIT `LICENSE` (§2).
- `script/generate-licenses` gains the static section `script/licenses/ghostty-vt-LICENSES` `cat`-ed into `licenses.md` as `# ###### GHOSTTY TERMINAL LICENSES ######` (themes/icons mechanism; cargo-about cannot see private workspace crates or native code). Salvaged by file from v1, texts **re-pulled from the `8867c37c5` tree** and the pinned package store (`uucode-2826a37a…`). It covers everything statically linked into the shipped binary:
  - ghostty (MIT, Mitchell Hashimoto and contributors),
  - the vendored bindings crates `ghostty_vt` / `ghostty_vt_sys` (MIT, Uzair Aftab and Leah Amelia Chen),
  - simdutf (MIT election), highway (BSD-3-Clause election), uucode (MIT + Höhrmann UTF-8 decoder + Unicode License v3), zig compiler-rt/ubsan-rt (Zig MIT).
- **wuffs is not bundled [v2 verified]:** `GhosttyZig.zig:140-152` imports wuffs into the vt module only when `kitty_graphics` is enabled; the trimmed artifact has no wuffs code, and the v1 archives (kitty graphics predating wuffs) carry 0 wuffs symbols. The workflow's zero-wuffs assertion (§3) is the guard: if the trim default ever re-enables kitty graphics, publishing fails until this section gains the wuffs (Apache-2.0/MIT) text.

## 9. Fork → upstream handoff — re-confirmed

The prebuilt repo is **transferred**, never republished: a GitHub repo transfer preserves releases and assets byte-identically and redirects old download URLs, so every stamped sha256 stays valid. `prebuilt_repo` in the pin file is updated in an ordinary PR afterwards.

## 10. Hand-off to implementation (v2)

In the prebuilt repo (one reviewed commit before any v2 dispatch):

1. Workflow: zig `0.16.0`; inputs `source_repo` (allowlisted), `cpu`, `vt_features`; recipe gains `-Dcpu`/`-Dvt-features`; release title/body stamp `source_repo@commit`, zig, cpu, features; trim/wuffs/`ghostty_build_info` assertions; `provenance.json` written, append-checked, and attested alongside the archives; README updated (v2 doc link, provenance section, Apple/Windows constraints from §3.1).
2. Dispatch at `ghostty-org/ghostty@8867c37c55b578b9eb4cfaba41cb9023e557176d`, defaults for cpu/features; verify all four attestations plus `provenance.json`.

In Zed (`migration/libghostty2`, execution phase per the seam/phase-plan ticket):

3. Stamp `prebuilt_repo`, `release = "ghostty-8867c37c55"`, the four hashes, and `zig`/`cpu`/`vt_features` into the v2 `ghostty_pin.toml` (schema: vendoring ticket #33); `fetch_prebuilt` salvaged from v1 `build.rs`.
4. Re-stamp `nix/ghostty-vt/package.nix` (§6).
5. Re-pull `script/licenses/ghostty-vt-LICENSES` texts at the pin (§8).
