#!/usr/bin/env bash
# Auris — installation clé en main (Raspberry Pi OS / Debian / Ubuntu, aarch64 ou x86_64).
# Usage: curl -fsSL https://raw.githubusercontent.com/mjcc30/auris/main/install.sh | sudo bash
set -euo pipefail

REPO="mjcc30/auris"
BIN_NAME="auris"
INSTALL_DIR="/usr/local/bin"
USER_NAME="auris"
DATA_DIR="/var/lib/auris"
SERVICE_FILE="/etc/systemd/system/auris.service"
PORT="${AURIS_PORT:-3000}"

log() { printf '\033[1;32m[auris]\033[0m %s\n' "$*"; }
die() { printf '\033[1;31m[auris] ERREUR:\033[0m %s\n' "$*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || die "Ce script doit être exécuté en root (sudo)."

# --- Détection de l'architecture -------------------------------------------
arch="$(uname -m)"
case "$arch" in
  aarch64|arm64) asset_arch="aarch64-unknown-linux-gnu" ;;
  x86_64|amd64)  asset_arch="x86_64-unknown-linux-gnu" ;;
  *) die "Architecture non supportée: $arch (attendu: aarch64 ou x86_64)." ;;
esac

# --- Dernière release GitHub -------------------------------------------------
log "Recherche de la dernière release de $REPO…"
api="https://api.github.com/repos/$REPO/releases/latest"
release_json="$(curl -fsSL -H "User-Agent: auris-install" "$api")" || die "Impossible de contacter l'API GitHub."
version="$(printf '%s' "$release_json" | grep -o '"tag_name":[[:space:]]*"[^"]*"' | head -1 | cut -d'"' -f4)"
[ -n "$version" ] || die "Version introuvable dans la release."
asset="$BIN_NAME-$asset_arch"
url="https://github.com/$REPO/releases/download/$version/$asset"
log "Téléchargement de $url (version $version)…"

tmp="$(mktemp)"
curl -fsSL -H "User-Agent: auris-install" -o "$tmp" "$url" || die "Téléchargement de $asset échoué (release $version)."
chmod 755 "$tmp"

# --- Installation du binaire -------------------------------------------------
log "Installation dans $INSTALL_DIR/$BIN_NAME…"
install -m 755 "$tmp" "$INSTALL_DIR/$BIN_NAME"
rm -f "$tmp"
"$INSTALL_DIR/$BIN_NAME" version

# --- ffmpeg (repli de décodage pour Opus/WMA/APE/TTA...) -----------------------
if ! command -v ffmpeg >/dev/null 2>&1; then
  log "Installation de ffmpeg (repli pour les formats hors symphonia)…"
  apt-get update -qq && apt-get install -y -qq ffmpeg
fi

# --- Utilisateur dédié + répertoires ------------------------------------------
if ! id "$USER_NAME" >/dev/null 2>&1; then
  log "Création de l'utilisateur système $USER_NAME…"
  useradd --system --home "$DATA_DIR" --shell /usr/sbin/nologin "$USER_NAME"
fi
mkdir -p "$DATA_DIR"
chown -R "$USER_NAME:$USER_NAME" "$DATA_DIR"

# --- Configuration par défaut -------------------------------------------------
# La config se lit dans <data-dir>/config.toml (voir crates/auris/src/config.rs).
if [ ! -f "$DATA_DIR/config.toml" ]; then
  log "Écriture de $DATA_DIR/config.toml…"
  cat > "$DATA_DIR/config.toml" <<EOF
# Configuration Auris — voir https://auris.audio/docs/SPECIFICATIONS
host = "0.0.0.0"
port = $PORT
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
EOF
  chown "$USER_NAME:$USER_NAME" "$DATA_DIR/config.toml"
  chmod 640 "$DATA_DIR/config.toml"
fi

# --- Service systemd -----------------------------------------------------------
log "Installation du service systemd…"
cat > "$SERVICE_FILE" <<EOF
[Unit]
Description=Auris — lecteur audiophile (style Volumio)
Documentation=https://auris.audio
After=network-online.target sound.target
Wants=network-online.target

[Service]
Type=simple
User=$USER_NAME
Group=$USER_NAME
ExecStart=$INSTALL_DIR/$BIN_NAME serve --data-dir $DATA_DIR
Environment=AURIS_PORT=$PORT
Restart=on-failure
RestartSec=5
# Le binaire met à jour automatiquement les sources NAS au démarrage.
AmbientCapabilities=CAP_SYS_NICE
Nice=-10

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable auris.service
systemctl restart auris.service
sleep 2
systemctl --no-pager --quiet is-active auris.service || die "Le service auris ne démarre pas (voir: journalctl -u auris)."

ip="$(hostname -I 2>/dev/null | awk '{print $1}')"
log "Installation terminée (version $version)."
log "Interface web: http://${ip:-localhost}:$PORT/"
log "Commandes utiles: journalctl -u auris -f | systemctl restart auris | $INSTALL_DIR/$BIN_NAME --help"
