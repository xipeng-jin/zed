use std::env;
use std::path::{Path, PathBuf};

use bindgen::EnumVariation;
use bindgen::callbacks::{EnumVariantValue, IntKind, ItemInfo, ItemKind, ParseCallbacks};

use heck::ToShoutySnakeCase;
use sha2::{Digest, Sha256};

fn main() {
    // The include directory is produced by build.rs. After a successful
    // source build of this crate (GHOSTTY_VT_FROM_SOURCE=1 or
    // GHOSTTY_SOURCE_DIR), the headers live in:
    //   target/<profile>/build/ghostty_vt_sys-<hash>/out/ghostty-install/include
    //
    // For convenience, also allow GHOSTTY_SOURCE_DIR/include or
    // an explicit GHOSTTY_INCLUDE_DIR override.
    let include_dir = if let Ok(dir) = env::var("GHOSTTY_INCLUDE_DIR") {
        PathBuf::from(dir)
    } else if let Ok(src) = env::var("GHOSTTY_SOURCE_DIR") {
        PathBuf::from(src).join("include")
    } else {
        // Walk target/debug/build/ to find the libghostty-vt-sys output.
        let manifest_dir =
            PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set"));
        let workspace_root = manifest_dir
            .parent()
            .and_then(std::path::Path::parent)
            .expect("workspace root must exist")
            .to_path_buf();

        let build_dir = workspace_root.join("target").join("debug").join("build");
        let mut found = None;
        if let Ok(entries) = std::fs::read_dir(&build_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.starts_with("ghostty_vt_sys-") {
                    let candidate = entry
                        .path()
                        .join("out")
                        .join("ghostty-install")
                        .join("include");
                    if candidate.join("ghostty").join("vt.h").exists() {
                        found = Some(candidate);
                        break;
                    }
                }
            }
        }
        found.unwrap_or_else(|| {
            panic!(
                "could not find ghostty headers; run `GHOSTTY_VT_FROM_SOURCE=1 cargo build \
                 -p ghostty_vt_sys` first, or set GHOSTTY_INCLUDE_DIR or GHOSTTY_SOURCE_DIR"
            )
        })
    };

    let header = include_dir.join("ghostty").join("vt.h");
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set"));
    let out = manifest_dir.join("src").join("bindings.rs");

    let mut builder = bindgen::Builder::default()
        .header(header.to_string_lossy())
        .clang_arg(format!("-I{}", include_dir.to_string_lossy()))
        .allowlist_function("[Gg]hostty.*")
        .allowlist_type("[Gg]hostty.*")
        .allowlist_var("GHOSTTY_.*")
        .generate_cstr(true)
        .derive_default(true)
        .size_t_is_usize(true)
        .default_enum_style(EnumVariation::ModuleConsts)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .parse_callbacks(Box::new(Callbacks));

    if cfg!(target_os = "linux") {
        builder = builder.clang_arg("-I/usr/include");
    }

    let bindings = builder
        .generate()
        .expect("failed to generate bindings from include/ghostty/vt.h");

    bindings
        .write_to_file(&out)
        .unwrap_or_else(|error| panic!("failed to write bindings to {}: {error}", out.display()));

    stamp_headers_sha256(&manifest_dir, &include_dir);
}

/// Stamps the digest of the headers the bindings were generated from into
/// ghostty_pin.toml, where build.rs verifies it on pinned source builds. Must
/// mirror hash_headers() in build.rs.
fn stamp_headers_sha256(manifest_dir: &Path, include_dir: &Path) {
    let mut files = Vec::new();
    collect_files(include_dir, include_dir, &mut files);
    files.sort();
    let mut hasher = Sha256::new();
    for relative_path in &files {
        hasher.update(relative_path.as_bytes());
        hasher.update([0]);
        let contents = std::fs::read(include_dir.join(relative_path))
            .unwrap_or_else(|error| panic!("failed to read header {relative_path}: {error}"));
        hasher.update(&contents);
        hasher.update([0]);
    }
    let digest = format!("{:x}", hasher.finalize());

    let pin_path = manifest_dir.join("ghostty_pin.toml");
    let pin = std::fs::read_to_string(&pin_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", pin_path.display()));
    let mut stamped = false;
    let updated: Vec<String> = pin
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("headers_sha256") {
                stamped = true;
                format!("headers_sha256 = \"{digest}\"")
            } else {
                line.to_string()
            }
        })
        .collect();
    assert!(
        stamped,
        "no headers_sha256 line found in {}",
        pin_path.display()
    );
    std::fs::write(&pin_path, updated.join("\n") + "\n")
        .unwrap_or_else(|error| panic!("failed to write {}: {error}", pin_path.display()));
    println!("stamped headers_sha256 = {digest} into ghostty_pin.toml");
}

