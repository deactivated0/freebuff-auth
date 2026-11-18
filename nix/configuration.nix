# System configuration for the machine installed by the Freebuff Auth
# installer. It is also usable on an existing NixOS system:
#
#   nixos-rebuild switch --flake /path/to/freebuff-auth#freebuff
#
# `hardware-configuration.nix` is generated on the target machine, so the
# import below is optional: without it the by-label fallbacks at the bottom
# are used, which match the layout created by `nix/install.sh`.
{ config, lib, pkgs, ... }:

{
  imports = [ ./module.nix ]
    ++ lib.optional (builtins.pathExists ../hardware-configuration.nix) ../hardware-configuration.nix;

  system.stateVersion = "26.05";

  # Flake-based system management.
  nix.settings.experimental-features = [
    "nix-command"
    "flakes"
  ];

  # UEFI boot. The installer creates a 1 GiB ESP mounted at /boot.
  boot.loader.systemd-boot.enable = true;
  boot.loader.efi.canTouchEfiVariables = true;

  networking.hostName = "freebuff";
  networking.networkmanager.enable = true;

  users.users.freebuff = {
    isNormalUser = true;
    description = "Freebuff user";
    extraGroups = [
      "networkmanager"
      "wheel"
    ];
    # Change this after the first login with `passwd`.
    initialPassword = "freebuff";
  };

  environment.systemPackages = with pkgs; [ git ];

  programs.freebuff-auth.enable = true;

  # Fallbacks for evaluation outside the installer. hardware-configuration.nix
  # overrides these with by-uuid devices once one is generated.
  fileSystems."/" = lib.mkDefault {
    device = "/dev/disk/by-label/nixos";
    fsType = "ext4";
  };

  fileSystems."/boot" = lib.mkDefault {
    device = "/dev/disk/by-label/boot";
    fsType = "vfat";
    options = [
      "fmask=0077"
      "dmask=0077"
    ];
  };
}
