{
  nixConfig = {
    extra-substituters = [ "https://wvhulle.cachix.org" ];
    extra-trusted-public-keys = [ "wvhulle.cachix.org-1:heXx8DZMiRsKUx6l1TxNoF+Nmtmz66QEdsonQzc1ir0=" ];
  };

  description = "Reedline - A readline-like crate for CLI text input (with LSP diagnostics)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    crane.url = "github:ipetkov/crane";
    git-to-jj.url = "git+https://codeberg.org/wvhulle/git-to-jj";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      crane,
      rust-overlay,
      git-to-jj,
      ...
    }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs {
        inherit system;
        overlays = [ (import rust-overlay) ];
      };

      craneLib = crane.mkLib pkgs;

      # Filter for .md files (used by include_str! for documentation)
      mdFilter = path: _type: builtins.match ".*\\.md$" path != null;

      src = pkgs.lib.cleanSourceWith {
        src = ./.;
        filter = path: type: (mdFilter path type) || (craneLib.filterCargoSources path type);
      };
      nightlyToolchain = pkgs.rust-bin.nightly.latest.default.override {
        extensions = [
          "clippy"
          "rust-analyzer"
        ];
      };

      nightlyCrane = (crane.mkLib pkgs).overrideToolchain nightlyToolchain;
      commonArgs = {
        inherit src;
        pname = "reedline";
        version = "0.44.0-lsp";

        nativeBuildInputs = with pkgs; [ pkg-config ];
        buildInputs = with pkgs; [ ];

        # Build with lsp_diagnostics feature
        cargoExtraArgs = "--features lsp_diagnostics";
      };

      # Build dependencies separately - this gets cached
      cargoArtifacts = craneLib.buildDepsOnly commonArgs;

      rulesPackage = git-to-jj.packages.${system}.default;
      yamlFormat = pkgs.formats.yaml { };
      sgconfig = yamlFormat.generate "sgconfig.yml" {
        ruleDirs = [ "${rulesPackage}/rules-bash" ];
      };
    in
    {
      packages.${system} = {
        default = nightlyCrane.buildPackage (
          commonArgs
          // {
            inherit cargoArtifacts;
            doCheck = false;

            meta = {
              description = "A readline-like crate for CLI text input (with LSP diagnostics)";
              homepage = "https://github.com/nushell/reedline";
            };
          }
        );

        # Export the cargoArtifacts for nushell to reuse
        cargoArtifacts = cargoArtifacts;
      };

      # Export source and build tools for nushell to include
      lib.${system} = {
        inherit
          src
          cargoArtifacts
          commonArgs
          craneLib
          ;
      };

      devShells.${system}.default = pkgs.mkShell {
        inputsFrom = [
          (nightlyCrane.buildPackage (
            commonArgs
            // {
              inherit cargoArtifacts;
              doCheck = false;
            }
          ))
        ];
        packages = [ pkgs.ast-grep ];
        shellHook = ''
          ln -sfn ${sgconfig} sgconfig.yml
        '';
      };
    };
}
