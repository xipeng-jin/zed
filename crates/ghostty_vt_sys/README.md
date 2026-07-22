# ghostty_vt_sys

Raw FFI bindings to [libghostty-vt](https://github.com/ghostty-org/ghostty),
the Ghostty terminal emulation library, vendored into the Zed monorepo. The
safe wrapper lives in `crates/ghostty_vt`.

## Provenance and license

Vendored from [uzaaft/libghostty-rs](https://github.com/uzaaft/libghostty-rs)
at commit `51bf4bf7327324d1f60899db69882ce5d308359e` (2026-07-13), crates
`libghostty-vt-sys` and `libghostty-vt`. This is a **one-time fork with full
Zed ownership**: we do not track upstream releases. Pulling an upstream change
is a manual act — diff upstream against the commit above and port what is
wanted. The primary maintenance loop is the ghostty pin bump (below), not
upstream sync.

Upstream's `LICENSE` file is MIT (Copyright 2026 Uzair Aftab, Leah Amelia
Chen) while its `Cargo.toml` declared `MIT OR Apache-2.0` with no Apache
license text present. The vendored copies resolve this to **MIT** — the
choice is valid under either reading of upstream's intent, and it matches the
only license text upstream actually ships. The upstream copyright notice is
preserved in `LICENSE` here and in `crates/ghostty_vt`.

Note: `cargo about` (script/generate-licenses) ignores private workspace
crates, so once Zed links these crates the MIT attribution for libghostty-rs
and ghostty must be added to the generated licenses explicitly. This is
tracked with the prebuilt-artifact pipeline work.

Deliberate divergences from upstream (beyond crate renames and Zed manifest
conventions): `build.rs` is rewritten to Zed's build contract (below);
`ghostty_pin.toml` replaces the in-code commit pin; the `pkg-config`,
`vendored`, and `link-dynamic` features are dropped (static-only, env-var
resolution); upstream's `clippy::pedantic`-class crate lints are dropped in
favor of Zed's workspace lint policy.

## Build contract

Decision record: `docs/ghostty-migration/build-strategy.md`. The native
`libghostty-vt.a` is resolved in this order:

1. **`GHOSTTY_VT_LIB_DIR`** — directory containing a prebuilt
   `libghostty-vt.a`; no network, no toolchain requirements. This is the
   nix/distro/offline contract.
2. **`GHOSTTY_SOURCE_DIR`** — local ghostty checkout, built with zig
   (>= 0.15.2, < 0.16 on PATH). The dev loop for hacking on ghostty itself;
   the checkout is not pin-enforced (a warning is emitted).
3. **`GHOSTTY_VT_FROM_SOURCE=1`** — git-fetch ghostty at the pinned commit
   into `OUT_DIR` and build with zig. Requires zig and network.
4. **Default** — fetch a Zed-published, sha256-pinned prebuilt archive.
   *Interim state*: no artifacts are published yet, so while the `[sha256]`
   table in `ghostty_pin.toml` is empty the default falls back to path 3 with
   a warning. The first published artifact removes this fallback.

Additional env vars: `GHOSTTY_ZIG_SYSTEM_DIR` (pre-populated zig package
store for hermetic source builds, forwarded as `zig build --system`),
`LIBGHOSTTY_VT_SYS_OPTIMIZE` (zig optimize-mode override:
`Debug`/`ReleaseSafe`/`ReleaseFast`/`ReleaseSmall`; source builds otherwise
follow the cargo profile).

## The pin

`ghostty_pin.toml` is the single source of truth for the native pin:

- `commit` — the exact ghostty commit the vendored `src/bindings.rs` was
  generated from. libghostty's C API is pre-1.0; only this commit is
  known-compatible.
- `source_repo` — the owner/name of the repository containing that commit.
- `headers_sha256` — digest of the installed C headers at that commit,
  stamped by the gen-bindings tool and verified by `build.rs` on pinned
  source builds, so a pin bump without regenerated bindings fails the build.
- `[sha256]` — per-target digests of the prebuilt archives (empty until the
  artifact pipeline publishes its first release).

## Regenerating bindings (pin bump)

1. Update `commit` in `ghostty_pin.toml`.
2. Build the headers at the new pin:
   `GHOSTTY_VT_FROM_SOURCE=1 cargo build -p ghostty_vt_sys`
   (this fails on the stale `headers_sha256` — expected; it proves the guard
   works. The headers are still installed under `OUT_DIR`.)
3. Regenerate `src/bindings.rs` and restamp `headers_sha256`:
   `cargo run -p ghostty_vt_sys --features bindgen-tool --bin gen-bindings`
4. Rebuild and reconcile any breakage in `crates/ghostty_vt`'s safe wrappers.
5. Land the pin bump, regenerated bindings, wrapper fixes, and (once the
   artifact pipeline exists) the new `[sha256]` entries in **one PR**.
