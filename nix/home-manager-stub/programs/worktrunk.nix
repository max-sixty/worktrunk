# Stands in for home-manager's own programs/worktrunk.nix in the flake's
# home-module check. It sits at the path homeModules.default probes under
# `modulesPath`, and declares only the option homeModules.default sets.
{ lib, ... }:
{
  options.programs.worktrunk.package = lib.mkOption {
    type = lib.types.nullOr lib.types.package;
    default = null;
  };
}
