{ lib, rustPlatform }:

rustPlatform.buildRustPackage {
  pname = "freebuff-auth";
  version = (lib.importTOML ../Cargo.toml).package.version;

  src = ../.;

  cargoLock.lockFile = ../Cargo.lock;

  # Both `freebuff-auth` and `freebuff-switch` are [[bin]] targets in
  # Cargo.toml, so cargoInstallHook installs both.

  meta = {
    description = "Manage multiple Freebuff CLI accounts with a terminal switcher";
    homepage = "https://github.com/deactivated0/freebuff-auth";
    license = lib.licenses.mit;
    mainProgram = "freebuff-auth";
    platforms = lib.platforms.unix;
  };
}
