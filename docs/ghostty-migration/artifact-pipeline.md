# libghostty-vt prebuilt artifact pipeline

Status: **Decided** (this doc is the decision record)
Scope: where prebuilt `libghostty-vt` artifacts are built, hosted, and owned; the URL/tag scheme `build.rs` downloads from; supply-chain posture; nix wiring; artifact variants; and how the artifacts flow into Zed's license attribution.
Related: [build-strategy.md](build-strategy.md) defines the build contract this pipeline serves (resolution order, `ghostty_pin.toml`, bump procedure). This doc resolves that doc's open questions 1, 3, 4, and 6.

---

## 1. Hosting and ownership

Artifacts and the publishing workflow live in a **dedicated repository**: `xipeng-jin/libghostty-vt-prebuilt` today, `<zed-owner>/libghostty-vt-prebuilt` as the upstream shape.

- The workflow lives **in** that repo and publishes releases in its own repo with the default `GITHUB_TOKEN` (`contents: write`) — no cross-repo PAT or standing secret exists anywhere in the pipeline.
- Zed's release namespace stays clean: no `ghostty-*` tags or releases on the Zed repo, whose releases are app releases consumed by release automation.
- The repo coordinate is data, not code: `ghostty_pin.toml` carries `prebuilt_repo = "<owner>/libghostty-vt-prebuilt"`, and `build.rs` derives the download URL from it. Moving owners is a one-line pin edit.

Rejected: in-tree workflow publishing releases on the Zed repo (namespace pollution); workflow in the Zed tree publishing cross-repo (requires a standing cross-repo token).

## 2. Release tag, asset naming, URL scheme

- **Release tag:** `ghostty-<commit[0..10]>`, e.g. `ghostty-a887df42c5` — exactly the `release` key already stamped in `ghostty_pin.toml`. The full 40-char commit is recorded in the pin file and the release body. (This normalizes the 12-char asset naming sketched in build-strategy §6.1 to the 10-char form.)
- **Asset name:** `libghostty-vt-<commit10>-<target-triple>.tar.gz`, e.g. `libghostty-vt-a887df42c5-x86_64-unknown-linux-musl.tar.gz`.
- **Archive contents:** `libghostty-vt.a` plus ghostty's MIT `LICENSE`. No headers — bindings are pre-generated and `GHOSTTY_VT_LIB_DIR` only needs the archive.
- **Format is `.tar.gz`**, a deliberate walk-back from the `.tar.zst` sketch: the only consumer is `build.rs`, which fetches with a `curl -fL` shell-out (the ConPTY-download precedent, `crates/zed/build.rs`), verifies sha256 over the archive bytes in Rust via the existing `sha2` build-dep **before unpacking**, then unpacks with `tar -xzf` — which works unmodified with bsdtar on macOS and Windows 10+ for the later gates. Zero new build dependencies; zstd would save ~1–2 MB on a ~5 MB asset at the cost of a new dep or a system `zstd` requirement.
- **Download URL, fully derived:**
  `https://github.com/{prebuilt_repo}/releases/download/{release}/libghostty-vt-{commit10}-{triple}.tar.gz`
- Each release also carries a **`SHA256SUMS`** asset. It is *not* consumed by `build.rs` — the in-tree pin is the sole trust root — but it is what the bump procedure copies hashes from, and lets anyone verify assets independently.

## 3. Publishing workflow

- **Trigger:** `workflow_dispatch` only. Inputs: `ghostty_commit` (full 40-char, format-validated) and optional `targets` (defaults to the full day-one matrix). No scheduled or push triggers — pin bumps are deliberate, so publishing is too (build-strategy §7 step 1: publish precedes the pin-bump PR).
- **Runner:** one x86_64 `ubuntu-latest` job. Zig 0.15.2 via `mlugg/setup-zig`, checkout of Zed's `xipeng-jin/ghostty` source fork at exactly the input commit (the same repository recorded as `source_repo` in Zed's pin).
- **Build recipe: direct zig**, per target — the same invocation `build.rs`'s source path makes, recorded in the release body:

  ```
  zig build -Demit-lib-vt=true -Doptimize=ReleaseFast -Demit-xcframework=false \
      -Dapp-runtime=none -Dtarget=<zig triple>
  ```

  The workflow does **not** check out Zed or go through the sys crate — that would couple the prebuilt repo to monorepo branch state. Recipe drift between the workflow and `build.rs`'s source path is guarded by Zed's in-tree source-build CI job, which builds at the pin on every PR.
