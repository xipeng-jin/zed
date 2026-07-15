#![allow(clippy::disallowed_methods, reason = "build scripts are exempt")]

//! Produces the native `libghostty-vt` static library for the vendored
//! bindings, per the build contract in docs/ghostty-migration/build-strategy.md.
//!
//! Resolution order:
//! 1. `GHOSTTY_VT_LIB_DIR` — link a prebuilt archive from that directory
//!    (no network, no toolchain requirements).
//! 2. `GHOSTTY_SOURCE_DIR` — build from a local ghostty checkout with zig
//!    (dev loop for hacking on ghostty; the checkout is not pin-enforced).
//! 3. `GHOSTTY_VT_FROM_SOURCE=1` — git-fetch ghostty at the pinned commit
//!    and build with zig.
//! 4. Default — fetch a Zed-published prebuilt archive, verified against the
//!    sha256 pins in `ghostty_pin.toml`. Until the artifact pipeline
//!    publishes the first release (empty `[sha256]` table), this falls back
//!    to path 3 with a warning.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

const PIN_FILE: &str = include_str!("ghostty_pin.toml");
const GHOSTTY_REPO: &str = "https://github.com/ghostty-org/ghostty.git";
const DOCS_URL: &str = "docs/ghostty-migration/build-strategy.md";
const ZIG_DOWNLOAD_URL: &str = "https://ziglang.org/download/#release-0.15.2";

struct Pin {
    commit: String,
    headers_sha256: String,
    prebuilt_sha256: BTreeMap<String, String>,
}

fn main() {
    // docs.rs has no toolchains; the checked-in src/bindings.rs is enough for
    // generating documentation.
    if env::var("DOCS_RS").is_ok() {
        return;
    }

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=ghostty_pin.toml");
    println!("cargo:rerun-if-env-changed=GHOSTTY_VT_LIB_DIR");
    println!("cargo:rerun-if-env-changed=GHOSTTY_SOURCE_DIR");
    println!("cargo:rerun-if-env-changed=GHOSTTY_VT_FROM_SOURCE");
    println!("cargo:rerun-if-env-changed=GHOSTTY_ZIG_SYSTEM_DIR");
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_SYS_OPTIMIZE");
    println!("cargo:rerun-if-env-changed=TARGET");
    println!("cargo:rerun-if-env-changed=HOST");
    println!("cargo:rerun-if-env-changed=DEBUG");
    println!("cargo:rerun-if-env-changed=OPT_LEVEL");

    let pin = parse_pin(PIN_FILE);
    let target = env::var("TARGET").expect("TARGET must be set");

    if let Ok(dir) = env::var("GHOSTTY_VT_LIB_DIR") {
        link_prebuilt_dir(Path::new(&dir), &target);
    } else if let Ok(dir) = env::var("GHOSTTY_SOURCE_DIR") {
        build_from_source(SourceCheckout::Local(PathBuf::from(dir)), &pin, &target);
    } else if env::var("GHOSTTY_VT_FROM_SOURCE").as_deref() == Ok("1") {
        build_from_source(SourceCheckout::PinnedFetch, &pin, &target);
    } else {
        fetch_prebuilt(&pin, &target);
    }
}

/// Parses `ghostty_pin.toml`. Deliberately strict, line-based parsing: the
/// file is owned by this crate and machine-stamped, so anything unrecognized
/// is an error rather than something to guess about.
fn parse_pin(contents: &str) -> Pin {
    let mut commit = None;
    let mut headers_sha256 = None;
    let mut prebuilt_sha256 = BTreeMap::new();
    let mut in_sha256_table = false;

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[sha256]" {
            in_sha256_table = true;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            panic!("ghostty-vt-sys: unrecognized line in ghostty_pin.toml: {line}");
        };
        let key = key.trim();
        let value = value
            .trim()
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .unwrap_or_else(|| {
                panic!("ghostty-vt-sys: value for '{key}' in ghostty_pin.toml must be quoted")
            })
            .to_string();
        if in_sha256_table {
            prebuilt_sha256.insert(key.to_string(), value);
        } else {
            match key {
                "commit" => commit = Some(value),
                "release" => {}
                "headers_sha256" => headers_sha256 = Some(value),
                other => {
                    panic!("ghostty-vt-sys: unrecognized key '{other}' in ghostty_pin.toml")
                }
            }
        }
    }

    Pin {
        commit: commit.expect("ghostty-vt-sys: ghostty_pin.toml is missing 'commit'"),
        headers_sha256: headers_sha256
            .expect("ghostty-vt-sys: ghostty_pin.toml is missing 'headers_sha256'"),
        prebuilt_sha256,
    }
}

