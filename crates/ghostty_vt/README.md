# ghostty_vt

Safe Rust API for [libghostty-vt](https://github.com/ghostty-org/ghostty),
the Ghostty terminal emulation library, vendored into the Zed monorepo from
[uzaaft/libghostty-rs](https://github.com/uzaaft/libghostty-rs) (crate
`libghostty-vt`, v0.2.1) at commit `de9fd9b0fa4ab53faebd3d489f4c74fe0ec832ec`
(2026-08-18), under the MIT license, and reconciled against the ghostty
commit pinned in `crates/ghostty_vt_sys/ghostty_pin.toml`.

See `crates/ghostty_vt_sys/README.md` for provenance detail, the license
resolution, the native build contract, the ghostty commit pin, and the
bindings-regeneration workflow — the two crates are vendored, pinned, and
bumped together.

The `kitty-graphics` cargo feature is **off by default** and must stay off:
the pinned native library is built with `-Dvt-features=-kitty_graphics,…`,
so enabling it fails at link time, not compile time.
