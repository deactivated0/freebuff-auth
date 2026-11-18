# Live ISO that installs NixOS from this flake.
#
#   nix build .#installer-iso
#   sudo dd if=result/iso/*.iso of=/dev/sdX bs=4M status=progress oflag=sync
#
# The ISO carries a copy of the whole flake at /etc/freebuff-auth and the
# `freebuff-install` script, so it can install without cloning anything.
{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}:

{
  imports = [ "${modulesPath}/installer/cd-dvd/installation-cd-minimal.nix" ];

  networking.hostName = "freebuff-installer";

  nix.settings.experimental-features = [
    "nix-command"
    "flakes"
  ];

  # The flake source, read by freebuff-install.
  environment.etc."freebuff-auth".source = ../.;

  environment.systemPackages = with pkgs; [
    dosfstools
    e2fsprogs
    gptfdisk
    parted
    util-linux
    (pkgs.writeShellScriptBin "freebuff-install" (builtins.readFile ./install.sh))
  ];

  isoImage.volumeID = "FREEBUFF_INSTALLER";
}