fn static_archive_name(target: &str) -> &'static str {
    if target.contains("windows") {
        "ghostty-vt-static.lib"
    } else {
        "libghostty-vt.a"
    }
}

/// Path 1: `GHOSTTY_VT_LIB_DIR` — the nix/distro/offline contract.
fn link_prebuilt_dir(dir: &Path, target: &str) {
    let archive = static_archive_name(target);
    assert!(
        dir.join(archive).exists(),
        "ghostty-vt-sys: GHOSTTY_VT_LIB_DIR is set to {} but it does not contain {archive}",
        dir.display()
    );
    println!("cargo:rustc-link-search=native={}", dir.display());
    println!("cargo:rustc-link-lib=static=ghostty-vt");
}

/// Path 4 (default): fetch a Zed-published prebuilt archive. The artifact
/// pipeline that publishes these archives has not landed yet, so today this
/// path always explains how to build instead; the sha256 lookup is already
/// wired so publishing artifacts only requires stamping the pin manifest.
fn fetch_prebuilt(pin: &Pin, target: &str) {
    match pin.prebuilt_sha256.get(target) {
        None => {
            // Interim state until the artifact pipeline publishes its first
            // release: an entirely empty sha256 table means "no artifacts
            // exist yet", and falling back to a pinned source build keeps
            // plain `cargo build` green on this branch. The first stamped
            // hash makes this branch unreachable and the contract strict.
            if pin.prebuilt_sha256.is_empty() {
                println!(
                    "cargo:warning=no prebuilt libghostty-vt artifacts are published yet; \
                     falling back to building from source at the pinned ghostty commit \
                     (requires zig 0.15.x and network). This fallback disappears once the \
                     artifact pipeline lands. See {DOCS_URL}."
                );
                build_from_source(SourceCheckout::PinnedFetch, pin, target);
                return;
            }
            let available = pin
                .prebuilt_sha256
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            panic!(
                "ghostty-vt-sys: no prebuilt libghostty-vt for target {target} \
                 (available: {available}). Build from source with GHOSTTY_VT_FROM_SOURCE=1 \
                 (requires zig 0.15.x on PATH), or set GHOSTTY_VT_LIB_DIR to a directory \
                 containing {archive}. See {DOCS_URL}.",
                archive = static_archive_name(target),
            );
        }
        Some(_sha256) => {
            // The download-and-verify implementation lands with the artifact
            // pipeline (it owns the release URL scheme).
            panic!(
                "ghostty-vt-sys: ghostty_pin.toml pins a prebuilt archive for {target}, but \
                 the prebuilt fetch is not implemented yet (it lands with the artifact \
                 pipeline). Build from source with GHOSTTY_VT_FROM_SOURCE=1, or set \
                 GHOSTTY_VT_LIB_DIR. See {DOCS_URL}."
            );
        }
    }
}

enum SourceCheckout {
    /// `GHOSTTY_SOURCE_DIR`: an arbitrary local checkout, not pin-enforced.
    Local(PathBuf),
    /// `GHOSTTY_VT_FROM_SOURCE=1`: git-fetch the pinned commit.
    PinnedFetch,
}

