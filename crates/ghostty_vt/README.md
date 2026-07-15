# ghostty_vt

Safe Rust API for [libghostty-vt](https://github.com/ghostty-org/ghostty),
the Ghostty terminal emulation library, vendored into the Zed monorepo from
[uzaaft/libghostty-rs](https://github.com/uzaaft/libghostty-rs) (crate
`libghostty-vt`) at commit `51bf4bf7327324d1f60899db69882ce5d308359e`
(2026-07-13), under the MIT license.

See `crates/ghostty_vt_sys/README.md` for provenance detail, the license
resolution, the native build contract, the ghostty commit pin, and the
bindings-regeneration workflow — the two crates are vendored, pinned, and
bumped together.
