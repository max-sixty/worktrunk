{
  description = "Worktrunk - A CLI for Git worktree management";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, crane }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        # Use latest stable Rust (must meet MSRV of 1.89)
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" ];
        };

        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        # Filter source to include Cargo files plus templates (needed by askama)
        src = pkgs.lib.cleanSourceWith {
          src = ./.;
          filter = path: type:
            (craneLib.filterCargoSources path type)
            || (pkgs.lib.hasInfix "/templates/" path)
            || (builtins.baseNameOf (builtins.dirOf path) == "templates");
        };

        # Common arguments for crane builds
        commonArgs = {
          inherit src;
          strictDeps = true;

          nativeBuildInputs = with pkgs; [
            pkg-config
          ];

          buildInputs = with pkgs; [
            # Required for tree-sitter (syntax-highlighting feature, enabled by default)
            tree-sitter
          ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
            darwin.apple_sdk.frameworks.Security
            darwin.apple_sdk.frameworks.SystemConfiguration
            libiconv
          ];

          # vergen-gitcl needs git info; VERGEN_IDEMPOTENT makes it emit
          # placeholder values when .git isn't available (which is the case
          # in Nix builds since the store doesn't include .git)
          VERGEN_IDEMPOTENT = "1";

          # Optionally provide git describe via environment if flake has rev
          VERGEN_GIT_DESCRIBE = self.shortRev or self.dirtyShortRev or "nix-${self.lastModifiedDate or "unknown"}";
        };

        # Build just the cargo dependencies for caching
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        # Build the actual package
        worktrunk = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;

          # Skip tests during package build - they require:
          # - Snapshot files (insta) which bloat the source
          # - Real system time (SOURCE_DATE_EPOCH breaks some tests)
          # Tests should run in CI instead
          doCheck = false;

          meta = with pkgs.lib; {
            description = "A CLI for Git worktree management, designed for parallel AI agent workflows";
            homepage = "https://github.com/max-sixty/worktrunk";
            license = with licenses; [ mit asl20 ];
            maintainers = [ ];
            mainProgram = "wt";
          };
        });

        # Build with git-wt feature for Windows compatibility
        worktrunk-with-git-wt = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          cargoExtraArgs = "--features git-wt";
          doCheck = false;

          meta = worktrunk.meta // {
            description = "Worktrunk with git-wt binary (for 'git wt' subcommand)";
          };
        });

      in
      {
        checks = {
          inherit worktrunk;

          # Run clippy
          worktrunk-clippy = craneLib.cargoClippy (commonArgs // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets -- --deny warnings";
          });

          # Check formatting
          worktrunk-fmt = craneLib.cargoFmt { inherit src; };
        };

        packages = {
          default = worktrunk;
          inherit worktrunk;
          inherit worktrunk-with-git-wt;
        };

        apps = {
          default = flake-utils.lib.mkApp {
            drv = worktrunk;
            name = "wt";
          };
          wt = flake-utils.lib.mkApp {
            drv = worktrunk;
            name = "wt";
          };
        };

        devShells.default = craneLib.devShell {
          checks = self.checks.${system};

          packages = with pkgs; [
            # Rust tooling
            cargo-watch
            cargo-edit
            cargo-outdated
            cargo-release
            cargo-llvm-cov

            # For shell integration tests
            bash
            zsh
            fish

            # Development tools
            git
            gh
            pre-commit
          ];

          shellHook = ''
            echo "Worktrunk development shell"
            echo "  Build:  cargo build"
            echo "  Test:   cargo test"
            echo "  Lint:   cargo clippy"
          '';
        };
      }
    );
}