/// Paths 2 and 3: build `libghostty-vt.a` from ghostty source with zig.
fn build_from_source(checkout: SourceCheckout, pin: &Pin, target: &str) {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR must be set"));
    let host = env::var("HOST").expect("HOST must be set");

    check_zig_version();

    let pin_enforced = matches!(checkout, SourceCheckout::PinnedFetch);
    let ghostty_dir = match checkout {
        SourceCheckout::Local(dir) => {
            assert!(
                dir.join("build.zig").exists(),
                "ghostty-vt-sys: GHOSTTY_SOURCE_DIR does not contain build.zig: {}",
                dir.display()
            );
            println!(
                "cargo:warning=building libghostty-vt from GHOSTTY_SOURCE_DIR={}; this checkout \
                 is not enforced to match the pinned commit {} in ghostty_pin.toml",
                dir.display(),
                pin.commit
            );
            dir
        }
        SourceCheckout::PinnedFetch => fetch_ghostty(&out_dir, &pin.commit),
    };

    let install_prefix = out_dir.join("ghostty-install");
    let zig_cache_dir = out_dir.join("zig-cache");
    let zig_global_cache_dir = out_dir.join("zig-global-cache");
    let optimize = zig_optimize_mode();

    let mut build = Command::new("zig");
    build
        .arg("build")
        .arg("-Demit-lib-vt=true")
        .arg(format!("-Doptimize={optimize}"))
        .arg("-Demit-xcframework=false")
        .arg("-Dapp-runtime=none")
        .arg("--prefix")
        .arg(&install_prefix)
        .arg("--cache-dir")
        .arg(&zig_cache_dir)
        .current_dir(&ghostty_dir);

    // Package managers can provide ghostty's zig package cache ahead of time
    // and have zig resolve packages from that immutable store instead of
    // fetching during this build script.
    if let Ok(dir) = env::var("GHOSTTY_ZIG_SYSTEM_DIR") {
        assert!(
            !dir.is_empty(),
            "ghostty-vt-sys: GHOSTTY_ZIG_SYSTEM_DIR must not be empty when set"
        );
        let zig_system_dir = PathBuf::from(dir);
        assert!(
            zig_system_dir.exists(),
            "ghostty-vt-sys: GHOSTTY_ZIG_SYSTEM_DIR does not exist: {}",
            zig_system_dir.display()
        );
        build
            .arg("--system")
            .arg(&zig_system_dir)
            .arg("--global-cache-dir")
            .arg(&zig_global_cache_dir);
    }

    // Only pass -Dtarget when cross-compiling; for native builds zig
    // auto-detects the host.
    if target != host {
        build.arg(format!("-Dtarget={}", zig_target(target)));
    }

    let status = build.status().unwrap_or_else(|error| {
        panic!("ghostty-vt-sys: failed to execute zig build: {error}");
    });
    assert!(
        status.success(),
        "ghostty-vt-sys: zig build failed (status {status}) building libghostty-vt from {}; \
         see output above",
        ghostty_dir.display()
    );

    let lib_dir = install_prefix.join("lib");
    let include_dir = install_prefix.join("include");
    let archive = static_archive_name(target);
    let mut search_dirs = vec![lib_dir];
    if target.contains("windows") {
        search_dirs.push(install_prefix.join("bin"));
    }
    assert!(
        search_dirs.iter().any(|dir| dir.join(archive).exists()),
        "ghostty-vt-sys: expected {archive} in one of {search_dirs:?} after zig build"
    );
    assert!(
        include_dir.join("ghostty").join("vt.h").exists(),
        "ghostty-vt-sys: expected header at {}",
        include_dir.join("ghostty").join("vt.h").display()
    );

    verify_headers(&include_dir, pin, pin_enforced);

    for dir in &search_dirs {
        println!("cargo:rustc-link-search=native={}", dir.display());
    }
    println!("cargo:rustc-link-lib=static=ghostty-vt");
    println!("cargo:include={}", include_dir.display());
}

