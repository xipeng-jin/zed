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
  commit10 = "636ce3a46f";
  target = stdenvNoCC.hostPlatform.rust.rustcTarget;
  sha256s = {
    "x86_64-unknown-linux-gnu" = "a45f2cdc3bfa1562d0b056f9803b0bf07a89c9071e9caf286795094f9a8c51b9";
    "aarch64-unknown-linux-gnu" = "617755cd144532ab279d6e764b25773fcb49d48faa424142b7f62fec9886974a";
    "x86_64-unknown-linux-musl" = "46c236f81d74ad44cfe2d579f82ad5b8cae1304dceee1a83f35cea0abae960c5";
    "aarch64-unknown-linux-musl" = "12de72930cf03cf1ee1abc1f91a3b17b505917f1c8e7efa8a82a14fc8511667c";
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