- **Day-one matrix** (cross-compiled by zig from the single runner): `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`. The macOS/Windows gates extend the matrix (build-strategy §6.6).
- **Per-target verification in the same job:**
  1. Symbol sanity via `nm`: assert `simdutf` and `hwy` symbols are present (the same grep ghostty's own `libghostty-vt.nix` sanity-check does) and that key `ghostty_vt_*` entry points exist.
  2. Link smoke test: compile a minimal C consumer against the `.a` with `zig cc -target <triple>`; execute it natively for x86_64 targets only. Link success is the real gate — native arm execution is not worth a second runner for a static-archive product.

Rejected: native runner matrix (`ubuntu-24.04-arm`) — buys only arm-native test execution.

## 4. Immutability and retention

**Append-only releases:**

- The workflow creates the `ghostty-<commit10>` release if absent, and may **add** assets for targets not yet present (the legitimate case: platform gates extending the matrix at the same pin).
- If an asset for a requested target already exists, the workflow **hard-fails** for that target. Assets are never deleted or re-uploaded — rebuilding the same commit does not produce byte-identical archives, so an overwrite would silently invalidate every stamped pin.
- `SHA256SUMS` is the one file that updates: regenerated to cover the union of assets, and the workflow asserts existing lines are unchanged.
- GitHub's immutable-releases repo setting stays **off** — it locks the whole release at creation, which conflicts with the append case; the discipline lives in the workflow.

**Retention: indefinite, delete-nothing.** Any release referenced by `ghostty_pin.toml` in any merged Zed commit must remain downloadable forever, or old checkouts and `git bisect` stop building. At ~20 MB per pin bump this is free.

## 5. Supply-chain posture

- **sha256 pins in `ghostty_pin.toml` remain the sole build-time gate.** `build.rs` verifies the archive hash and refuses to link on mismatch; every hash change is a reviewed PR diff. `build.rs` never verifies attestations — no `gh` or network-trust machinery at build time.
- **GitHub artifact attestation is ON**: the workflow runs `actions/attest-build-provenance` (permissions `id-token: write`, `attestations: write`) over every asset. This adds *origin* to the pin's *integrity*: `gh attestation verify <asset> -R <prebuilt_repo>` proves the asset was built by the CI workflow from the stated commit — a laptop-built artifact with a stamped hash is otherwise indistinguishable in a pin-bump diff.
- Verification is a **documented bump-procedure step** (run `gh attestation verify` when stamping hashes), not a hard CI gate on day one; it can be promoted later.
- **minisign rejected**: it reintroduces a standing private key to store and rotate — the ops burden the hosting decision deliberately avoided. Attestation is keyless.

## 6. Nix wiring

Nix consumes the **prebuilt**, not a source build:

- A small fixed-output derivation (`zed/nix/ghostty-vt/package.nix`, ~15 lines): `fetchurl` the release asset for the host platform with the sha256 mirrored from `ghostty_pin.toml`, unpack, expose the directory.
- Wired in `nix/build.nix` beside the `LK_CUSTOM_WEBRTC` line: `GHOSTTY_VT_LIB_DIR = pkgs.callPackage ./ghostty-vt/package.nix { };`.
- Nothing extra to touch on a pin bump beyond the derivation's mirrored hash/URL (same PR as the pin file).

This deliberately diverges from the ticket sketch (importing ghostty's `nix/libghostty-vt.nix` + `build.zig.zon.nix` package store) and from the `LK_CUSTOM_WEBRTC` source-build precedent. libwebrtc is built from source in nix because livekit's prebuilts fight nix's runtime model; our artifact has none of that pathology (static, bundled compiler-rt, PIC, libc-only). The fixed-output fetch means **nix links the byte-identical, attestation-covered artifact every other build links**, with no zig-in-nixpkgs pin and no zon-store churn. Packagers who require source builds are pointed at ghostty's upstream `libghostty-vt.nix` + `GHOSTTY_VT_LIB_DIR` in the docs.

Known gap, accepted: `nix build` on the migration branch stays broken until the first release is published (same window as the default cargo path).

## 7. Variants — both rejected

- **No `-Dsimd=false` artifact.** All four day-one targets build the fat SIMD archive (musl verified in the vendoring ticket); a target exotic enough to break simdutf/highway is what `GHOSTTY_VT_FROM_SOURCE` / `GHOSTTY_VT_LIB_DIR` exist for. A variant would force a dimension into the pin schema, URL scheme, and `build.rs` for a consumer that doesn't exist. The scheme leaves room to add one later (`libghostty-vt-nosimd-…`) without breaking anything.
- **No Debug archive.** The spike showed zig Debug cores degrade `vt_write` ~3000× with non-empty scrollback — a published Debug artifact is an attractive nuisance. Debugging into ghostty requires a source checkout anyway: `GHOSTTY_SOURCE_DIR` + `LIBGHOSTTY_VT_SYS_OPTIMIZE=Debug`, deliberately and locally.
- **Recorded amendment to the build contract** (same spike finding): source-path builds default to **ReleaseFast regardless of Cargo profile** — the libghostty-rs `DEBUG=true → Debug` profile mapping is dropped; `LIBGHOSTTY_VT_SYS_OPTIMIZE` remains the explicit override. Prebuilts were already ReleaseFast-only.

## 8. License attribution flow

- Every archive ships ghostty's MIT `LICENSE` (§2).
- `script/generate-licenses` gains a static section — a checked-in `script/licenses/ghostty-vt-LICENSES` file `cat`-ed into `licenses.md` as `# ###### GHOSTTY TERMINAL LICENSES ######`, the same mechanism as the themes/icons sections (cargo-about cannot see private workspace crates or native code). It must cover everything statically linked into the shipped binary:
  - ghostty (MIT, Mitchell Hashimoto and contributors),
  - the vendored bindings crates `ghostty_vt` / `ghostty_vt_sys` (MIT, Uzair Aftab and Leah Amelia Chen),
  - the components the fat archive bundles: simdutf, highway, uucode, and zig's compiler-rt/ubsan-rt — exact license texts pulled from the pinned ghostty tree at implementation time.

## 9. Fork → upstream handoff

When the pipeline graduates to the upstream owner, the prebuilt repo is **transferred**, never republished: a GitHub repo transfer preserves releases and assets byte-identically and redirects old download URLs, so every stamped sha256 stays valid and historical checkouts keep building through the redirect. `prebuilt_repo` in the pin file is updated in an ordinary PR afterwards. Republishing would re-tar the assets and invalidate every historical hash.

## 10. Hand-off to implementation

The migration spec's implementation phase for this pipeline, in order:

1. Create the prebuilt repo; add the publishing workflow (§3–§5).
2. Dispatch it at the current pin (`a887df42c5…`); verify attestations; stamp the four hashes and `prebuilt_repo` into `ghostty_pin.toml`.
3. Implement `fetch_prebuilt` in `crates/ghostty_vt_sys/build.rs` (curl + sha2 + tar per §2); delete the interim empty-table source fallback; switch source-path default optimize to ReleaseFast (§7).
4. Add the nix fixed-output derivation and the `GHOSTTY_VT_LIB_DIR` line in `nix/build.nix` (§6).
5. Add `script/licenses/ghostty-vt-LICENSES` and the `generate-licenses` section (§8).