/// The vendored src/bindings.rs is only valid for the exact headers it was
/// generated from. On pinned source builds a hash mismatch is fatal, which
/// makes a pin bump without regenerated bindings un-mergeable (the source
/// build CI job trips it); for GHOSTTY_SOURCE_DIR checkouts it only warns.
fn verify_headers(include_dir: &Path, pin: &Pin, pin_enforced: bool) {
    let actual = hash_headers(include_dir);
    if pin.headers_sha256 == actual {
        return;
    }
    if pin.headers_sha256.is_empty() {
        println!(
            "cargo:warning=ghostty_pin.toml has no headers_sha256 stamp; stamp it with \
             headers_sha256 = \"{actual}\" (or rerun gen-bindings, which stamps it)"
        );
    } else if pin_enforced {
        panic!(
            "ghostty-vt-sys: the C headers built at the pinned ghostty commit hash to \
             {actual}, but ghostty_pin.toml expects {expected}. The vendored \
             src/bindings.rs and ghostty_pin.toml must be updated together: regenerate \
             the bindings with `cargo run -p ghostty_vt_sys --features bindgen-tool --bin \
             gen-bindings` (which restamps headers_sha256). See {DOCS_URL}.",
            expected = pin.headers_sha256,
        );
    } else {
        println!(
            "cargo:warning=C headers in this GHOSTTY_SOURCE_DIR checkout hash to {actual}, \
             which differs from headers_sha256 in ghostty_pin.toml; the vendored bindings \
             may not match this checkout"
        );
    }
}

/// Deterministic digest of every file under `include/`: sorted relative
/// paths, each mixed in as `path\0contents\0`.
fn hash_headers(include_dir: &Path) -> String {
    let mut files = Vec::new();
    collect_files(include_dir, include_dir, &mut files);
    files.sort();
    let mut hasher = Sha256::new();
    for relative_path in &files {
        hasher.update(relative_path.as_bytes());
        hasher.update([0]);
        let contents = std::fs::read(include_dir.join(relative_path)).unwrap_or_else(|error| {
            panic!("ghostty-vt-sys: failed to read header {relative_path}: {error}")
        });
        hasher.update(&contents);
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())
}

fn collect_files(root: &Path, dir: &Path, files: &mut Vec<String>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|error| {
        panic!("ghostty-vt-sys: failed to read {}: {error}", dir.display())
    });
    for entry in entries {
        let entry = entry.unwrap_or_else(|error| {
            panic!(
                "ghostty-vt-sys: failed to read entry in {}: {error}",
                dir.display()
            )
        });
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, files);
        } else {
            let relative = path
                .strip_prefix(root)
                .expect("path is under root by construction")
                .to_string_lossy()
                .replace('\\', "/");
            files.push(relative);
        }
    }
}

/// Enforce zig >= 0.15.2, < 0.16 ourselves so the user gets an actionable
/// message instead of a Zig @compileError buried in build output (ghostty's
/// requireZig demands the same major.minor and patch >= required).
fn check_zig_version() {
    let output = Command::new("zig")
        .arg("version")
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "ghostty-vt-sys: building libghostty-vt from source requires zig (>= 0.15.2, \
             < 0.16) on PATH, but 'zig version' could not be run: {error}. Install from \
             {ZIG_DOWNLOAD_URL} or unset GHOSTTY_SOURCE_DIR/GHOSTTY_VT_FROM_SOURCE to use \
             the prebuilt library."
            )
        });
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let numeric = version.split(['-', '+']).next().unwrap_or("");
    let parts: Vec<u64> = numeric
        .split('.')
        .map(|part| part.parse().unwrap_or(u64::MAX))
        .collect();
    let compatible = matches!(parts.as_slice(), [0, 15, patch, ..] if *patch >= 2);
    assert!(
        compatible,
        "ghostty-vt-sys: zig {version} is not compatible: ghostty at the pinned commit \
         requires >= 0.15.2 and < 0.16 (its build enforces same-minor). Install 0.15.2 \
         from {ZIG_DOWNLOAD_URL}."
    );
}

