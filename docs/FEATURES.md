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

- [x] Source SMB/CIFS vers NAS (Synology, QNAP, TrueNAS...) — inscriptible (copie vers le NAS)
- [x] Sources multiples: dossier local (NTFS monté), **USB** (détection des disques amovibles), SMB, HTTP, UPnP/DLNA, webradio
- [x] Source UPnP/DLNA (découverte SSDP, navigation ContentDirectory)
- [x] Webradios (m3u/m3u8) et URLs HTTP directes
- [x] Scan des sources, index sled, métadonnées + pochettes
- [x] Recherche plein texte, pages albums/artistes
- [x] Gestion des sources depuis l'UI façon Nova (cartes avec interrupteur, modale d'édition 2 colonnes, ajout par onglets local/USB/NAS/radio/HTTP) — ajout/édition (PUT)/suppression/activation, persistées dans `config.toml`
- [x] UI conforme à la charte graphique (thème sombre `#121212`, accent `#1ED760`, icônes SVG, recherche globale en barre supérieure)
- [x] **Explorateur d'arborescence** dans chaque source (grille ou liste, fil d'Ariane, lecture d'un dossier entier)
- [x] **Import/export**: copier fichiers/albums/dossiers d'une source à une autre (ex. NAS → clé USB), exporter en ZIP (fichier, dossier ou album)

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
