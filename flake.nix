{
  description = "aterm: a minimal terminal multiplexer for coding agents";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
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
            cargo
            rustc
            clippy
            rustfmt
            rust-analyzer
          ];
        };
      });
}
