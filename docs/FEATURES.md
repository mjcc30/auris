# Fonctionnalités — Auris (MoSCoW)

Priorités par jalon (voir [ROADMAP.md](ROADMAP.md)). Chaque fonctionnalité = un ticket
kanban avec critères d'acceptation (voir [USER_STORIES.md](USER_STORIES.md)).

## Must have — v0.1.0 (MVP)

- [ ] Binaire unique cross-plateforme (Linux x86_64/arm64, macOS, Windows)
- [ ] Lecture fichiers locaux (FLAC, WAV, MP3, AAC, OGG...) sans ré-échantillonnage
- [ ] Sortie DAC USB via cpal (ALSA / CoreAudio / WASAPI)
- [ ] Interface web embarquée, style Spotify (sombre), responsive
- [ ] API REST v1 + WebSocket état temps réel
- [ ] File d'attente, lecture/pause/stop, suivant/précédent, seek, volume, muet,
      répétition (off/all/one), aléatoire
- [ ] Sélection du périphérique de sortie (DAC) depuis l'UI
- [ ] CLI : `serve`, `version`, `audio-devices`, `update`
- [ ] CI : fmt, clippy, tests, couverture, commits conventionnels

## Should have — v0.2.0 (NAS & bibliothèque)

- [ ] Source SMB/CIFS vers NAS (Synology, QNAP, TrueNAS...)
- [ ] Source UPnP/DLNA (découverte SSDP, navigation ContentDirectory)
- [ ] Webradios (m3u/m3u8) et URLs HTTP directes
- [ ] Scan des sources, index sled, métadonnées + pochettes
- [ ] Recherche plein texte, pages albums/artistes
- [ ] Gestion des sources depuis l'UI (ajout/suppression)

## Must have — v0.3.0 (audio audiophile)

- [ ] DSD64/128/256 : lecture DSF/DFF → DoP 24 bits vers le DAC (jusqu'au DSD256)
- [ ] Gapless entre pistes de même format
- [ ] Position/durée fiables, seek précis (coarse)
- [ ] Badge qualité (PCM xx bit/xx kHz, DSDxx) dans l'UI

## Could have — v0.4.0 (plugins)

- [ ] Système de plugins communautaires v1 (manifest + processus JSON-RPC stdio)
- [ ] Registre de plugins (index communautaire sur GitHub)
- [ ] UI de gestion : installer/activer/désactiver/configurer
- [ ] Plugins de type source, sortie, métadonnées

## Should have — v1.0.0 (stable)

- [ ] Mise à jour automatique (check 6 h + bouton + checksums SHA-256)
- [ ] Install clé en main : `install.sh` Raspberry Pi + unité systemd,
      paquets macOS (brew) et Windows (winget) — dépôts publics
- [ ] Site vitrine + documentation (GitHub Pages, domaine auris.audio)
- [ ] Authentification par token (optionnelle) sur l'API
- [ ] i18n fr/en complet

## Won't have (v1)

- Multiroom, comptes cloud, DRM, applications natives iOS/Android.
