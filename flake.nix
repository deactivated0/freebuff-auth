{
  description = "Freebuff Auth — Freebuff CLI account switcher, with a NixOS module and a flake-based NixOS installer";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs =
    { self, nixpkgs }:
    let
      lib = nixpkgs.lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = lib.genAttrs systems;
      mkApp = program: {
        type = "app";
        inherit program;
      };
    in
    {
      overlays.default = final: _prev: {
        freebuff-auth = final.callPackage ./nix/package.nix { };
      };

      packages = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          freebuff-auth = pkgs.callPackage ./nix/package.nix { };
          default = self.packages.${system}.freebuff-auth;
        }
        // lib.optionalAttrs (system == "x86_64-linux") {
          installer-iso = self.nixosConfigurations.installer.config.system.build.isoImage;
          installer = self.packages.${system}.installer-iso;
        }
      );

      apps = forAllSystems (system: {
        default = mkApp "${self.packages.${system}.freebuff-auth}/bin/freebuff-auth";
        freebuff-auth = mkApp "${self.packages.${system}.freebuff-auth}/bin/freebuff-auth";
        freebuff-switch = mkApp "${self.packages.${system}.freebuff-auth}/bin/freebuff-switch";
      });

      nixosModules = {
        default = import ./nix/module.nix;
        freebuff-auth = self.nixosModules.default;
      };

      nixosConfigurations = {
        # Bootable ISO: builds the system in nix/installer.nix.
        installer = nixpkgs.lib.nixosSystem {
          system = "x86_64-linux";
          modules = [ ./nix/installer.nix ];
        };

        # The installed system. `nixos-install --flake ...#freebuff` and later
        # `nixos-rebuild switch --flake /etc/nixos#freebuff` both use this.
        freebuff = nixpkgs.lib.nixosSystem {
          system = "x86_64-linux";
          modules = [ ./nix/configuration.nix ];
        };
      };
    };
}
