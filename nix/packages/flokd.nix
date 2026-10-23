# The Rust service daemon. See flokd.md.
{ pkgs, lib, version }:

pkgs.rustPlatform.buildRustPackage {
  pname = "flokd";
  inherit version;

  src = lib.cleanSource ../../flokd;
  cargoLock.lockFile = ../../flokd/Cargo.lock;

  meta = {
    description = "Service daemon for flokshell";
    mainProgram = "flokd";
  };
}
