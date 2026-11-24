# Roadmap — Auris

Jalons GitHub (milestones) + board kanban : https://github.com/users/mjcc30/projects/2

## v0.1.0 — MVP (31/12/2026)

Binaire unique, lecture locale PCM, UI web embarquée style Spotify,
API REST v1 + WS, CLI, CI complète (fmt/clippy/tests/couverture/commits).
*Sortie : `cargo build --release` produit un binaire fonctionnel sur 5 cibles.*

## v0.2.0 — NAS & bibliothèque (31/01/2027)

Sources SMB/UPnP/HTTP, scan + index + recherche + pochettes,
gestion des sources dans l'UI.
*Sortie : lecture depuis un NAS Synology/QNAP sans configuration manuelle avancée.*

## v0.3.0 — Audio audiophile (28/02/2027)

DSD64–256 via DoP, gapless, sélection DAC, badges qualité.
*Sortie : DSD256 lu bit-perfect sur un DAC compatible DoP.*

## v0.4.0 — Plugins communautaires v1 (31/03/2027)

Manifeste + protocole JSON-RPC stdio, registre `auris-plugins`, UI de gestion,
SDK Rust + exemples Python/Go.
*Sortie : un plugin tiers installé et fonctionnel en < 5 min.*

## v1.0.0 — Stable (30/04/2027)

Mise à jour auto, install clé en main (install.sh RPi/systemd, brew, winget),
site vitrine + doc sur GitHub Pages (domaine auris.audio), token d'accès optionnel,
i18n fr/en, accessibilité AA auditée.
*Sortie : 1.0 publique, annonces, paquets officiels.*

## Au-delà (idées, non planifiées)

- v1.1 : DSD natif (ASIO Windows, ALSA DSD), redémarrage auto après update,
  plugins UI (iframe bac à sable), égaliseur via plugin, application mobile native.
- v1.2 : multiroom synchronisé, chromecast/airplay via plugins output.