/// Decide which zig `OptimizeMode` to pass. `LIBGHOSTTY_VT_SYS_OPTIMIZE`
/// overrides unconditionally; otherwise the cargo profile decides (dev →
/// Debug, opt-level s/z → ReleaseSmall, else ReleaseFast).
fn zig_optimize_mode() -> &'static str {
    if let Ok(mode) = env::var("LIBGHOSTTY_VT_SYS_OPTIMIZE") {
        return match mode.as_str() {
            "Debug" => "Debug",
            "ReleaseSafe" => "ReleaseSafe",
            "ReleaseFast" => "ReleaseFast",
            "ReleaseSmall" => "ReleaseSmall",
            other => panic!(
                "ghostty-vt-sys: LIBGHOSTTY_VT_SYS_OPTIMIZE must be one of Debug, \
                 ReleaseSafe, ReleaseFast, ReleaseSmall (got '{other}')"
            ),
        };
    }
    if env::var("DEBUG").as_deref() == Ok("true") {
        return "Debug";
    }
    match env::var("OPT_LEVEL").as_deref() {
        Ok("s") | Ok("z") => "ReleaseSmall",
        _ => "ReleaseFast",
    }
}

/// Clone ghostty at the pinned commit into OUT_DIR/ghostty-src, reusing an
/// existing clone when the stamp matches.
fn fetch_ghostty(out_dir: &Path, commit: &str) -> PathBuf {
    let src_dir = out_dir.join("ghostty-src");
    let stamp = src_dir.join(".ghostty-commit");

    if stamp.exists()
        && let Ok(existing) = std::fs::read_to_string(&stamp)
        && existing.trim() == commit
    {
        return src_dir;
    }

    if src_dir.exists() {
        std::fs::remove_dir_all(&src_dir).unwrap_or_else(|error| {
            panic!(
                "ghostty-vt-sys: failed to remove {}: {error}",
                src_dir.display()
            )
        });
    }

    eprintln!("ghostty-vt-sys: fetching ghostty {commit} ...");
    let fetch_failure = |error: &dyn std::fmt::Display| -> ! {
        panic!(
            "ghostty-vt-sys: failed to fetch ghostty {commit} from {GHOSTTY_REPO}: {error}. \
             If offline, use GHOSTTY_SOURCE_DIR with an existing checkout, or set \
             GHOSTTY_VT_LIB_DIR to a directory containing a prebuilt libghostty-vt.a."
        )
    };

    // Blobless clone: full history, lazily fetched blobs — much cheaper than
    // a full clone while still able to check out any commit.
    let clone_status = Command::new("git")
        .arg("clone")
        .arg("--filter=blob:none")
        .arg("--no-checkout")
        .arg(GHOSTTY_REPO)
        .arg(&src_dir)
        .status();
    match clone_status {
        Ok(status) if status.success() => {}
        Ok(status) => fetch_failure(&format!("git clone exited with {status}")),
        Err(error) => fetch_failure(&error),
    }

    let checkout_status = Command::new("git")
        .arg("checkout")
        .arg(commit)
        .current_dir(&src_dir)
        .status();
    match checkout_status {
        Ok(status) if status.success() => {}
        Ok(status) => fetch_failure(&format!("git checkout exited with {status}")),
        Err(error) => fetch_failure(&error),
    }

    std::fs::write(&stamp, commit)
        .unwrap_or_else(|error| panic!("ghostty-vt-sys: failed to write stamp: {error}"));

    src_dir
}

fn zig_target(target: &str) -> &'static str {
    match target {
        "x86_64-unknown-linux-gnu" => "x86_64-linux-gnu",
        "x86_64-unknown-linux-musl" => "x86_64-linux-musl",
        "aarch64-unknown-linux-gnu" => "aarch64-linux-gnu",
        "aarch64-unknown-linux-musl" => "aarch64-linux-musl",
        "aarch64-apple-darwin" => "aarch64-macos-none",
        "x86_64-apple-darwin" => "x86_64-macos-none",
        "x86_64-pc-windows-gnu" => "x86_64-windows-gnu",
        "aarch64-pc-windows-gnullvm" => "aarch64-windows-gnu",
        "x86_64-pc-windows-msvc" => "x86_64-windows-msvc",
        "aarch64-pc-windows-msvc" => "aarch64-windows-msvc",
        other => panic!(
            "ghostty-vt-sys: unsupported Rust target for a libghostty-vt source build: {other} \
             (see zig_target() in crates/ghostty_vt_sys/build.rs for the supported set)"
        ),
    }
}
