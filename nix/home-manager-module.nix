{
  lib,
  config,
  pkgs,
  worktrunk-pkgs,
  ...
}:

let
  cfg = config.programs.worktrunk;
in
{
  options.programs.worktrunk = {
    enable = lib.mkEnableOption "worktrunk - Git worktree management CLI";

    package = lib.mkOption {
      type = lib.types.package;
      default = worktrunk-pkgs.worktrunk;
      defaultText = lib.literalExpression "inputs.worktrunk.packages.\${stdenv.hostPlatform.system}.worktrunk";
      example = lib.literalExpression "inputs.worktrunk.packages.\${stdenv.hostPlatform.system}.worktrunk-with-git-wt";
      description = ''
        The worktrunk package to use.

        Use `inputs.worktrunk.packages.''${pkgs.stdenv.hostPlatform.system}.worktrunk-with-git-wt` to install as git-wt
        so `git wt <command>` works as a git subcommand. Primarily useful on Windows
        where `wt` conflicts with Windows Terminal.
      '';
    };

    enableBashIntegration = lib.hm.shell.mkBashIntegrationOption { inherit config; };

    enableZshIntegration = lib.hm.shell.mkZshIntegrationOption { inherit config; };

    enableFishIntegration = lib.hm.shell.mkFishIntegrationOption { inherit config; };

    enableNushellIntegration = lib.hm.shell.mkNushellIntegrationOption { inherit config; };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];

    programs = {
      bash.initExtra = lib.mkIf cfg.enableBashIntegration ''
        eval "$(${lib.getExe cfg.package} config shell init bash)"
      '';

      zsh.initContent = lib.mkIf cfg.enableZshIntegration ''
        eval "$(${lib.getExe cfg.package} config shell init zsh)"
      '';

      fish.interactiveShellInit = lib.mkIf cfg.enableFishIntegration ''
        ${lib.getExe cfg.package} config shell init fish | source
      '';

      nushell = lib.mkIf cfg.enableNushellIntegration {
        extraConfig = ''
          source ${
            pkgs.runCommand "worktrunk-nushell-config.nu" { } ''
              ${lib.getExe cfg.package} config shell init nu > $out
            ''
          }
        '';
      };
    };
  };
}
