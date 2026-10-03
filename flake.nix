{
  description = "vexnas — NAS management web UI for VexOS (NixOS)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    # Linux-only (PAM, systemd), so not eachDefaultSystem.
    flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" ];
          targets = [ "wasm32-unknown-unknown" ];
        };
        customRustPlatform = pkgs.makeRustPlatform {
          rustc = rustToolchain;
          cargo = rustToolchain;
        };

        # Trunk enforces an exact match between this CLI and the wasm-bindgen crate
        # in Cargo.lock, and consumers (vexos-nix) build us against their own
        # nixpkgs-unstable, so the CLI is pinned here rather than taken from nixpkgs.
        # To bump: `cargo update -p wasm-bindgen --precise X`, then update version
        # and both hashes below (build once; Nix prints the real hashes).
        wasmBindgenCli = pkgs.rustPlatform.buildRustPackage rec {
          pname = "wasm-bindgen-cli";
          version = "0.2.129";
          src = pkgs.fetchCrate {
            inherit pname version;
            hash = "sha256-pcecKQd7E8Opw6bkFoE569epUi7gh5qpQF1e5PJY6V8=";
          };
          cargoHash = "sha256-vmUrWVU7kPJJxO5qIVeAkwQyWDELO1Z4Z5gitz2kco8=";
          nativeBuildInputs = [ pkgs.pkg-config ];
          doCheck = false;
        };
      in
      {
        packages.vexnas = pkgs.callPackage ./nix/package.nix {
          inherit rustToolchain;
          rustPlatform = customRustPlatform;
          inherit wasmBindgenCli;
        };
        packages.default = self.packages.${system}.vexnas;

        # NixOS VM tests. Run one directly: nix build .#checks.<system>.vm-login
        # (vexos-nix never runs these.)
        checks.vm-login = pkgs.callPackage ./nix/tests/login.nix {
          nixosModule = self.nixosModules.vexnas;
          vexnasPackage = self.packages.${system}.vexnas;
        };

        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            rustToolchain
            pkg-config
            linux-pam
            trunk
            wasmBindgenCli
            binaryen
          ];
        };
      }
    ) // {
      nixosModules.vexnas = ./nix/module.nix;
      nixosModules.default = self.nixosModules.vexnas;

      # Makes pkgs.vexnas available, which the NixOS module's default `package`
      # option needs:  nixpkgs.overlays = [ inputs.vexnas.overlays.default ];
      overlays.default = final: prev: {
        vexnas = self.packages.${prev.stdenv.hostPlatform.system}.vexnas;
      };
    };
}
