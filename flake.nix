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

        # Filter source to include Cargo files, askama templates, compile-time
        # data, and the agent skills the package installs.
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
            || (pkgs.lib.hasInfix "/skills/" p)
            || (baseNameOf p == "gemini-extension.json")
            || (baseNameOf p == "imported-fixture");
        };

        # Install the agent skills where nixpkgs' `installAgentSkills` hook
        # puts them, `share/skills/worktrunk/<skill>`. home-manager's
        # `programs.worktrunk.claudeCodeIntegration` links them from there, and
        # fails the build when they are missing.
        installSkills = ''
          mkdir -p $out/share/skills/worktrunk
          cp -RL skills/. $out/share/skills/worktrunk/
        '';

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
            ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [
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

            postInstall = installSkills;

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
            postInstall = installSkills;

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

          # `nix flake check` doesn't run `homeModules` through the module
          # system, so this check does, under two stand-ins for home-manager:
          # one whose `modulesPath` has programs/worktrunk.nix (a stub declaring
          # `programs.worktrunk.package`), and a legacy one without it, which
          # gets nix/home-manager-module.nix.
          home-module =
            let
              # The legacy module builds its shell options with home-manager's lib.
              lib = pkgs.lib.extend (
                _: prev: {
                  hm.shell = prev.genAttrs [
                    "mkBashIntegrationOption"
                    "mkZshIntegrationOption"
                    "mkFishIntegrationOption"
                    "mkNushellIntegrationOption"
                  ] (_: _: prev.mkEnableOption "shell integration");
                }
              );
              eval =
                modulesPath: modules:
                (lib.evalModules {
                  specialArgs = { inherit modulesPath; };
                  modules = [
                    self.homeModules.default
                    { _module.args.pkgs = pkgs; }
                  ]
                  ++ modules;
                }).config;
              package = modulesPath: modules: (eval modulesPath modules).programs.worktrunk.package;
              current = toString ./nix/home-manager-stub;
              currentModule = "${current}/programs/worktrunk.nix";
              legacy = toString ./nix;
              # The legacy module also sets options a bare evaluation lacks,
              # such as `home.packages`; this lets those definitions pass.
              unchecked = {
                _module.check = false;
              };
              userPackage = {
                programs.worktrunk.package = pkgs.hello;
              };
            in
            # The flake's build replaces the option's default...
            assert package current [ currentModule ] == worktrunk;
            assert package legacy [ unchecked ] == worktrunk;
            # ...and the user's own setting replaces the flake's build.
            assert package current [ currentModule userPackage ] == pkgs.hello;
            assert package legacy [ unchecked userPackage ] == pkgs.hello;
            # With programs/worktrunk.nix present but not loaded, the module's
            # throw fails evaluation; `unchecked` keeps the module system's own
            # undeclared-option error from failing it instead.
            assert !(builtins.tryEval (builtins.deepSeq (eval current [ unchecked ]) null)).success;
            pkgs.runCommand "home-module-check" { } "touch $out";

          # Run tests inside the nix sandbox. Catches packaging-environment
          # bugs (#2624 is the canonical example) before nixpkgs maintainers
          # do — see .github/workflows/nightly.yaml.
          #
          # Wider src than the package build: tests need .snap files and
          # tests/ fixtures (prebuilt _git/ trees, .sh scripts, no-extension
          # git database files). Default features only — shell-integration-
          # tests wants a PTY and more shells than this derivation carries;
          # the devShell below is where that set lives (see tests/AGENTS.md →
          # "Feature Flags, Not Runtime Skipping").
          worktrunk-tests = craneLib.cargoTest (
            commonArgs
            // {
              inherit cargoArtifacts;
              src = pkgs.lib.cleanSource ./.;
              # Tests shell out to a few host tools — `git` for the harness,
              # `python3` for argv-quoting and post-start fixtures, `ps`
              # (procps) for process checks, `lsof` for fsmonitor socket
              # discovery, and `ssh-keygen` (openssh) for the `--no-ff`
              # commit-signing test. Without these on PATH
              # the sandbox surfaces them as `No such file or directory`.
              nativeBuildInputs = commonArgs.nativeBuildInputs ++ [
                pkgs.git
                pkgs.python3
                pkgs.procps
                pkgs.lsof
                pkgs.openssh
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
      # home-manager owns `programs.worktrunk`, whose `package` defaults to
      # nixpkgs' release; this module points it at this flake's build.
      homeModules.default =
        {
          lib,
          modulesPath,
          options,
          pkgs,
          ...
        }:
        {
          # home-manager ships programs/worktrunk.nix from release 26.11; older
          # releases get this flake's own module instead. Once home-manager
          # 26.05 is end-of-life (about end of December 2026), delete this
          # import, nix/home-manager-module.nix, and the home-module check's
          # legacy cases.
          imports = lib.optional (
            !builtins.pathExists "${modulesPath}/programs/worktrunk.nix"
          ) ./nix/home-manager-module.nix;

          # Reading `options` inside `config` doesn't cause infinite recursion,
          # because the module system derives which options exist from
          # declarations alone.
          config =
            if options ? programs.worktrunk.package then
              {
                programs.worktrunk.package = lib.mkDefault self.packages.${pkgs.stdenv.hostPlatform.system}.worktrunk;
              }
            else
              throw ''
                worktrunk's homeModules.default sets programs.worktrunk.package, but no module
                declares that option. This home-manager ships programs/worktrunk.nix, which
                declares it, but doesn't load it: remove it from disabledModules, or in minimal
                mode add "''${modulesPath}/programs/worktrunk.nix" to imports.
              '';
        };
    };
}
