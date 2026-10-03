{ lib, rustPlatform, rustToolchain, pkg-config, linux-pam, trunk, wasmBindgenCli, binaryen }:

let
  cargoToml = builtins.fromTOML (builtins.readFile ../Cargo.toml);
in
rustPlatform.buildRustPackage {
  pname = "vexnas";
  version = cargoToml.workspace.package.version;
  src = lib.cleanSource ./..;

  cargoLock.lockFile = ../Cargo.lock;

  nativeBuildInputs = [
    rustToolchain # first: exposes the wasm32-unknown-unknown target to trunk
    pkg-config
    trunk
    wasmBindgenCli
    binaryen # wasm-opt, so trunk does not try to download one (no network in the sandbox)
  ];

  buildInputs = [ linux-pam ];

  buildPhase = ''
    runHook preBuild

    # Trunk writes cache/config files; give it a writable home in the sandbox.
    export HOME=$(mktemp -d)
    export TRUNK_TOOLS_WASM_OPT_VERSION=skip

    (cd crates/vexnas-frontend && trunk build --release)
    cargo build --release -p vexnas-web -p vexnasd

    runHook postBuild
  '';

  # Native crates only; the frontend is wasm-only.
  cargoTestFlags = [ "-p" "vexnas-proto" "-p" "vexnasd" "-p" "vexnas-web" ];

  installPhase = ''
    runHook preInstall

    mkdir -p $out/bin $out/share/vexnas/assets
    install -m755 target/release/vexnas-web target/release/vexnasd $out/bin/
    cp -r crates/vexnas-frontend/dist/* $out/share/vexnas/assets/

    runHook postInstall
  '';

  meta = {
    description = "NAS management web UI for VexOS (NixOS)";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
    mainProgram = "vexnas-web";
  };
}
