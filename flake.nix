{
  description = "Supabase Edge Runtime";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs";
    flake-utils.url = "github:numtide/flake-utils";
  };
  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
      in {
        packages.edge-runtime = pkgs.callPackage ./nix/edge-runtime.nix { };
        defaultPackage = self.packages.${system}.edge-runtime;
      });
}

