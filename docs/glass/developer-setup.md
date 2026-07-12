# Developer setup: CEF on Linux

The integrated browser embeds Chromium via CEF, consumed through the
[`cef-rs`](https://github.com/tauri-apps/cef-rs) bindings pinned at tag
`cef-v150.0.0+150.0.10` (ADR-0001). CEF itself is a prebuilt binary
distribution; this page covers getting it onto a Linux development machine and
how the build and the running binary find it. Linux is the primary development
platform (migration plan §1); the macOS flow is shaped but not yet validated.

## Quickstart

```sh
script/download-cef
export CEF_PATH="$HOME/.local/share/cef"
export LD_LIBRARY_PATH="${LD_LIBRARY_PATH:+$LD_LIBRARY_PATH:}$CEF_PATH"
```

Prerequisites: `curl`, `jq`, `tar` with bzip2 support, and roughly 2.5 GB of
free disk (a ~400 MB cached archive plus the ~1.7 GB extracted distribution).

`script/download-cef`:

- resolves the pinned CEF distribution (version `150.0.10`, pinned down to the
  exact CDN respin) for the host platform (`linux64`, `linuxarm64`,
  `macosarm64`, `macosx64`) against the Spotify CDN's `index.json`,
- downloads the *minimal* distribution and verifies its sha1 against the index,
- extracts it into `$CEF_PATH` (default `~/.local/share/cef`) in the layout
  `cef-dll-sys` expects, and writes the `archive.json` manifest it validates,
- is idempotent: if the pinned version is already staged it exits immediately,
  and the downloaded archive is cached under `~/.cache/zed/cef/` and reused
  (after sha1 re-verification) when staging into a different `CEF_PATH`.

`CEF_DOWNLOAD_URL` overrides the CDN mirror (same variable `cef-rs` honors).

## How the build locates CEF

`cef-dll-sys`'s build script (see `sys/build.rs` at the pinned tag) resolves
the CEF distribution in this order:

1. `FLATPAK` set → use `/usr/lib` (Flatpak runtimes ship CEF there).
2. `CEF_PATH` set → use that directory. The build validates
   `$CEF_PATH/archive.json`: its `name` must be a
   `cef_binary_<version>+…` archive whose version is not newer than the CEF
   version baked into the crate's own version (the `+150.0.10` build-metadata
   suffix of the pinned tag). `script/download-cef` writes this manifest.
3. Neither set → the build script downloads and extracts the same minimal
   distribution by itself into that build's `OUT_DIR`. This works but
   re-downloads per target directory; setting `CEF_PATH` is the supported flow.

On Linux the build then emits `rustc-link-search=native=$CEF_PATH` and links
`libcef.so` dynamically — nothing else from the distribution is used at build
time. (On macOS and Windows the build additionally compiles the
`libcef_dll_wrapper` static library with CMake/Ninja from the distribution's
`CMakeLists.txt`, `include/`, and `libcef_dll/`, which is why the staged layout
keeps them.)

## How the runtime finds CEF files on Linux

Two pieces have to resolve at process start:

1. **`libcef.so`** — the binary is dynamically linked against it, so the
   loader must find it via rpath or `LD_LIBRARY_PATH`.
2. **CEF resources** — CEF resolves `icudtl.dat`, the `.pak` resource bundles,
   and the `locales/` directory relative to its *module directory*, the
   directory containing `libcef.so` (per the `resources_dir_path` /
   `locales_dir_path` documentation in the distribution's
   `include/internal/cef_types.h`: when unset, the files "must be located in
   the module directory").

That gives two supported layouts:

- **Development (default):** put `$CEF_PATH` on `LD_LIBRARY_PATH` as in the
  quickstart. `libcef.so` loads from `$CEF_PATH`, which makes `$CEF_PATH` the
  module directory — where `script/download-cef` already staged `icudtl.dat`,
  the `.pak` files, and `locales/`. No copying involved.
- **Self-contained:** `script/stage-cef-runtime [target-directory]` (default
  `target/debug`) copies the shared libraries, `icudtl.dat`,
  `v8_context_snapshot.bin`, the `.pak` files, `vk_swiftshader_icd.json`, and
  `locales/` next to the binary, mirroring Glass's Windows staging script.
  The binary must then find `libcef.so` there, e.g. via an `$ORIGIN` rpath or
  `LD_LIBRARY_PATH` pointing at the binary's directory. (An `$ORIGIN` rpath
  cannot be emitted by `crates/browser` — Cargo link flags from a library's
  build script do not propagate to dependent binaries — so today this layout
  is exercised with `LD_LIBRARY_PATH`; packaging work in M3 owns the rpath.)

CEF subprocesses on Linux are the main binary re-invoked with `--type=…`
arguments; they inherit the environment, so both layouts cover them. No
separate helper binary exists on Linux (that is a macOS packaging concern,
scheduled for M3).

## Testing without CEF

The real engine sits behind the browser crate's `cef` cargo feature, which is
off by default and enabled by `crates/zed` for the shipped binary. So

```sh
cargo test -p browser
```

builds and runs the crate's deterministic tests — the browser view driven
through the scripted stub tab backend (`crates/browser/src/stub_tab_backend.rs`)
— with no CEF distribution present at build or run time. This is the
invocation CI must use for the CEF-free configuration (ticket #8): it has to
be package-scoped. A workspace-wide test build includes `crates/zed`, whose
dependency on `browser` re-enables the `cef` feature through cargo feature
unification, so in that configuration the browser test binary links
`libcef.so` again (and the workspace build needs the CEF distribution anyway
just to compile `zed`).

The engine-side unit tests (input conversion, keycode mapping, pump
scheduling) compile only with the feature enabled and, because `libcef.so` is
dynamically linked, need the runtime on the loader path:

```sh
LD_LIBRARY_PATH="$CEF_PATH" cargo test -p browser --features cef
```

## Version pinning and upgrades

The CEF version is pinned in three places that must move together:

- `CEF_RESPIN` in `script/download-cef` (the full
  `150.0.10+g8042e43+chromium-150.0.7871.101` respin, so every machine gets
  identical bits even if the CDN respins the version),
- the `cef` dependency tag, `cef-v150.0.0+150.0.10` (added to the workspace
  `Cargo.toml` together with `crates/browser` — migration plan §7, M1 step 2),
  and
- the spoofed user-agent's Chrome version in
  `crates/browser/src/cef_instance.rs`, which must match the respin's
  Chromium version.

`cef-dll-sys` fails the build if `archive.json` names a CEF distribution newer
than its own pin, so a mismatched bump is caught immediately. After changing
the pin, re-run `script/download-cef`; it replaces the staged distribution
when the version differs. Because the pinned Chromium accrues security
exposure (migration plan R6), upgrades are recurring deliberate tasks — the
first (145.0.28 → 150.0.10) landed immediately after M2 (ticket #23).

## Known limitations

- **No H.264/AAC.** Stock CDN builds of CEF omit proprietary codecs, so some
  video (and most DRM streaming, given the codec dependency) will not play.
  Building CEF from source with codecs enabled is documented in Glass but out
  of scope here (migration plan §6.4).
- The distribution ships a `chrome-sandbox` SUID helper; the browser currently
  runs with the sandbox disabled (`no_sandbox=1`, migration plan R2), so the
  staging scripts do not install it.
