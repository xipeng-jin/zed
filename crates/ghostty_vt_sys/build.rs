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
//! 4. Default — fetch the Zed-published prebuilt archive from the
//!    `prebuilt_repo` release, verified against the sha256 pins in
//!    `ghostty_pin.toml` before unpacking. No zig, no git, no bindgen.
//!    Interim: while the pin file's `[sha256]` table is empty (no release
//!    published yet) this falls back to path 3 with a warning.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

const PIN_FILE: &str = include_str!("ghostty_pin.toml");
const DOCS_URL: &str = "docs/ghostty-migration/build-strategy.md";

struct Pin {
    commit: String,
    release: String,
    source_repo: String,
    prebuilt_repo: String,
    /// ghostty's `minimum_zig_version` at `commit`; source builds enforce
    /// same major.minor and patch >= (ghostty's own `requireZig` rule).
    zig: ZigVersion,
    /// Default `-Dcpu` for source builds; matches the published prebuilts.
    cpu: String,
    /// `-Dvt-features` passed verbatim on every source build; matches the
    /// published prebuilts.
    vt_features: String,
    headers_sha256: String,
    prebuilt_sha256: BTreeMap<String, String>,
}

struct ZigVersion {
    major: u64,
    minor: u64,
    patch: u64,
}

impl ZigVersion {
    /// Parses the numeric prefix of a zig version string; `0.16.0-dev.123`
    /// counts as 0.16.0.
    fn parse(version: &str) -> Option<Self> {
        let numeric = version.trim().split(['-', '+']).next()?;
        let mut parts = numeric.split('.').map(|part| part.parse::<u64>().ok());
        let major = parts.next()??;
        let minor = parts.next()??;
        let patch = parts.next()??;
        Some(Self {
            major,
            minor,
            patch,
        })
    }

    fn accepts(&self, found: &Self) -> bool {
        found.major == self.major && found.minor == self.minor && found.patch >= self.patch
    }

    fn download_url(&self) -> String {
        format!("https://ziglang.org/download/#release-{self}")
    }

    fn window(&self) -> String {
        format!(">= {self}, < {}.{}", self.major, self.minor + 1)
    }
}

impl std::fmt::Display for ZigVersion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

fn main() {
    // docs.rs has no toolchains; the checked-in src/bindings.rs is enough for
    // generating documentation.
    if env::var("DOCS_RS").is_ok() {
        return;
    }

    // Miri cannot load or call the native library; nothing to build or link.
    if env::var("CARGO_CFG_MIRI").is_ok() {
        return;
    }

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=ghostty_pin.toml");
    println!("cargo:rerun-if-env-changed=GHOSTTY_VT_LIB_DIR");
    println!("cargo:rerun-if-env-changed=GHOSTTY_SOURCE_DIR");
    println!("cargo:rerun-if-env-changed=GHOSTTY_VT_FROM_SOURCE");
    println!("cargo:rerun-if-env-changed=GHOSTTY_ZIG_SYSTEM_DIR");
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_SYS_OPTIMIZE");
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_SYS_CPU");
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
    } else if pin.prebuilt_sha256.is_empty() {
        // Interim until the artifact pipeline publishes the first v2 release:
        // keep plain `cargo build` green by building at the pin. The first
        // stamped hash makes this branch unreachable.
        println!(
            "cargo:warning=ghostty_pin.toml has no prebuilt sha256 entries yet (release {} not \
             published); falling back to a pinned source build of libghostty-vt (requires zig \
             {} and git on PATH)",
            pin.release,
            pin.zig.window()
        );
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
    let mut release = None;
    let mut source_repo = None;
    let mut prebuilt_repo = None;
    let mut zig = None;
    let mut cpu = None;
    let mut vt_features = None;
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
                "release" => release = Some(value),
                "source_repo" => source_repo = Some(value),
                "prebuilt_repo" => prebuilt_repo = Some(value),
                "zig" => zig = Some(value),
                "cpu" => cpu = Some(value),
                "vt_features" => vt_features = Some(value),
                "headers_sha256" => headers_sha256 = Some(value),
                other => {
                    panic!("ghostty-vt-sys: unrecognized key '{other}' in ghostty_pin.toml")
                }
            }
        }
    }

    let commit = commit.expect("ghostty-vt-sys: ghostty_pin.toml is missing 'commit'");
    assert!(
        commit.len() == 40 && commit.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "ghostty-vt-sys: 'commit' in ghostty_pin.toml must be a full 40-char hex commit \
         (got '{commit}')"
    );

    let zig = zig.expect("ghostty-vt-sys: ghostty_pin.toml is missing 'zig'");
    let zig = ZigVersion::parse(&zig).unwrap_or_else(|| {
        panic!("ghostty-vt-sys: 'zig' in ghostty_pin.toml must be major.minor.patch (got '{zig}')")
    });
    let cpu = cpu.expect("ghostty-vt-sys: ghostty_pin.toml is missing 'cpu'");
    assert!(
        !cpu.is_empty(),
        "ghostty-vt-sys: 'cpu' in ghostty_pin.toml must not be empty"
    );

    Pin {
        commit,
        release: release.expect("ghostty-vt-sys: ghostty_pin.toml is missing 'release'"),
        source_repo: source_repo
            .expect("ghostty-vt-sys: ghostty_pin.toml is missing 'source_repo'"),
        prebuilt_repo: prebuilt_repo
            .expect("ghostty-vt-sys: ghostty_pin.toml is missing 'prebuilt_repo'"),
        zig,
        cpu,
        vt_features: vt_features
            .expect("ghostty-vt-sys: ghostty_pin.toml is missing 'vt_features'"),
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
    emit_link_directives(dir, target);
}

