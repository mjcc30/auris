#!/usr/bin/env bash
# build-image.sh — construit une image disque prête à flasher pour Auris.
#
#   sudo ./packaging/image/build-image.sh pc  v0.8.0 [sortie.img]   # PC x86_64 (Debian stable, BIOS/UEFI)
#   sudo ./packaging/image/build-image.sh rpi v0.8.0 [sortie.img]   # Raspberry Pi 4/5 (Raspberry Pi OS Lite arm64)
#
# L'image embarque le binaire Auris (release GitHub), le service systemd activé,
# et démarre directement sur l'interface web http://<ip>:3000/ (ssh actif sur rpi).
#
# Prérequis (hôte Linux x86_64, root) :
#   pc : debootstrap parted dosfstools xz-utils
#   rpi: kpartx parted xz-utils dpkg-dev curl ca-certificates   (pas besoin de qemu)
set -euo pipefail

log()  { printf '\033[1;32m[image]\033[0m %s\n' "$*"; }
die()  { printf '\033[1;31m[image] ERREUR:\033[0m %s\n' "$*" >&2; exit 1; }

TYPE="${1:-}"; VERSION="${2:-}"; OUT="${3:-}"
[ -n "$TYPE" ] && [ -n "$VERSION" ] || die "usage: $0 <pc|rpi> <version|latest> [sortie.img]"
REPO="mjcc30/auris"
OUT="${OUT:-auris-$TYPE-$VERSION.img}"
WORK="$(mktemp -d /tmp/auris-image.XXXXXX)"
trap 'umount_quiet' EXIT

umount_quiet() {
  for d in "$WORK/mnt" "$WORK/mnt/boot" "$WORK/mnt/boot/firmware"; do
    mountpoint -q "$d" 2>/dev/null && umount -R "$d" 2>/dev/null || true
  done
  if [ -n "${LOOP:-}" ] && losetup "$LOOP" >/dev/null 2>&1; then
    kpartx -d "$LOOP" 2>/dev/null || true
    losetup -d "$LOOP" 2>/dev/null || true
  fi
  rm -rf "$WORK"
}

resolve_version() {
  if [ "$VERSION" = "latest" ]; then
    VERSION="$(curl -fsSL -H 'User-Agent: auris-image' \
      "https://api.github.com/repos/$REPO/releases/latest" \
      | grep -o '"tag_name":[[:space:]]*"[^"]*"' | head -1 | cut -d'"' -f4)"
    [ -n "$VERSION" ] || die "version introuvable"
  fi
  log "release: $VERSION"
}

download_deb() { # $1=arch (amd64|arm64) -> chemin local
  local arch="$1"
  local f="$WORK/auris_${VERSION#v}-1_$arch.deb"
  curl -fsSL -H 'User-Agent: auris-image' -o "$f" \
    "https://github.com/$REPO/releases/download/$VERSION/auris_${VERSION#v}-1_$arch.deb" \
    || die "téléchargement du .deb $arch échoué (release $VERSION)"
  echo "$f"
}

write_default_config() { # $1=chroot prefix
  local root="$1/var/lib/auris"
  mkdir -p "$root"
  if [ ! -f "$root/config.toml" ]; then
    cat > "$root/config.toml" <<'CFG'
host = "0.0.0.0"
port = 3000
volume = 80
auto_update = true
repo_owner = "mjcc30"
repo_name = "auris"

[[sources]]
id = "nas"
name = "NAS"
enabled = true
kind = "local"
path = "/mnt/nas/musique"
CFG
  fi
  chown -R 999:999 "$root" 2>/dev/null || chown -R auris:auris "$root" 2>/dev/null || true
  chmod 640 "$root/config.toml" 2>/dev/null || true
}

