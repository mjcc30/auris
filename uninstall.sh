#!/usr/bin/env bash
# Auris — désinstallation (Raspberry Pi OS / Debian / Ubuntu).
# Usage: curl -fsSL https://raw.githubusercontent.com/mjcc30/auris/main/uninstall.sh | sudo bash
#       sudo bash uninstall.sh --purge   # supprime aussi /var/lib/auris (config, index, favoris)
set -euo pipefail

BIN_NAME="auris"
INSTALL_DIR="/usr/local/bin"
USER_NAME="auris"
DATA_DIR="/var/lib/auris"
SERVICE_FILE="/etc/systemd/system/auris.service"

log() { printf '\033[1;32m[auris]\033[0m %s\n' "$*"; }
die() { printf '\033[1;31m[auris] ERREUR:\033[0m %s\n' "$*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || die "Ce script doit être exécuté en root (sudo)."

PURGE=0
[ "${1:-}" = "--purge" ] && PURGE=1

if systemctl is-active --quiet auris.service 2>/dev/null; then
  log "Arrêt du service auris…"
  systemctl stop auris.service || true
fi
if systemctl is-enabled --quiet auris.service 2>/dev/null; then
  log "Désactivation du service…"
  systemctl disable auris.service || true
fi
if [ -f "$SERVICE_FILE" ]; then
  rm -f "$SERVICE_FILE"
  systemctl daemon-reload
  log "Service supprimé."
fi
if [ -f "$INSTALL_DIR/$BIN_NAME" ]; then
  rm -f "$INSTALL_DIR/$BIN_NAME"
  log "Binaire supprimé."
fi
if id "$USER_NAME" >/dev/null 2>&1; then
  if [ "$PURGE" -eq 1 ]; then
    userdel "$USER_NAME" 2>/dev/null || true
    rm -rf "$DATA_DIR"
    log "Utilisateur et données ($DATA_DIR) supprimés (--purge)."
  else
    log "Utilisateur $USER_NAME conservé ; données conservées dans $DATA_DIR (réinstallez pour les réutiliser, ou --purge pour tout supprimer)."
  fi
fi
log "Désinstallation terminée."
