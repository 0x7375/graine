{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs, ... }:
    let
      forAllSystems =
        function:
        nixpkgs.lib.genAttrs [
          "x86_64-linux"
          "aarch64-linux"
          "aarch64-darwin"
        ] (system: function nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage (finalAttrs: {
          pname = "graine";
          version = "0.1.0";
          src = pkgs.lib.sources.cleanSource ./.;

          strictDeps = true;
          cargoLock.lockFile = ./Cargo.lock;
        });

      });

      apps.default = {
        type = "app";
        program = "${self.packages.default}/bin/graine";
      };
    };
}
