# libghostty-vt build strategy

Status: **Decided** (this doc is the decision record)
Scope: how Zed's build produces the native `libghostty-vt` static library, and the exact build contract we commit to.
Related: the bindings-vendoring ticket (how `libghostty-rs`-derived code lands in-tree) is separate; this doc only constrains it.

Local sources cited below:

- `libghostty-rs` checkout: `/home/xjin/Projects/refs/libghostty-rs` (referred to as `libghostty-rs/`)
- `ghostty` checkout: `/home/xjin/Projects/refs/ghostty` at `f8041e849b` (2026-07-14), which contains the pinned commit `a887df42c5` (2026-07-11) in history (referred to as `ghostty/`)
- Zed worktree: this repo (referred to as `zed/`)

Claims are marked **[verified]** (read from source / measured on this machine) or **[recon]** (taken from prior reconnaissance or web, cited).

---

## 1. The question

Zed is migrating terminal emulation from `alacritty_terminal` to `libghostty-vt`. The Rust side (vendored bindings, in-tree sys crate) needs a native static library `libghostty-vt.a` that only ghostty's Zig build system can produce. Who builds it, when, and with what toolchain?

Options evaluated:

- **(a)** Zig-on-PATH + build-time git fetch of ghostty source (libghostty-rs's current default)
- **(b)** Vendored/pinned ghostty source consumed via `GHOSTTY_SOURCE_DIR` (hermetic source, no git fetch)
- **(c)** Prebuilt static libs per platform, checked in or fetched
- Hybrids of the above

Criteria: plain `cargo build` works for contributors; CI; offline/sandboxed (nix) builds; build time and caching; Linux-first with a credible macOS/Windows path.

## 2. TL;DR recommendation

**Hybrid (c)+(a): default to a prebuilt, Zed-published, sha256-pinned static archive fetched by `build.rs`; keep the zig-from-source path as a fully supported escape hatch behind env vars.**

- Plain `cargo build` on Linux downloads a ~15 MB `libghostty-vt.a` (ReleaseFast) from a Zed-controlled GitHub release, keyed by ghostty commit + target triple, and verifies it against a sha256 checked into the tree. No Zig, no git-clone-of-ghostty, no bindgen/libclang at build time (bindings are pre-generated and vendored).
- `GHOSTTY_VT_LIB_DIR` (new) points at a local archive and skips all network — this is the nix/distro/offline contract, modeled on Zed's existing `LK_CUSTOM_WEBRTC` pattern for libwebrtc (`zed/nix/build.nix:234`).
- `GHOSTTY_SOURCE_DIR` (kept from libghostty-rs) builds from a local ghostty checkout with Zig 0.15.x — the dev loop for anyone hacking on ghostty itself, and the path the artifact-publishing workflow uses.
- The ghostty commit pin, per-target artifact sha256s, and the vendored `bindings.rs` are updated together, in one file set, by one documented bump procedure; one Linux CI job builds from source at the pin so the escape hatch can never rot.

This matches two precedents Zed contributors already live with daily: `webrtc-sys` downloads a prebuilt static libwebrtc in its build script (`~/.cargo/git/checkouts/livekit-rust-sdks-*/d0e27be/webrtc-sys/build.rs:102` calls `webrtc_sys_build::download_webrtc()`) **[verified]**, and `crates/zed/build.rs:103–118` downloads a ConPTY nupkg on Windows **[verified]**. It keeps the migration-period tax at ~zero for the thousands of contributors who will never touch ghostty internals, while the measured ~49 s cold / ~36 s warm source build stays one env var away for those who do.

---

## 3. How the pieces build today

### 3.1 `libghostty-vt-sys/build.rs` — the contract as found

File: `libghostty-rs/crates/libghostty-vt-sys/build.rs` (398 lines). All line refs below are into that file. **[verified]**

**Pin:** `GHOSTTY_REPO = "https://github.com/ghostty-org/ghostty.git"`, `GHOSTTY_COMMIT = "a887df42c56f6de86c0fe6da9c4eeca37931e083"` (lines 6–7). The pinned commit dates to 2026-07-11 **[verified]** — libghostty-rs tracks near-HEAD ghostty, not a release tag.

**Flow (lines 64–100):**

1. `DOCS_RS` set → return immediately; the checked-in `src/bindings.rs` suffices for docs (lines 66–70). Note: bindings are pre-generated — bindgen only runs under the optional `bindgen-tool` feature (`libghostty-rs/crates/libghostty-vt-sys/Cargo.toml:20`), so a normal build needs no headers, libclang, or bindgen. **[verified]**
2. `GHOSTTY_SOURCE_DIR` set → always wins, even over pkg-config (lines 86–89).
3. `pkg-config` cargo feature + probe succeeds → use installed lib (lines 94–97, 226–275; probes `libghostty-vt` or `libghostty-vt-static`, lines 55–61).
4. Otherwise → fetch ghostty into `OUT_DIR` and build with zig ("vendored" default feature).

**Env vars honored:**

| Var | Effect | Lines |
|---|---|---|
| `DOCS_RS` | skip native build entirely | 68 |
| `GHOSTTY_SOURCE_DIR` | use this checkout instead of fetching; must contain `build.zig` (asserted) | 86, 110–121 |
| `GHOSTTY_ZIG_SYSTEM_DIR` | pass `--system <dir> --global-cache-dir $OUT_DIR/zig-global-cache` to zig: resolve Zig packages from a pre-populated store, no network | 146–162 |
| `LIBGHOSTTY_VT_SYS_OPTIMIZE` | force zig optimize mode: `Debug`/`ReleaseSafe`/`ReleaseFast`/`ReleaseSmall`; anything else panics | 296–307 |
| `DEBUG`, `OPT_LEVEL` (cargo-set) | profile mapping: `DEBUG=true` → `Debug`; `OPT_LEVEL=s\|z` → `ReleaseSmall`; else `ReleaseFast` | 309–316 |
| `TARGET`, `HOST` | `-Dtarget=` passed only when cross-compiling (`target != host`) | 106–107, 166–169 |

**Cargo features:** `default = ["vendored"]`, `pkg-config`, `link-dynamic` (static is the default link mode), `bindgen-tool` (`Cargo.toml:15–20`); `links = "ghostty-vt"` so `cargo:include=` metadata (lines 277–285) reaches dependents as `DEP_GHOSTTY_VT_INCLUDE`. **[verified]**

**Git fetch (lines 319–360):** not a shallow clone — `git clone --filter=blob:none --no-checkout` (full history, lazy blobs) into `$OUT_DIR/ghostty-src`, then `git checkout <commit>`; a `.ghostty-commit` stamp file makes re-runs a no-op when the pin matches, and a pin change deletes and re-clones. Because this lives in `OUT_DIR`, each cargo profile keeps its own clone, and `cargo clean` deletes it.

**Zig invocation (lines 130–171):**

```
zig build -Demit-lib-vt=true -Doptimize=<mode> -Demit-xcframework=false \
    -Dapp-runtime=none --prefix $OUT_DIR/ghostty-install --cache-dir $OUT_DIR/zig-cache \
    [--system $GHOSTTY_ZIG_SYSTEM_DIR --global-cache-dir $OUT_DIR/zig-global-cache] \
    [-Dtarget=<mapped triple>]
```

Note the asymmetry: without `GHOSTTY_ZIG_SYSTEM_DIR`, no `--global-cache-dir` is passed, so zig's default user-global cache (`~/.cache/zig`) is used and survives `cargo clean`; the local `--cache-dir` under `OUT_DIR` does not.

**Target mapping (`zig_target()`, lines 380–397):** covers linux gnu/musl × x86_64/aarch64, macOS both arches, Windows gnu/gnullvm/msvc, Android; **any other Rust target panics** with `unsupported Rust target for vendored build: <triple>`.

**Artifact selection (lines 31–53, 172–211):** static mode looks for `libghostty-vt.a` (`ghostty-vt-static.lib` on Windows) in `<prefix>/lib` (plus `<prefix>/bin` on Windows), asserts `include/ghostty/vt.h` exists, emits `rustc-link-search` + `rustc-link-lib=static=ghostty-vt`. Dynamic mode (feature `link-dynamic`) links `dylib=ghostty-vt`.

**Rerun triggers (lines 74–81):** `rerun-if-env-changed` for `LIBGHOSTTY_VT_SYS_OPTIMIZE`, `GHOSTTY_SOURCE_DIR`, `GHOSTTY_ZIG_SYSTEM_DIR`, `TARGET`, `HOST`, `DEBUG`, `OPT_LEVEL`, plus `rerun-if-changed=crates/libghostty-vt-sys/build.rs`. That last path is relative to the *package* root (which already is `crates/libghostty-vt-sys`), so it points at a nonexistent file; cargo treats a missing `rerun-if-changed` path as always-changed, which appears to force the build script to re-run on every build (cheap — a warm zig no-op is 0.09 s, see §3.4 — but it re-executes git/zig checks each time). Fix this to `build.rs` in our vendored copy. **[verified path is wrong; the always-rerun consequence follows from cargo's documented behavior, not separately measured]**

**Failure modes as found (what the user actually sees):**

| Failure | Actual behavior |
|---|---|
| zig not on PATH | panic: `failed to execute zig build: No such file or directory (os error 2)` (lines 362–366) — no mention of zig being a requirement or how to get it |
| wrong zig version (e.g. 0.16.0) | ghostty's `requireZig` emits a Zig `@compileError`: `Your Zig version vX.Y.Z does not meet the required build version of v0.15.2` (`ghostty/src/build/zig.zig:5–18`), then cargo shows panic `zig build failed with status exit status: 1` |
| no network (default path) | `git clone` fails; panic `git clone ghostty failed with status exit status: 128` |
| `GHOSTTY_SOURCE_DIR` without `build.zig` | assert: `GHOSTTY_SOURCE_DIR does not contain build.zig: <path>` (lines 113–117) |
| `GHOSTTY_ZIG_SYSTEM_DIR` empty / nonexistent | asserts at lines 147–155 |
| unsupported target | panic: `unsupported Rust target for vendored build: <triple>` (line 394) |
| bad `LIBGHOSTTY_VT_SYS_OPTIMIZE` | panic listing the four accepted values (lines 303–306) |

### 3.2 Ghostty's zig build for `-Demit-lib-vt`

- **Zig version:** `minimum_zig_version = "0.15.2"` (`ghostty/build.zig.zon:6`), enforced by `requireZig` (`ghostty/build.zig:13–16`) which requires **the same major.minor** and `patch >= required` (`ghostty/src/build/zig.zig:9–12`). So Zig 0.16.0 (current stable, released 2026-04-13 per [ziglang.org/download](https://ziglang.org/download/)) **cannot** build the pinned commit; the toolchain must be 0.15.2 ≤ zig < 0.16. **[verified]**
- **What `-Demit-lib-vt` builds:** `src/lib_vt.zig` as a module (`ghostty/src/build/GhosttyZig.zig:104–145`) plus: unicode tables, `uucode` (URL dependency, always needed for grapheme break support), and — when `-Dsimd=true`, the default for non-wasm (`ghostty/src/build/Config.zig:196–206`) — vendored `simdutf` and `highway` (in-tree path deps `ghostty/pkg/simdutf`, `ghostty/pkg/highway`; `build.zig.zon:66–81`), built in no-libcxx mode so consumers don't need libc++ (`GhosttyZig.zig:121–129`, `GhosttyLibVt.zig:316–322`).
- **Fat archive:** for static builds, the lib plus all vendored SIMD archives are combined into a single `libghostty-vt.a` by `CombineArchivesStep` (`ghostty/src/build/GhosttyLibVt.zig:283–302`); `bundle_compiler_rt = true`, `bundle_ubsan_rt = true` (ubsan disabled for MSVC), and `pic = true` so the archive links into PIE executables (`GhosttyLibVt.zig:217–234`). Downstream link requirement is libc only; `-Dsimd=false` removes simdutf/highway at a perf cost **[recon for the perf claim; the wiring is verified]**.
- **Hermetic mode:** ghostty's official offline flow (`ghostty/PACKAGING.md:67–92`) is: populate a zig cache with `ZIG_GLOBAL_CACHE_DIR=<dir> ./nix/build-support/fetch-zig-cache.sh` (which runs `zig fetch <url>` for every line of `ghostty/build.zig.zon.txt`, because `zig build --fetch` misses transitive deps — ziglang/zig#20976, cited in the script) — then build with `--system <dir>/p`. Ghostty's own nix packages do exactly this via `build.zig.zon.nix` (`ghostty/nix/package.nix:58,87–88`; `ghostty/nix/libghostty-vt.nix:76–78`). **[verified]**
- **`--system` caveat, tested:** `--system` flips zig into system-package mode where `systemIntegrationOption` defaults to true (`/opt/zig/0.15.2/lib/std/Build.zig:2689`), which in principle could make the build expect system simdutf instead of bundling it. Empirically this does **not** happen for the lib-vt build: on this machine (no system simdutf; `pkg-config --exists simdutf` fails), `zig build --system <p-dir> -Demit-lib-vt=true` produced a fat archive with 201 simdutf and 80 highway symbols, byte-size-identical to the network build. Ghostty's own `libghostty-vt.nix` sanity-check asserts the same (`ghostty/nix/libghostty-vt.nix` `sanity-check` greps `simdutf`/`3hwy` symbols in the `.a`). **[verified empirically; mechanism not fully traced]**
- **libghostty-rs's own hermetic build** doesn't use `GHOSTTY_ZIG_SYSTEM_DIR` at all: its flake consumes ghostty's nix-built `libghostty-vt` package through the `pkg-config` cargo feature (`libghostty-rs/flake.nix:21–23,62,85,98`), with zig 0.15.2 from an overlay only in the devShell (`flake.nix:61,166–171`). Its GitHub CI is nix on linux/macOS (`.github/workflows/ci.yml:49–65`) and, on Windows, the (a)-style path with `mlugg/setup-zig@v2` pinned to 0.15.2 (`.github/workflows/windows-ci.yml:35–39`). **[verified]**

### 3.3 What ghostty publishes (and doesn't)

- No prebuilt libghostty/libghostty-vt artifacts: releases are app bundles (e.g. `Ghostty.dmg`) and source; the GitHub releases page shows a "tip" nightly and points to ghostty.org for tagged app releases ([github.com/ghostty-org/ghostty/releases](https://github.com/ghostty-org/ghostty/releases)). libghostty has no version tags or release artifacts of its own; libghostty-rs pins a raw commit. **[recon, spot-checked via web — asset list only partially loaded; consistent with the commit-pin design in build.rs]**
- The C API is explicitly pre-1.0/in-flux: build.rs's own comment — "libghostty is pre-1.0, so this crate intentionally does not promise compatibility with every installed C API revision" (`build.rs:92–93`). The engine is production-stable **[recon]**. This is the core argument for hard commit-pinning (§7).
- Zig ships pinned binary tarballs per platform (linux/macos x86_64+aarch64 ~50–55 MiB, windows zips ~90 MiB, all minisign-signed) and releases roughly every 6–12 months; 0.15.2 (2025-10-11) remains downloadable after 0.16.0 ([ziglang.org/download](https://ziglang.org/download/)). Pinning an exact zig tarball in CI is routine (`mlugg/setup-zig` with `version: 0.15.2`, as libghostty-rs does). **[verified via web]**

### 3.4 Measured reality (this machine: AMD Ryzen AI MAX+ 395, 32 threads, Fedora, zig 0.15.2 in `/opt/zig`, ghostty checkout at `f8041e849b`, i.e. 3 days past the pin)

| Measurement | Value |
|---|---|
| Cold `zig build -Demit-lib-vt=true -Doptimize=ReleaseFast -Dapp-runtime=none` incl. all network package fetches into empty caches | **49.1 s** wall, 605 MB peak RSS |
| Same build, warm package store via `--system` but cold compile cache | 36.1 s |
| No-op rebuild (all caches warm) | **0.09 s** |
| Switch to `-Doptimize=Debug` with warm caches | 6.0 s |
| `libghostty-vt.a` ReleaseFast / Debug | **15.3 MB / 41.2 MB** (`.so` is 7.8 MB) |
| Zig local cache / global cache after build | 232 MB / 424 MB (package store `p/`: 338 MB) |
| ghostty working tree (no `.git`) / `.git` / `git archive` tarball at pin | 101 MB / 76 MB / **38 MB** |
| Subset ghostty's `libghostty-vt.nix` fileset needs (`include+pkg+src+vendor+build.zig*`, `ghostty/nix/libghostty-vt.nix:26–38`) | ≈ 57 MB raw |

The cold-build network fetch pulled ~24 zig packages (uucode, libxev, vaxis, z2d, zf, wayland protocols, fonts, …) even for the lib-vt-only build — the lazy-dependency graph resolves wider than strictly lib-vt. **[verified: package store listing]**

## 4. Zed's current build-toolchain landscape

What precedent exists for "install X before cargo build" vs "cargo build just works": **[all verified]**

- **System packages, yes; pinned toolchains, no:** `zed/script/linux` installs distro packages (gcc, cmake, clang, lld, libssl, …) and rustup; `zed/docs/src/development/linux.md:12–21` tells contributors to run it before `cargo run`. No non-Rust *versioned* toolchain is required on PATH today.
- **Build-time downloads of prebuilt native code are already the norm:**
  - `webrtc-sys` (LiveKit) — a workspace dependency (`zed/Cargo.toml:868,945`) — downloads a prebuilt static libwebrtc from GitHub releases inside its build script (`webrtc-sys/build.rs:102`, `rerun-if-env-changed=LK_CUSTOM_WEBRTC` at line 29). Every contributor's first `cargo build` does this download today.
  - `zed/crates/zed/build.rs:103–118` downloads the ConPTY nupkg from microsoft/terminal releases on Windows.
  - CI setup downloads a pinned wasi-sdk tarball into `./target/wasi-sdk` (`zed/script/download-wasi-sdk`, wired as `steps::setup_linux` + `download_wasi_sdk` in `zed/tooling/xtask/src/tasks/workflows/steps.rs:324–334`).
- **The offline/nix escape hatch pattern exists:** `zed/nix/build.nix:234` sets `LK_CUSTOM_WEBRTC = pkgs.callPackage ./livekit-libwebrtc/package.nix {}` — nix builds libwebrtc from source hermetically and points the sys crate at it, bypassing the download. There is a `nix_build.yml` CI workflow, so whatever we do must have a no-network path.
- **Cross-compilation is load-bearing:** `zed/script/bundle-linux:60–92` cross-builds `remote_server` for `*-musl` on the same host, and `remote_server → project → terminal` (`zed/crates/remote_server/Cargo.toml:56`, `zed/crates/project/Cargo.toml:93`), so the terminal backend must build for x86_64/aarch64 linux gnu **and** musl from day one. (`zig_target()` covers all four; a prebuilt matrix must too.)
- **No git submodules** (`zed/.gitmodules` does not exist), and CLAUDE.md forbids silent-failure ergonomics — error messages must be actionable.

## 5. Options analysis

Scored against: (1) plain `cargo build` for contributors, (2) CI, (3) offline/nix, (4) build time & caching, (5) Linux-first → macOS/Windows path.

### (a) Zig-on-PATH + build-time git fetch (libghostty-rs default)

- (1) **Fails the "plain cargo build" bar.** Every contributor on every platform where the terminal crate builds must install Zig — and not "a zig", but 0.15.x specifically, while stable is 0.16 (§3.2). `script/linux` distro packages will drift to 0.16+; we'd be pinning a tarball install into `script/linux`, docs for three OSes, and every CI image. The failure mode for the unprepared is a panic mentioning neither zig nor a fix (§3.1). During a coexistence migration this tax lands on ~100% of contributors for the benefit of the ~1% touching ghostty.
- (2) CI: workable (`mlugg/setup-zig`), but adds setup to linux/mac/windows test jobs, bundling, nightly, and remote-server cross jobs; zig compile output is invisible to sccache.
- (3) Offline/nix: fails twice over — git clone at build time *and* zig package fetches. Nix would need ghostty's `build.zig.zon.nix` machinery imported into Zed's nix.
- (4) Time: 49 s cold is fine; but the clone + local zig cache live in `OUT_DIR`, so per-profile duplication and full refetch after `cargo clean`.
- (5) Cross/mac/win path is genuinely good — zig cross-compiles all our targets from one host.

### (b) Vendored/pinned ghostty source + `GHOSTTY_SOURCE_DIR`

- Removes the git fetch, keeps every other cost of (a): Zig 0.15.x still required by everyone, and the build is *still not offline* — `uucode` and ~2 dozen other zig packages are URL deps fetched at build time unless we also vendor a 338 MB package store or wire up `fetch-zig-cache.sh`. True hermeticity means vendoring source **and** shipping a `GHOSTTY_ZIG_SYSTEM_DIR` store.
- Repo cost: ≥ 57 MB raw (lib-vt fileset) to ~101 MB (full tree) added to the monorepo, re-churned on every pin bump; a git submodule avoids the bloat but Zed has none today and submodules break "clone && cargo build" in a different way.
- Verdict: strictly more repo weight than (a) for only a partial hermeticity win. Reasonable only as an input to the artifact pipeline, not as the contributor path.

### (c) Prebuilt static libs

- (1) **Passes plainly:** `cargo build` downloads one 15 MB file. No zig, no git, no headers (bindings are pre-generated, §3.1). Identical in kind to the libwebrtc download contributors already make.
- Checked-in vs fetched: checking 15 MB × 4 linux targets into git, re-written per bump, permanently bloats history — rejected. Fetched from a Zed-controlled GitHub release with an in-tree sha256 pin is the webrtc/wasi-sdk/ConPTY pattern and keeps the repo clean.
- (2) CI: nothing to install; download is negligible and cacheable.
- (3) Offline/nix: same story as webrtc today — an env var override pointing at a locally-built archive; nix builds it from source via ghostty's own `libghostty-vt.nix`-style derivation (ghostty maintains one upstream).
- (4) Time: ~0. Caching: re-download (15 MB) after `cargo clean` — acceptable.
- (5) Platform path: we control the artifact matrix; adding macOS/Windows targets is a workflow-matrix change plus new pins. Cross to musl handled by publishing musl artifacts (the publishing workflow itself uses zig's easy cross-compilation).
- Costs: someone must own an artifact-publishing workflow; a pin bump requires publishing before merging; supply-chain surface (mitigated: artifacts built by CI from the pinned public commit, sha256-pinned in-tree, only fetched from our own release URL); contributors debugging *into* ghostty internals get a ReleaseFast archive by default (escape hatch: source build).

### Hybrids

- **(c) default + (a/b) escape hatch** — recommended. The source path must stay in the build script (not a side script) so the artifact workflow, the nix derivation, and ghostty hackers all exercise the same code, and so a CI job can keep it green.
- **(a) default + (c) for CI only** — inverts the cost: contributors pay the zig tax so CI can be lazy. Rejected.
- **(b) as the escape hatch's source** — unnecessary weight; `GHOSTTY_SOURCE_DIR` pointing at any checkout, plus the pinned-commit git fetch, already covers it.

| | (a) zig+fetch | (b) vendored src | (c) prebuilt fetch | (c)+(a) hybrid |
|---|---|---|---|---|
| plain `cargo build` | ✗ (zig 0.15.x for all) | ✗ (same) | ✓ | ✓ |
| CI | ~ (setup everywhere) | ~ | ✓ | ✓ (+1 source job) |
| offline / nix | ✗✗ | ✗ (zig pkgs still fetch) | ~ (override var) | ✓ (`GHOSTTY_VT_LIB_DIR`) |
| build time / caching | ~ (49 s cold, OUT_DIR churn) | ~ | ✓ | ✓ |
| Linux-first → mac/win | ✓ | ✓ | ✓ (matrix we own) | ✓ |
| repo weight | ✓ | ✗ (57–101 MB churned) | ✓ | ✓ |
| ops burden | low | low | artifact pipeline | artifact pipeline |

## 6. Recommendation — the build contract we commit to

**Adopt the (c)+(a) hybrid.** The vendored in-tree sys crate (name TBD by the vendoring ticket; assume `crates/ghostty_vt_sys` here) ships a `build.rs` derived from libghostty-rs's, with the following exact contract.

### 6.1 Resolution order

1. `GHOSTTY_VT_LIB_DIR` set → link `<dir>/libghostty-vt.a` directly. No network, no zig, no git. (nix, distros, air-gapped builds, "I built it myself".)
2. `GHOSTTY_SOURCE_DIR` set → source build: invoke zig against that checkout (dev loop on ghostty; pin is *not* enforced, a `cargo:warning` notes the checkout is unpinned).
3. `GHOSTTY_VT_FROM_SOURCE=1` → source build at the pin: git-fetch ghostty at `GHOSTTY_COMMIT` into `OUT_DIR` (blobless clone + stamp, as today), then zig. (Escape hatch when prebuilt is missing for an exotic target; also what the publishing workflow runs.)
4. Default → **prebuilt fetch**: download `libghostty-vt-<GHOSTTY_COMMIT[0..10]>-<target-triple>.tar.gz` from `https://github.com/{prebuilt_repo}/releases/download/ghostty-<commit[0..10]>/…` (`prebuilt_repo` read from `ghostty_pin.toml`; scheme decided in [artifact-pipeline.md](artifact-pipeline.md)) into `$OUT_DIR/`, verify sha256 against the in-tree pin manifest, unpack, link.

We do **not** carry libghostty-rs's `pkg-config` feature (a system libghostty of arbitrary API revision contradicts the pin; build.rs itself documents the API as pre-1.0) or `link-dynamic` (static only). Drop both to shrink the matrix. `DOCS_RS` short-circuit is kept.

### 6.2 Env vars

| Var | Who sets it | Meaning |
|---|---|---|
| `GHOSTTY_VT_LIB_DIR` | nix, packagers, offline users | dir containing prebuilt `libghostty-vt.a`; highest priority; no toolchain requirements |
| `GHOSTTY_SOURCE_DIR` | ghostty developers | local ghostty checkout to build with zig; must contain `build.zig` |
| `GHOSTTY_VT_FROM_SOURCE` | CI source job, artifact workflow, exotic targets | `1` → git-fetch pinned commit + zig build |
| `GHOSTTY_ZIG_SYSTEM_DIR` | hermetic source builds | forwarded to `zig build --system <dir>`; only meaningful with a source path |
| `LIBGHOSTTY_VT_SYS_OPTIMIZE` | perf/debug investigation | zig optimize override, source builds only (same four values as today) |

All of the above get `cargo:rerun-if-env-changed`. Additionally `rerun-if-changed=build.rs` (fixing the broken relative path, §3.1) and `rerun-if-changed=ghostty_pin.toml`. `TARGET`/`HOST`/`DEBUG`/`OPT_LEVEL` rerun-triggers are kept as today.

Profile mapping: **all paths default to ReleaseFast regardless of cargo profile** — prebuilts are published ReleaseFast-only, and source builds drop libghostty-rs's `DEBUG=true → Debug` mapping because the spike measured zig-Debug cores degrading `vt_write` ~3000× with non-empty scrollback (spike-findings.md), so a plain dev build must never silently link a Debug core. `LIBGHOSTTY_VT_SYS_OPTIMIZE` remains the explicit override for source paths (debugging inside ghostty is exactly the case where you set `GHOSTTY_SOURCE_DIR` anyway).

### 6.3 Toolchain requirement and version check

- Prebuilt path (default): **no toolchain requirements beyond what Zed already needs.** `zig` and `git` are not consulted.
- Source paths: `zig` on PATH with version `>= 0.15.2, < 0.16` (ghostty's `requireZig` rule, §3.2). Before invoking the build, `build.rs` runs `zig version` and enforces this itself so the user gets our message, not a Zig `@compileError` buried in build output. `git` required only for path 3.
- `script/linux` does **not** grow a zig dependency. `docs/src/development/linux.md` gains a short "Working on the ghostty terminal backend" subsection documenting `GHOSTTY_SOURCE_DIR` + the zig 0.15.2 tarball / `mise`/`zigup` install. CI's single source-build job uses `mlugg/setup-zig@v2` with `version: 0.15.2` (the exact pattern in `libghostty-rs/.github/workflows/windows-ci.yml:35–39`).

### 6.4 Failure modes and their exact messages

Every failure panics the build script (cargo convention) with a message that names the fix:

| Condition | Message (prefix `ghostty-vt-sys:`) |
|---|---|
| prebuilt download failed (network/404) | `failed to download prebuilt libghostty-vt for <triple> from <url>: <cause>. If you are offline, set GHOSTTY_VT_LIB_DIR to a directory containing libghostty-vt.a, or build from source with GHOSTTY_VT_FROM_SOURCE=1 (requires zig 0.15.x). See docs/ghostty-migration/build-strategy.md.` |
| sha256 mismatch | `prebuilt libghostty-vt for <triple> failed checksum verification (expected <hash>, got <hash>). Refusing to link. Delete <path> and retry; if this persists, the release asset or the pin in ghostty_pin.toml is wrong.` |
| no prebuilt published for target | `no prebuilt libghostty-vt for target <triple> (available: <list from pin manifest>). Build from source with GHOSTTY_VT_FROM_SOURCE=1 (requires zig 0.15.x), or set GHOSTTY_VT_LIB_DIR.` |
| `GHOSTTY_VT_LIB_DIR` lacks the archive | `GHOSTTY_VT_LIB_DIR is set to <dir> but it does not contain libghostty-vt.a` |
| zig missing (source path) | `building libghostty-vt from source requires zig (>= 0.15.2, < 0.16) on PATH, but 'zig version' could not be run: <cause>. Install from https://ziglang.org/download/#release-0.15.2 or unset GHOSTTY_SOURCE_DIR/GHOSTTY_VT_FROM_SOURCE to use the prebuilt library.` |
| zig wrong version | `zig <found> is not compatible: ghostty at the pinned commit requires >= 0.15.2 and < 0.16 (its build enforces same-minor). Install 0.15.2 from https://ziglang.org/download/#release-0.15.2.` |
| git clone fails (source path 3) | `failed to fetch ghostty <commit> from <repo>: <cause>. If offline, use GHOSTTY_SOURCE_DIR with an existing checkout.` |
| `GHOSTTY_SOURCE_DIR` without `build.zig` | as today (§3.1) |
| zig build itself fails | `zig build failed (status <s>) building libghostty-vt from <source dir>; see output above` |
| unsupported target for source build | as today, plus the `zig_target()` table location |

### 6.5 Caching behavior

- Prebuilt: downloaded tarball + unpacked `.a` live in `OUT_DIR`, keyed by commit (stamp file, mirroring the `.ghostty-commit` pattern at `build.rs:321–359`). `cargo clean` costs one 15 MB re-download. No cross-profile sharing needed at this size.
- Source: unchanged from libghostty-rs — clone + zig local cache in `OUT_DIR` (per-profile, cleaned by `cargo clean`); zig *global* package cache in the user default (`~/.cache/zig`) so package fetches survive `cargo clean`, except in `GHOSTTY_ZIG_SYSTEM_DIR` mode where it's redirected under `OUT_DIR` (as today, lines 146–162).
- CI: the prebuilt download needs no special caching; the one source-build job caches `~/.cache/zig` keyed on the pin.

### 6.6 Platform rollout

- **Day one (Linux-first):** artifacts for `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl` — the musl pair because `remote_server` transitively includes `terminal` and is cross-built to musl in bundling (§4). The sys crate enters the tree as a Linux-only target-specific dependency (`[target.'cfg(target_os = "linux")'.dependencies]`) of the terminal crate, so macOS/Windows contributors and CI are untouched during the Linux gate.
- **macOS/Windows gates:** extend the artifact matrix (`aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc` — note the Windows static artifact is named `ghostty-vt-static.lib`, §3.1) and widen the cfg. The publishing workflow cross-compiles darwin/musl slices with zig from Linux where possible; msvc artifacts come from a Windows runner (the pattern libghostty-rs's Windows CI already proves out).

## 7. Commit-pinning policy

- **Where the pin lives:** one file, `crates/ghostty_vt_sys/ghostty_pin.toml` (exact crate path decided by the vendoring ticket), read by `build.rs` via `include_str!`:

  ```toml
  # Single source of truth for the ghostty native pin.
  commit = "a887df42c56f6de86c0fe6da9c4eeca37931e083"
  release = "ghostty-a887df42c5"   # tag in the prebuilt-artifacts repo
  prebuilt_repo = "xipeng-jin/libghostty-vt-prebuilt"   # owner/name; upstream handoff = repo transfer + this line

  [sha256]
  x86_64-unknown-linux-gnu = "…"
  aarch64-unknown-linux-gnu = "…"
  x86_64-unknown-linux-musl = "…"
  aarch64-unknown-linux-musl = "…"
  ```

  No pin anywhere else: the vendored `bindings.rs`, the prebuilt artifacts, and the source-build fallback all key off this file. (libghostty-rs keeps the pin as a `const` in build.rs, line 7; we move it to data so the bump diff is boring and greppable.)
- **What we pin to:** a raw ghostty commit, not a tag — libghostty has no versioned releases (§3.3) and a pre-1.0 C API, so *only* the exact commit that `bindings.rs` was generated from is known-compatible. Prebuilt artifacts are content-addressed by that commit and sha256-pinned, so a tampered or re-uploaded asset cannot link.
- **Bump procedure (gist):**
  1. Trigger the artifact workflow at the new ghostty commit; it builds all matrix targets from source (`GHOSTTY_VT_FROM_SOURCE` path at the new commit) and publishes a `ghostty-<commit>` release with a sha256 manifest.
  2. One Zed PR: update `ghostty_pin.toml` (commit + hashes), regenerate `bindings.rs` from the same commit's headers (`bindgen-tool`-style feature, per the vendoring ticket), fix any API breakage.
  3. CI on that PR proves both paths: every job links the new prebuilt; the dedicated source-build job (`GHOSTTY_VT_FROM_SOURCE=1`, zig 0.15.x) rebuilds at the pin, guaranteeing the escape hatch and the published artifact can't silently diverge or rot.
- **Relationship to vendored bindings:** bindings and native pin move atomically, in the same PR, always. A pin bump without regenerated bindings (or vice versa) must be un-mergeable; cheapest enforcement is the source-build CI job plus a header-hash recorded in `ghostty_pin.toml` that the bindings-regeneration step also stamps — details to the vendoring ticket.
- **Cadence:** bump deliberately (when we need an upstream fix/API), not on a schedule; pre-1.0 API churn means every bump is potentially a code change, and the pin file makes each one auditable.

## 8. Open questions → other tickets

1. **Artifact pipeline ownership** — **resolved**, decision record: [artifact-pipeline.md](artifact-pipeline.md) (dedicated prebuilt repo, append-only releases, single-runner direct-zig workflow, GitHub artifact attestation, `generate-licenses` static section).
2. **Vendoring ticket interface** — **resolved** by the vendoring ticket: `crates/ghostty_vt_sys` + `crates/ghostty_vt`, `bindgen-tool` feature kept (`gen-bindings` bin restamps `headers_sha256`).
3. **Nix derivation** — **resolved**, and the sketch here was walked back: nix consumes the prebuilt via a fixed-output `fetchurl` derivation wired as `GHOSTTY_VT_LIB_DIR`, not an in-tree source derivation ([artifact-pipeline.md §6](artifact-pipeline.md)).
4. **`-Dsimd=false` variant** — **resolved: no** ([artifact-pipeline.md §7](artifact-pipeline.md)).
5. **Windows specifics** (deferred to the Windows gate): msvc vs gnullvm target choice, `ghostty-vt-static.lib` naming, ubsan-rt exclusion behavior (§3.2) when linking with MSVC.
6. **Debug-symbols story** — **resolved: no Debug artifact**; `GHOSTTY_SOURCE_DIR` is the debugging story, and source builds now default to ReleaseFast (§6.2, [artifact-pipeline.md §7](artifact-pipeline.md)).