fn collect_files(root: &Path, dir: &Path, files: &mut Vec<String>) {
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", dir.display()));
    for entry in entries {
        let entry = entry
            .unwrap_or_else(|error| panic!("failed to read entry in {}: {error}", dir.display()));
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

const PREFIXES: &[(&str, &str)] = &[
    ("GhosttyOptimizeMode", "GHOSTTY_OPTIMIZE"),
    ("GhosttyKeyEncoderOption", "GHOSTTY_KEY_ENCODER_OPT"),
    ("GhosttyMouseTrackingMode", "GHOSTTY_MOUSE_TRACKING"),
    ("GhosttyMouseEncoderOption", "GHOSTTY_MOUSE_ENCODER_OPT"),
    ("GhosttySgrAttributeTag", "GHOSTTY_SGR_ATTR"),
    ("GhosttyOscCommandData", "GHOSTTY_OSC_DATA"),
    ("GhosttyOscCommandType", "GHOSTTY_OSC_COMMAND"),
    ("GhosttyTerminalOption", "GHOSTTY_TERMINAL_OPT"),
    (
        "GhosttyTerminalScrollViewportTag",
        "GHOSTTY_SCROLL_VIEWPORT",
    ),
    ("GhosttyStyleColorTag", "GHOSTTY_STYLE_COLOR"),
    ("GhosttyRowSemanticPrompt", "GHOSTTY_ROW_SEMANTIC"),
    ("GhosttyCellSemanticContent", "GHOSTTY_CELL_SEMANTIC"),
    ("GhosttyCellContentTag", "GHOSTTY_CELL_CONTENT"),
    ("GhosttySizeReportStyle", "GHOSTTY_SIZE_REPORT"),
    ("GhosttyModeReportState", "GHOSTTY_MODE_REPORT"),
    ("GhosttyFocusEvent", "GHOSTTY_FOCUS"),
    ("GhosttyResult", "GHOSTTY_"),
    ("GhosttyKittyGraphicsImageData", "GHOSTTY_KITTY_IMAGE_DATA"),
    (
        "GhosttySelectionGestureEventOption",
        "GHOSTTY_SELECTION_GESTURE_EVENT_OPT",
    ),
];

#[derive(Debug)]
struct Callbacks;

impl ParseCallbacks for Callbacks {
    fn item_name(&self, item_info: ItemInfo) -> Option<String> {
        let prefix = match item_info.kind {
            // Do not rename functions since bindgen unconditionally prefixes
            // the `link_name` with `\u{1}`, which was supposed to stop LLVM
            // from mangling the name again but apparently this is necessary
            // on macOS and other Apple platforms?
            //
            // Honestly, what the hell. See:
            // https://github.com/rust-lang/rust-bindgen/issues/1221
            ItemKind::Function => return None,
            ItemKind::Var => "GHOSTTY_",
            _ => "Ghostty",
        };
        Some(item_info.name.trim_start_matches(prefix).to_string())
    }

    fn enum_variant_name(
        &self,
        enum_name: Option<&str>,
        original_variant_name: &str,
        _variant_value: EnumVariantValue,
    ) -> Option<String> {
        let enum_name = enum_name?;

        // Remove redundant C prefixes
        let prefix = PREFIXES
            .into_iter()
            .find(|(v, _)| *v == enum_name)
            .map(|(_, n)| n.to_string())
            .unwrap_or(enum_name.to_shouty_snake_case());

        let transformed = original_variant_name
            .trim_start_matches(&prefix)
            .trim_start_matches('_');

        Some(transformed.to_string())
    }

    fn process_comment(&self, comment: &str) -> Option<String> {
        Some(
            comment
                .lines()
                // Ignore doxygen directives.
                .filter(|s| !s.trim().starts_with("@"))
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    fn int_macro(&self, name: &str, _value: i64) -> Option<IntKind> {
        // Fixup some int macro types to reduce manual casting
        if name.starts_with("GHOSTTY_DA_") || name.starts_with("GHOSTTY_MODS_") {
            Some(IntKind::U16)
        } else if name.starts_with("GHOSTTY_KITTY_KEY_") || name.starts_with("GHOSTTY_COLOR_NAMED_")
        {
            Some(IntKind::U8)
        } else {
            None
        }
    }
}