fn emit_link_directives(dir: &Path, target: &str) {
    println!("cargo:rustc-link-search=native={}", dir.display());
    println!("cargo:rustc-link-lib=static={}", static_link_name(target));
}

/// MSVC resolves `ghostty-vt` to `ghostty-vt.lib`, the DLL import library;
/// ghostty names the static archive `ghostty-vt-static.lib` to avoid that.
fn static_link_name(target: &str) -> &'static str {
    if target.contains("windows") && target.contains("msvc") {
        "ghostty-vt-static"
    } else {
        "ghostty-vt"
    }
}

/// Path 4 (default): fetch the Zed-published prebuilt archive for this
/// target from the `prebuilt_repo` release and verify it against the sha256
/// pinned in `ghostty_pin.toml` before unpacking. The unpacked archive is
/// cached in OUT_DIR keyed by its hash, so `cargo clean` costs one
/// re-download and a pin bump invalidates the cache automatically.
fn fetch_prebuilt(pin: &Pin, target: &str) {
    let Some(expected_sha256) = pin.prebuilt_sha256.get(target) else {
        let available = pin
            .prebuilt_sha256
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        panic!(
            "ghostty-vt-sys: no prebuilt libghostty-vt for target {target} \
             (available: {available}). Build from source with GHOSTTY_VT_FROM_SOURCE=1 \
             (requires zig {zig} on PATH), or set GHOSTTY_VT_LIB_DIR to a directory \
             containing {archive}. See {DOCS_URL}.",
            archive = static_archive_name(target),
            zig = pin.zig.window(),
        );
    };

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR must be set"));
    let commit10 = pin
        .commit
        .get(..10)
        .expect("commit length is validated in parse_pin");
    let archive = static_archive_name(target);
    let asset_name = format!("libghostty-vt-{commit10}-{target}.tar.gz");
    let unpack_dir = out_dir.join("ghostty-prebuilt");
    let stamp = unpack_dir.join(".sha256-stamp");

    let cache_is_valid = std::fs::read_to_string(&stamp)
        .is_ok_and(|existing| existing.trim() == expected_sha256)
        && unpack_dir.join(archive).exists();
    if !cache_is_valid {
        let url = format!(
            "https://github.com/{repo}/releases/download/{release}/{asset_name}",
            repo = pin.prebuilt_repo,
            release = pin.release,
        );
        if unpack_dir.exists() {
            std::fs::remove_dir_all(&unpack_dir).unwrap_or_else(|error| {
                panic!(
                    "ghostty-vt-sys: failed to remove {}: {error}",
                    unpack_dir.display()
                )
            });
        }
        std::fs::create_dir_all(&unpack_dir).unwrap_or_else(|error| {
            panic!(
                "ghostty-vt-sys: failed to create {}: {error}",
                unpack_dir.display()
            )
        });

        let archive_path = out_dir.join(&asset_name);
        eprintln!("ghostty-vt-sys: downloading {url} ...");
        let download_failure = |cause: &dyn std::fmt::Display| -> ! {
            panic!(
                "ghostty-vt-sys: failed to download prebuilt libghostty-vt for {target} from \
                 {url}: {cause}. If you are offline, set GHOSTTY_VT_LIB_DIR to a directory \
                 containing {archive}, or build from source with GHOSTTY_VT_FROM_SOURCE=1 \
                 (requires zig {zig}). See {DOCS_URL}.",
                zig = pin.zig.window(),
            )
        };
        // curl shell-out, following the ConPTY-download precedent in
        // crates/zed/build.rs; bsdtar on macOS/Windows 10+ handles the same
        // invocation for the later platform gates.
        let output = Command::new("curl")
            .arg("-fsSL")
            .arg("--retry")
            .arg("3")
            .arg("-o")
            .arg(&archive_path)
            .arg(&url)
            .output();
        match output {
            Ok(output) if output.status.success() => {}
            Ok(output) => download_failure(&format!(
                "curl exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )),
            Err(error) => download_failure(&format!("failed to execute curl: {error}")),
        }

        let archive_bytes = std::fs::read(&archive_path).unwrap_or_else(|error| {
            panic!(
                "ghostty-vt-sys: failed to read downloaded archive {}: {error}",
                archive_path.display()
            )
        });
        let actual_sha256 = format!("{:x}", Sha256::digest(&archive_bytes));
        assert!(
            actual_sha256 == *expected_sha256,
            "ghostty-vt-sys: prebuilt libghostty-vt for {target} failed checksum verification \
             (expected {expected_sha256}, got {actual_sha256}). Refusing to link. Delete {} \
             and retry; if this persists, the release asset or the pin in ghostty_pin.toml \
             is wrong.",
            archive_path.display()
        );

        let status = Command::new("tar")
            .arg("-xzf")
            .arg(&archive_path)
            .arg("-C")
            .arg(&unpack_dir)
            .status()
            .unwrap_or_else(|error| {
                panic!("ghostty-vt-sys: failed to execute tar: {error}");
            });
        assert!(
            status.success(),
            "ghostty-vt-sys: tar failed (status {status}) unpacking {}",
            archive_path.display()
        );
        assert!(
            unpack_dir.join(archive).exists(),
            "ghostty-vt-sys: expected {archive} in {} after unpacking {asset_name}",
            unpack_dir.display()
        );

        std::fs::write(&stamp, expected_sha256)
            .unwrap_or_else(|error| panic!("ghostty-vt-sys: failed to write stamp: {error}"));
    }

    emit_link_directives(&unpack_dir, target);
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

    check_zig_version(&pin.zig);

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
        SourceCheckout::PinnedFetch => fetch_ghostty(&out_dir, &pin.source_repo, &pin.commit),
    };

    let install_prefix = out_dir.join("ghostty-install");
    let zig_cache_dir = out_dir.join("zig-cache");
    let zig_global_cache_dir = out_dir.join("zig-global-cache");
    let optimize = zig_optimize_mode();
    // Distributed binaries may run on older CPUs than the build host, so the
    // default is the pin's portable `baseline`; LIBGHOSTTY_VT_SYS_CPU is the
    // override for a known machine (`native`, `x86_64_v3`, ...).
    let cpu = env::var("LIBGHOSTTY_VT_SYS_CPU").unwrap_or_else(|_| pin.cpu.clone());
    assert!(
        !cpu.is_empty(),
        "ghostty-vt-sys: LIBGHOSTTY_VT_SYS_CPU must not be empty when set"
    );

    let mut build = Command::new("zig");
    build
        .arg("build")
        .arg("-Demit-lib-vt=true")
        .arg(format!("-Doptimize={optimize}"))
        .arg(format!("-Dcpu={cpu}"))
        .arg("-Demit-xcframework=false")
        .arg("-Dapp-runtime=none")
        .arg("--prefix")
        .arg(&install_prefix)
        .arg("--cache-dir")
        .arg(&zig_cache_dir)
        .current_dir(&ghostty_dir);
    if !pin.vt_features.is_empty() {
        build.arg(format!("-Dvt-features={}", pin.vt_features));
    }

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
    println!("cargo:rustc-link-lib=static={}", static_link_name(target));
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

/// Enforce the zig window from `ghostty_pin.toml` ourselves so the user gets
/// an actionable message instead of a Zig @compileError buried in build
/// output (ghostty's requireZig demands the same major.minor and patch >=
/// the pinned minimum).
fn check_zig_version(required: &ZigVersion) {
    let window = required.window();
    let download_url = required.download_url();
    let output = Command::new("zig")
        .arg("version")
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "ghostty-vt-sys: building libghostty-vt from source requires zig ({window}) \
                 on PATH, but 'zig version' could not be run: {error}. Install from \
                 {download_url} or unset GHOSTTY_SOURCE_DIR/GHOSTTY_VT_FROM_SOURCE to use \
                 the prebuilt library."
            )
        });
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let compatible = ZigVersion::parse(&version).is_some_and(|found| required.accepts(&found));
    assert!(
        compatible,
        "ghostty-vt-sys: zig {version} is not compatible: ghostty at the pinned commit \
         requires {window} (its build enforces same-minor). Install {required} from \
         {download_url}."
    );
}

/// Decide which zig `OptimizeMode` to pass. Always ReleaseFast regardless of
/// cargo profile — zig-Debug cores degrade `vt_write` ~3000× with non-empty
/// scrollback (docs/ghostty-migration/spike-findings.md), so a plain dev
/// build must never silently link a Debug core. `LIBGHOSTTY_VT_SYS_OPTIMIZE`
/// is the explicit override for debugging inside ghostty.
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
    "ReleaseFast"
}

/// Clone ghostty at the pinned commit into OUT_DIR/ghostty-src, reusing an
/// existing clone when the stamp matches.
fn fetch_ghostty(out_dir: &Path, source_repo: &str, commit: &str) -> PathBuf {
    let src_dir = out_dir.join("ghostty-src");
    let stamp = src_dir.join(".ghostty-commit");
    let source_url = format!("https://github.com/{source_repo}.git");

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
            "ghostty-vt-sys: failed to fetch ghostty {commit} from {source_url}: {error}. \
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
        .arg(&source_url)
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
