{ config, lib, pkgs, ... }:

let
  cfg = config.programs.freebuff-auth;
in
{
  options.programs.freebuff-auth = {
    enable = lib.mkEnableOption "the Freebuff Auth account switcher";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.freebuff-auth or (pkgs.callPackage ./package.nix { });
      defaultText = lib.literalMD "`pkgs.freebuff-auth`, or the package built from this flake";
      description = ''
        The Freebuff Auth package to install. It provides `freebuff-auth`
        and `freebuff-switch`.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];
  };
}
