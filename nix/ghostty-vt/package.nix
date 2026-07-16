# Prebuilt libghostty-vt for the terminal backend, consumed by
# crates/ghostty_vt_sys via GHOSTTY_VT_LIB_DIR (wired in build.nix). The
# fixed-output fetch links the byte-identical, attestation-covered artifact
# every other build links (docs/ghostty-migration/artifact-pipeline.md §6).
#
# The release/commit/hashes mirror crates/ghostty_vt_sys/ghostty_pin.toml and
# must be updated in the same PR as any pin bump.
{
  stdenvNoCC,
  fetchurl,
}:
let
  prebuiltRepo = "xipeng-jin/libghostty-vt-prebuilt";
  commit10 = "a887df42c5";
  target = stdenvNoCC.hostPlatform.rust.rustcTarget;
  sha256s = {
    "x86_64-unknown-linux-gnu" = "47896301eb32f113169385815b469a7534cc90b32ac6136aaa744e0d4fd38530";
    "aarch64-unknown-linux-gnu" = "fbe83b84646c8100e5e1580f4daf20a6eb3a85ece838c137bc7c0d55c0082412";
    "x86_64-unknown-linux-musl" = "2f5c46ced9f497421f925a446eec7196d286177f1f6b180ff5314a864ae4333b";
    "aarch64-unknown-linux-musl" = "053a1be4760e3bbd8be865b72f07fb8d51a5b636123b991ef635961b927cb3b4";
  };
in
stdenvNoCC.mkDerivation {
  pname = "libghostty-vt-prebuilt";
  version = commit10;

  src = fetchurl {
    url = "https://github.com/${prebuiltRepo}/releases/download/ghostty-${commit10}/libghostty-vt-${commit10}-${target}.tar.gz";
    sha256 = sha256s.${target};
  };

  sourceRoot = ".";

  installPhase = ''
    mkdir -p $out
    cp libghostty-vt.a LICENSE $out/
  '';
}
