#!/usr/bin/env bash
# Install NixOS from the Freebuff Auth flake onto one disk.
#
#   sudo freebuff-install /dev/nvme0n1
#
# The disk is erased. Layout: 1 GiB FAT32 ESP at /boot, ext4 root at /.
# Everything else (kernel, users, program list) comes from the flake, so the
# installed system is reproducible with `nixos-rebuild switch --flake
# /etc/nixos#freebuff`.
set -euo pipefail

REPO_COPY=/etc/freebuff-auth
CONFIG_NAME=freebuff

log() { printf '\n==> %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

case "${1:-}" in
  -h | --help)
    cat <<'EOF'
usage: freebuff-install DISK

Erases DISK and installs NixOS with the Freebuff Auth flake.

  DISK   whole disk, e.g. /dev/nvme0n1, /dev/sda, or /dev/disk/by-id/...

The installed hostname, users and packages come from nix/configuration.nix.
EOF
    exit 0
    ;;
esac

[[ ${EUID} -eq 0 ]] || die "run as root: sudo freebuff-install /dev/sdX"
[[ -n "${1:-}" ]] || die "usage: sudo freebuff-install /dev/sdX"
[[ -b "$1" ]] || die "$1 is not a block device"
disk=$(readlink -f "$1")
[[ -b "${disk}" ]] || die "$1 is not a block device"
[[ -d "${REPO_COPY}" ]] || die "${REPO_COPY} is missing; boot the Freebuff Auth installer ISO"

# Do not wipe the medium we booted from.
if lsblk -no MOUNTPOINTS "${disk}" 2>/dev/null | grep -q .; then
  die "${disk} has mounted partitions (is it the install medium?)"
fi
if mountpoint -q /mnt; then
  die "/mnt is already mounted; unmount it first"
fi

# nvme0n1 -> nvme0n1p1, sda -> sda1
case "${disk}" in
  *[0-9]) sep=p ;;
  *) sep= ;;
esac
esp="${disk}${sep}1"
root="${disk}${sep}2"

printf '\nThis will ERASE %s and install NixOS on it.\n' "${disk}"
printf 'Everything on that disk is lost.\n\n'
read -r -p 'Type ERASE to continue: ' confirm
[[ "${confirm}" == "ERASE" ]] || die "aborted"

log "Partitioning ${disk}"
wipefs -a "${disk}"
sgdisk --zap-all "${disk}"
sgdisk --new=1:0:+1G --typecode=1:ef00 --change-name=1:EFI "${disk}"
sgdisk --new=2:0:0 --typecode=2:8300 --change-name=2:nixos "${disk}"
partprobe "${disk}"
udevadm settle

log "Creating filesystems"
mkfs.fat -F 32 -n boot "${esp}"
mkfs.ext4 -L nixos "${root}"

log "Mounting at /mnt"
mount "${root}" /mnt
mkdir -p /mnt/boot
mount "${esp}" /mnt/boot

log "Generating hardware configuration"
nixos-generate-config --root /mnt
# The flake replaces this file.
rm -f /mnt/etc/nixos/configuration.nix

log "Copying the flake to /mnt/etc/nixos"
cp -rL "${REPO_COPY}/." /mnt/etc/nixos/
chmod -R u+w /mnt/etc/nixos
rm -rf /mnt/etc/nixos/.git

log "Installing NixOS from /mnt/etc/nixos#${CONFIG_NAME}"
nixos-install --flake "/mnt/etc/nixos#${CONFIG_NAME}" --no-root-passwd

cat <<EOF

Installation finished.

  Reboot, then log in as:

    user:     freebuff
    password: freebuff
               (change it with 'passwd')

  The system configuration lives in /etc/nixos and is managed with:

    sudo nixos-rebuild switch --flake /etc/nixos#${CONFIG_NAME}

  Install the official Freebuff CLI separately; Freebuff Auth only manages
  profiles for it.

EOF
