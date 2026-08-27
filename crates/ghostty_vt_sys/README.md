# ghostty_vt_sys

Raw FFI bindings to [libghostty-vt](https://github.com/ghostty-org/ghostty),
the Ghostty terminal emulation library, vendored into the Zed monorepo. The
safe wrapper lives in `crates/ghostty_vt`.

## Provenance and license

Re-forked (v2 of the migration, 2026-08-27) from
[uzaaft/libghostty-rs](https://github.com/uzaaft/libghostty-rs) at commit
`de9fd9b0fa4ab53faebd3d489f4c74fe0ec832ec` (v0.2.1, 2026-08-18), crates
`libghostty-vt-sys` and `libghostty-vt`. This is a **one-time fork with full
Zed ownership**: we do not track upstream releases, and the crates are never
rebased onto a newer libghostty-rs (salvage policy rule 2 in
`docs/ghostty-migration/salvage-policy.md`). The primary maintenance loop is
the ghostty pin bump (below), not upstream sync.

Upstream's `LICENSE` file is MIT (Copyright 2026 Uzair Aftab, Leah Amelia
Chen) while its `Cargo.toml` declares `MIT OR Apache-2.0` with no Apache
license text present. The vendored copies resolve this to **MIT** — the
choice is valid under either reading of upstream's intent, and it matches the
only license text upstream actually ships. The upstream copyright notice is
preserved in `LICENSE` here and in `crates/ghostty_vt`.

Note: `cargo about` (script/generate-licenses) ignores private workspace
crates, so the MIT attribution for libghostty-rs and ghostty is added to the
generated licenses through the static `script/licenses/ghostty-vt-LICENSES`
section owned by the artifact pipeline (`docs/ghostty-migration/artifact-pipeline.md` §8).

Deliberate divergences from upstream (beyond crate renames and Zed manifest
conventions): `build.rs` is rewritten to Zed's build contract (below);
`ghostty_pin.toml` replaces the in-code commit pin; the `pkg-config`,
`vendored`, and `link-dynamic` features and the iOS xcframework path are
dropped (static-only, env-var resolution); upstream's `clippy::pedantic`-class
crate lints are dropped in favor of Zed's workspace lint policy; the safe
crate's `kitty-graphics` feature is off by default.

## Build contract

Decision record: `docs/ghostty-migration/build-strategy.md` §6. The native
`libghostty-vt.a` is resolved in this order:

1. **`GHOSTTY_VT_LIB_DIR`** — directory containing a prebuilt
   `libghostty-vt.a`; no network, no toolchain requirements. This is the
   nix/distro/offline contract.
2. **`GHOSTTY_SOURCE_DIR`** — local ghostty checkout, built with zig. The
   dev loop for hacking on ghostty itself; the checkout is not pin-enforced
   (a warning is emitted, and a header-digest mismatch only warns).
3. **`GHOSTTY_VT_FROM_SOURCE=1`** — git-fetch ghostty at the pinned commit
   into `OUT_DIR` (blobless clone) and build with zig. Requires zig, git and
   network. The header digest is **enforced** on this path.
4. **Default** — fetch the Zed-published, sha256-pinned prebuilt archive
   from `https://github.com/{prebuilt_repo}/releases/download/{release}/`.
   *Interim state*: until the artifact pipeline publishes `release`, the
   `[sha256]` table in `ghostty_pin.toml` is empty and the default falls back
   to path 3 with a warning. The first stamped hash makes that fallback
   unreachable.

Every source build (paths 2 and 3) runs

```
zig build -Demit-lib-vt=true -Doptimize=<ReleaseFast> -Dcpu=<cpu> \
    -Demit-xcframework=false -Dapp-runtime=none -Dvt-features=<vt_features> \
    [-Dtarget=<zig triple>]
```

with `cpu` and `vt_features` taken from `ghostty_pin.toml`, so a source build
can never link a different feature set than the prebuilt it replaces. Source
builds are **ReleaseFast regardless of cargo profile** (a zig Debug core
degrades `vt_write` ~3000× with non-empty scrollback).

Zig is required only on source paths: `build.rs` runs `zig version` and
enforces ghostty's own `requireZig` rule from the `zig` field of the pin file
(same major.minor, patch ≥) — at the current pin that is `>= 0.16.0, < 0.17`
— with an actionable message instead of a Zig `@compileError`.

Additional env vars (source paths only): `GHOSTTY_ZIG_SYSTEM_DIR`
(pre-populated zig package store for hermetic builds, forwarded as
`zig build --system`), `LIBGHOSTTY_VT_SYS_OPTIMIZE`
(`Debug`/`ReleaseSafe`/`ReleaseFast`/`ReleaseSmall` override),
`LIBGHOSTTY_VT_SYS_CPU` (`-Dcpu` override, e.g. `native` or `x86_64_v3`, for
perf work on a known machine).

## The pin

`ghostty_pin.toml` is the single source of truth for the native pin:

- `commit` — the exact **upstream** ghostty commit the vendored
  `src/bindings.rs` was generated from (`8867c37c55b578b9eb4cfaba41cb9023e557176d`).
  libghostty's C API is pre-1.0; only this commit is known-compatible.
- `release` / `source_repo` / `prebuilt_repo` — the prebuilt release tag
  (`ghostty-<commit10>`), the repo containing `commit`, and the repo hosting
  the prebuilt releases.
- `zig` — ghostty's `minimum_zig_version` at `commit`; the version window
  `build.rs` enforces on source builds.
- `cpu`, `vt_features` — the `-Dcpu` / `-Dvt-features` used for the
  published prebuilts and every source build. The headers and `bindings.rs`
  still declare trimmed symbols (`ghostty_kitty_graphics_*`), so anything
  behind a trimmed feature must stay behind a default-off cargo feature —
  the failure mode is a link error.
- `headers_sha256` — digest of the installed C headers (`include/ghostty/**`)
  at that commit, stamped by `gen-bindings` and verified by `build.rs` on
  pinned source builds, so a pin bump without regenerated bindings fails the
  build.
- `[sha256]` — per-target digests of the prebuilt archives (empty until the
  artifact pipeline publishes `release`; see "Build contract" step 4).

Bindings at the pin: 190 `ghostty_*` functions declared; the trimmed
ReleaseFast/baseline archive exports 180 (0 `ghostty_kitty_graphics_*`,
12 `ghostty_snapshot_*`, 0 wuffs).

## Regenerating bindings (pin bump)

Requires zig (per the pin's window), git, and libclang (for bindgen).

1. Update `commit` (and `release`, `zig` if ghostty's
   `build.zig.zon` moved) in `ghostty_pin.toml`.
2. Build the headers at the new pin:
   `GHOSTTY_VT_FROM_SOURCE=1 cargo build -p ghostty_vt_sys`
   (this fails on the stale `headers_sha256` — expected; it proves the guard
   works. The headers are still installed under `OUT_DIR`.)
3. Regenerate `src/bindings.rs` and restamp `headers_sha256`:
   `cargo run -p ghostty_vt_sys --features bindgen-tool --bin gen-bindings`
   (`GHOSTTY_INCLUDE_DIR` or `GHOSTTY_SOURCE_DIR` can point it at headers
   explicitly). Note bindgen's enum underlying type follows the headers:
   ghostty ≥ `e4ec4f0f9` declares every enum `: int`, so enum modules are
   `c_int` and the safe crate's int-enums are `#[repr(i32)]`.
4. Rebuild and reconcile any breakage in `crates/ghostty_vt`'s safe wrappers,
   then `cargo test -p ghostty_vt -p ghostty_vt_sys` and
   `./script/clippy -p ghostty_vt -p ghostty_vt_sys`.
5. Publish the prebuilt release for the new commit
   (`docs/ghostty-migration/artifact-pipeline.md`), verify the attestations,
   stamp the `[sha256]` entries.
6. Land the pin bump, regenerated bindings, wrapper fixes, and hashes in
   **one PR**.

## Pulling libghostty-rs changes

There is no tracked fork. To pick up an upstream libghostty-rs change:

1. `git -C <libghostty-rs> diff de9fd9b0fa..<new> -- crates/libghostty-vt crates/libghostty-vt-sys/tools`
   (never `build.rs` or `src/bindings.rs`: ours are owned here).
2. Port the wanted hunks by hand onto `crates/ghostty_vt` / `tools/gen_bindings.rs`,
   translating crate names (`libghostty_vt[_sys]` → `ghostty_vt[_sys]`).
   Upstream tracks an older ghostty commit than our pin, so any hunk that
   touches the FFI surface must be checked against our `bindings.rs`, not
   theirs.
3. Update the "Provenance" commit reference above to the new upstream commit
   if the port is a wholesale sync.
