{
  description = "aterm: a minimal terminal multiplexer for coding agents";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    # Nightly rustfmt, for the unstable options in rustfmt.toml.
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };
        shellHook = "[[ $- == *i* ]] && exec fish || true";
      in {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "aterm";
          version = "0.1.0";
          src = self;
          cargoLock.lockFile = ./Cargo.lock;
          # Tests spawn PTYs, which the Nix build sandbox does not provide.
          doCheck = false;
        };

        devShells.default = pkgs.mkShell {
          inherit shellHook;
          packages = with pkgs; [
            # Listed before the stable toolchain so `cargo fmt` finds it first.
            (rust-bin.selectLatestNightlyWith (toolchain: toolchain.rustfmt))
            cargo
            rustc
            clippy
            rust-analyzer
          ];
        };
      });
}
