{
  lib,
  rustPlatform,
  stdenv,
  libiconv,
}:
let
  pname = if stdenv.hostPlatform.isDarwin then "hidpipe-client" else "hidpipe-server";
  manifest = builtins.fromTOML (
    builtins.readFile (
      if stdenv.hostPlatform.isDarwin then ../client/Cargo.toml else ../server/Cargo.toml
    )
  );
in
rustPlatform.buildRustPackage {
  inherit pname;
  inherit (manifest.package) version;

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../proto
      ../server
      ../client
    ];
  };
  cargoLock.lockFile = ../Cargo.lock;
  cargoBuildFlags = [
    "--package"
    pname
  ];
  cargoTestFlags = [
    "--package"
    pname
    "--package"
    "hidpipe-proto"
  ];

  buildInputs = lib.optionals stdenv.hostPlatform.isDarwin [ libiconv ];

  meta = {
    description = "Cross-machine HID event pipe: capture on Linux, inject on macOS";
    license = lib.licenses.mit;
    mainProgram = pname;
    platforms = lib.platforms.linux ++ lib.platforms.darwin;
  };
}