# ---------------------------------------------------------------------------
# PC x86_64 : Debian stable (bookworm) minimale + Auris, bootable BIOS/UEFI.
# ---------------------------------------------------------------------------
build_pc() {
  command -v debootstrap >/dev/null || die "debootstrap requis (apt install debootstrap)"
  command -v parted >/dev/null || die "parted requis"
  local size="${IMAGE_SIZE:-3G}"
  local img="$WORK/disk.img"
  log "création de l'image $TYPE ($size) -> $OUT"
  truncate -s "$size" "$img"
  parted -s "$img" mklabel msdos
  parted -s "$img" mkpart primary ext4 1MiB 100%
  LOOP="$(losetup --find --show --partscan "$img")"
  mkfs.ext4 -q -L AURIS "${LOOP}p1"
  mkdir -p "$WORK/mnt"
  mount "${LOOP}p1" "$WORK/mnt"

  log "debootstrap bookworm amd64…"
  debootstrap --arch=amd64 --variant=minbase bookworm "$WORK/mnt" http://deb.debian.org/debian

  mount --bind /dev "$WORK/mnt/dev"
  mount --bind /proc "$WORK/mnt/proc"
  mount --bind /sys "$WORK/mnt/sys"

  local deb; deb="$(download_deb amd64)"
  cp "$deb" "$WORK/mnt/tmp/auris.deb"
  chroot "$WORK/mnt" /bin/bash -e <<'CHROOT'
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends \
      ca-certificates libasound2 ffmpeg alsa-utils \
      grub-pc-bin grub2-common linux-image-amd64 openssh-server \
      locales sudo curl
    dpkg -i /tmp/auris.deb || apt-get -f install -y -qq
    rm -f /tmp/auris.deb
    useradd --system --home /var/lib/auris --shell /usr/sbin/nologin auris 2>/dev/null || true
    systemctl enable auris.service
    # SSH actif pour l'admin ; root login désactivé (mot de passe par défaut Pi-style non requis ici).
    systemctl enable ssh
    # Locale UTF-8
    echo "fr_FR.UTF-8 UTF-8" > /etc/locale.gen
    locale-gen fr_FR.UTF-8 >/dev/null
    # Redimensionnement auto de la racine au premier boot
    cat > /usr/local/sbin/auris-resizefs <<'EOF'
#!/bin/sh
# Agrandit la partition racine si le disque est plus grand que l'image.
rootdev="$(findmnt -n -o SOURCE /)"
rootpart="${rootdev%[0-9]*}"; rootnum="${rootdev##*[!0-9]}"
if [ -b "$rootpart" ] && command -v growpart >/dev/null 2>&1; then
  growpart "$rootpart" "$rootnum" 2>/dev/null || true
  resize2fs "$rootdev" 2>/dev/null || true
fi
systemctl disable auris-resizefs.service >/dev/null 2>&1 || true
EOF
    chmod 755 /usr/local/sbin/auris-resizefs
    cat > /etc/systemd/system/auris-resizefs.service <<'EOF'
[Unit]
Description=Redimensionne la racine au premier boot
Before=local-fs.target
[Service]
Type=oneshot
ExecStart=/usr/local/sbin/auris-resizefs
[Install]
WantedBy=multi-user.target
EOF
    apt-get install -y -qq cloud-guest-utils 2>/dev/null || true
    systemctl enable auris-resizefs.service
    # Grub sur le disque (BIOS legacy ; l'UEFI fallback dépend du firmware)
    echo "auris-image" > /etc/hostname
    printf 'root=LABEL=AURIS console=tty1\n' > /etc/default/grub.d/99-auris.cfg
    update-grub >/dev/null 2>&1 || true
CHROOT
  # Installe grub sur la boucle (nécessite le device loop)
  grub-install --target=i386-pc --boot-directory="$WORK/mnt/boot" "$LOOP" >/dev/null 2>&1 \
    || log "ATTENTION: grub-install a échoué (image démarrera peut-être en UEFI seul)"

  write_default_config "$WORK/mnt"

  umount "$WORK/mnt/dev" "$WORK/mnt/proc" "$WORK/mnt/sys"
  umount "$WORK/mnt"
  kpartx -d "$LOOP"; losetup -d "$LOOP"; LOOP=""
  mv "$img" "$OUT"
}

