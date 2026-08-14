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

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      rust-overlay,
      crane,
    }:
    # `eachDefaultSystem` minus x86_64-darwin: nixpkgs drops Intel macOS in
    # 26.11, so that system stops evaluating once flake.lock advances past
    # it. Release binaries still ship for Intel macOS (dist-workspace.toml);
    # this is the Nix surface only.
    flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ] (
      system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        # Pin to the channel declared in rust-toolchain.toml so rustup and Nix
        # stay on the same version. Extensions are dev-shell-only, so we keep
        # them here rather than in rust-toolchain.toml (which CI also reads).
        toolchainChannel = (builtins.fromTOML (builtins.readFile ./rust-toolchain.toml)).toolchain.channel;
        rustToolchain = pkgs.rust-bin.stable.${toolchainChannel}.default.override {
          extensions = [
            "rust-src"
            "rust-analyzer"
          ];
        };

        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        # Filter source to include Cargo files, askama templates, and compile-time data.
        # Gemini's loader requires its manifest at the repo root (see #2807).
        # The benchmark fixture definition stays extensionless so Cargo does not
        # auto-discover it as a bench target.
        src = pkgs.lib.cleanSourceWith {
          src = ./.;
          filter =
            p: type:
            (craneLib.filterCargoSources p type)
            || (pkgs.lib.hasInfix "/templates/" p)
            || (baseNameOf (dirOf p) == "templates")
            || (pkgs.lib.hasInfix "/dev/" p)
            || (baseNameOf (dirOf p) == "dev")
            || (baseNameOf p == "gemini-extension.json")
            || (baseNameOf p == "imported-fixture");
        };

        # Common arguments for crane builds
        commonArgs = {
          inherit src;
          strictDeps = true;

          nativeBuildInputs = with pkgs; [
            pkg-config
          ];

          buildInputs =
            with pkgs;
            [
              # Required for tree-sitter (syntax-highlighting feature, enabled by default)
              tree-sitter
            ]
            ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
              libiconv
            ];

          # vergen-gitcl needs git info; VERGEN_IDEMPOTENT makes it emit
          # placeholder values when .git isn't available (which is the case
          # in Nix builds since the store doesn't include .git)
          VERGEN_IDEMPOTENT = "1";

          # Optionally provide git describe via environment if flake has rev
          VERGEN_GIT_DESCRIBE =
            self.shortRev or self.dirtyShortRev or "nix-${self.lastModifiedDate or "unknown"}";
        };

        # Build just the cargo dependencies for caching.
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        # Build the actual package
        worktrunk = craneLib.buildPackage (
          commonArgs
          // {
            inherit cargoArtifacts;

            # Skip tests during package build - they require snapshot files (insta)
            # which bloat the source. Tests should run in CI instead.
            doCheck = false;

            meta = with pkgs.lib; {
              description = "A CLI for Git worktree management, designed for parallel AI agent workflows";
              homepage = "https://github.com/max-sixty/worktrunk";
              license = with licenses; [
                mit
                asl20
              ];
              maintainers = [ ];
              mainProgram = "wt";
            };
          }
        );

        # Build with git-wt feature for Windows compatibility
        worktrunk-with-git-wt = craneLib.buildPackage (
          commonArgs
          // {
            inherit cargoArtifacts;
            cargoExtraArgs = "--features git-wt";
            doCheck = false;

            meta = worktrunk.meta // {
              description = "Worktrunk with git-wt binary (for 'git wt' subcommand)";
            };
          }
        );

      in
      {
        checks = {
          inherit worktrunk;

          # Run clippy
          worktrunk-clippy = craneLib.cargoClippy (
            commonArgs
            // {
              inherit cargoArtifacts;
              cargoClippyExtraArgs = "--all-targets -- --deny warnings";
            }
          );

          # Check formatting
          worktrunk-fmt = craneLib.cargoFmt { inherit src; };

          # Run tests inside the nix sandbox. Catches packaging-environment
          # bugs (#2624 is the canonical example) before nixpkgs maintainers
          # do — see .github/workflows/nightly.yaml.
          #
          # Wider src than the package build: tests need .snap files and
          # tests/ fixtures (prebuilt _git/ trees, .sh scripts, no-extension
          # git database files). Default features only — shell-integration-
          # tests wants a PTY and more shells than this derivation carries;
          # the devShell below is where that set lives (see tests/CLAUDE.md →
          # "Feature Flags, Not Runtime Skipping").
          worktrunk-tests = craneLib.cargoTest (
            commonArgs
            // {
              inherit cargoArtifacts;
              src = pkgs.lib.cleanSource ./.;
              # Tests shell out to a few host tools — `git` for the harness,
              # `python3` for argv-quoting and post-start fixtures, `ps`
              # (procps) for the pgid invariant test, `lsof` for the
              # `--reap` process-discovery test. Without these on PATH
              # the sandbox surfaces them as `No such file or directory`.
              nativeBuildInputs = commonArgs.nativeBuildInputs ++ [
                pkgs.git
                pkgs.python3
                pkgs.procps
                pkgs.lsof
              ];
            }
          );
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

            # Shells the `shell-integration-tests` feature drives, plus the
            # `jq` its Claude-hook tests pipe the hook payload through. The
            # pre-merge gate runs `--all-features`, so a run here exercises
            # every one.
            bash
            zsh
            fish
            nushell
            powershell
            jq

            # Development tools. `git` comes from the `checks` above: crane
            # folds each check's `nativeBuildInputs` in via `inputsFrom`.
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
    )
    // {
      homeModules = {
        default =
          {
            lib,
            config,
            pkgs,
            ...
          }:
          (import ./nix/home-manager-module.nix) {
            inherit lib config pkgs;
            worktrunk-pkgs = self.packages.${pkgs.stdenv.hostPlatform.system};
          };
      };
    };
}
