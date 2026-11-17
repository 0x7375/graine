{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];

      forAllSystems =
        function:
        nixpkgs.lib.genAttrs systems (
          system: function system nixpkgs.legacyPackages.${system}
        );
    in
    {
      packages = forAllSystems (system: pkgs: {
        default = pkgs.rustPlatform.buildRustPackage (finalAttrs: {
          pname = "graine";
          version = "0.1.0";
          src = pkgs.lib.sources.cleanSource ./.;

          strictDeps = true;
          cargoLock.lockFile = ./Cargo.lock;
        });
      });

      apps = forAllSystems (system: pkgs: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/graine";
        };
      });

      devShells = forAllSystems (_: pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            # Rust backend: build, lint, format, and editor tooling.
            cargo
            clippy
            rust-analyzer
            rustc
            rustfmt

            # React/Vite frontend.
            nodejs
            pnpm

            # Local development and inspection tools.
            sqlite
            watchexec
          ];

          shellHook = ''
            export RUST_BACKTRACE=1
          '';
        };
      });
    };
}
