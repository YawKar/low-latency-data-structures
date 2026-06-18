{
  description = "low-latency-data-structures";

  inputs = {
    flake-parts.url = "github:hercules-ci/flake-parts";
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs =
    inputs@{ flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [ "x86_64-linux" ];

      perSystem =
        { system, ... }:
        let
          pkgs = import inputs.nixpkgs {
            inherit system;
            overlays = [ (import inputs.rust-overlay) ];
          };
        in
        {
          devShells.default = pkgs.mkShell {
            nativeBuildInputs = with pkgs; [
              bashInteractive
              rustPlatform.bindgenHook
              ((rust-bin.fromRustupToolchainFile ./rust-toolchain.toml).override {
                extensions = [
                  "rust-src"
                  "rust-analyzer"
                ];
              })
              perf
              heaptrack
              nixfmt
              statix
              yamlfmt
              just
              pre-commit
            ];

            shellHook = ''
              pre-commit uninstall && pre-commit install
              echo "[FLAKE] DevShell for low-latency-data-structures development is loaded!"
            '';
          };
        };
    };
}