# ---------------------------------------------------------------------------
# Raspberry Pi : Raspberry Pi OS Lite (arm64) + .deb arm64 d'Auris + ssh actif.
# Pas de qemu : on extrait le .deb avec dpkg-deb -x (pas de postinst) et on crée
# l'utilisateur système + le symlink d'activation du service à la main. Le binaire
# arm64 s'exécutera nativement sur le Pi. (qemu-user-static n'existe plus dans les
# dépôts Ubuntu >= 24.04.)
# ---------------------------------------------------------------------------
build_rpi() {
  command -v kpartx >/dev/null || die "kpartx requis (apt install kpartx)"
  command -v dpkg-deb >/dev/null || die "dpkg-deb requis (apt install dpkg-dev)"
  local base="2025-05-06-raspios-bookworm-arm64-lite"
  local src_url="https://downloads.raspberrypi.com/raspios_lite_arm64/images/raspios_lite_arm64-2025-05-06/2025-05-06-raspios-bookworm-arm64-lite.img.xz"
  local img_xz="$WORK/raspios.img.xz"
  log "téléchargement de Raspberry Pi OS Lite arm64 ($base)…"
  curl -fsSL -H 'User-Agent: auris-image' -o "$img_xz" "$src_url" || die "téléchargement RPi OS échoué"
  xz -dk -T0 "$img_xz"
  local img="${img_xz%.xz}"

  LOOP="$(losetup --find --show --partscan "$img")"
  kpartx -a "$LOOP"
  local map; map="/dev/mapper/$(basename "$LOOP")"
  mkdir -p "$WORK/mnt"
  mount "${map}p2" "$WORK/mnt"          # partition racine
  mkdir -p "$WORK/mnt/boot/firmware"
  mount "${map}p1" "$WORK/mnt/boot/firmware"  # partition boot FAT

  local deb; deb="$(download_deb arm64)"
  log "extraction du .deb arm64 dans l'image (dpkg-deb -x, sans postinst)…"
  dpkg-deb -x "$deb" "$WORK/mnt"

  # Utilisateur système auris (uid/gid 999, comme sur Debian/RPi OS)
  grep -q '^auris:' "$WORK/mnt/etc/passwd" || echo 'auris:x:999:999:Auris:/var/lib/auris:/usr/sbin/nologin' >> "$WORK/mnt/etc/passwd"
  grep -q '^auris:' "$WORK/mnt/etc/group" || echo 'auris:x:999:' >> "$WORK/mnt/etc/group"
  grep -q '^auris:' "$WORK/mnt/etc/shadow" || echo 'auris:!:19000:0:99999:7:::' >> "$WORK/mnt/etc/shadow"
  chown 999:999 "$WORK/mnt/var/lib/auris"

  # Active auris.service et ssh au boot (équivalent systemctl enable, sans chroot arm64)
  mkdir -p "$WORK/mnt/etc/systemd/system/multi-user.target.wants"
  ln -sf /etc/systemd/system/auris.service "$WORK/mnt/etc/systemd/system/multi-user.target.wants/auris.service"
  ln -sf /lib/systemd/system/ssh.service "$WORK/mnt/etc/systemd/system/multi-user.target.wants/ssh.service" 2>/dev/null || true
  touch "$WORK/mnt/boot/firmware/ssh"   # ssh actif au premier boot

  write_default_config "$WORK/mnt"

  umount "$WORK/mnt/boot/firmware" "$WORK/mnt"
  kpartx -d "$LOOP"; losetup -d "$LOOP"; LOOP=""
  mv "$img" "$OUT"
}

resolve_version
case "$TYPE" in
  pc)  build_pc ;;
  rpi) build_rpi ;;
  *) die "type inconnu: $TYPE (attendu: pc ou rpi)" ;;
esac

log "image prête: $OUT"
sha256sum "$OUT" > "$OUT.sha256"
log "sha256: $(cat "$OUT.sha256")"
log "flasher avec: xz -dkc $OUT.xz | sudo dd of=/dev/sdX bs=4M status=progress conv=fsync  (ou Raspberry Pi Imager)"
