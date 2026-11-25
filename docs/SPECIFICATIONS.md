# Spécifications techniques — Auris

Document de référence. Toute modification passe par une ADR (voir `docs/adr/`).

## 1. Vision

Lecteur audiophile autonome, un seul binaire Rust, pilotable depuis un navigateur,
qui lit la musique d'un NAS et la délivre bit-perfect à un DAC (PCM jusqu'à
32 bit/768 kHz, DSD jusqu'au DSD256).

## 2. Plateformes cibles

| Plateforme | Cible Rust | Version min | Build CI |
|---|---|---|---|
| Raspberry Pi 4/5 64 bits | `aarch64-unknown-linux-gnu` | Raspberry Pi OS 12 (Debian 12) | ubuntu + cross arm64 (libasound2-dev:arm64) |
| Linux x86_64 | `x86_64-unknown-linux-gnu` | Debian 12 / Ubuntu 22.04 | ubuntu-latest |
| macOS | `x86_64-apple-darwin`, `aarch64-apple-darwin` | macOS 13 Ventura | macos-14, macos-15 |
| Windows x86_64 | `x86_64-pc-windows-msvc` | Windows 10 1809+ | windows-latest |

Contrainte de build : **100 % Rust, aucune dépendance C** (pas de crate avec
`cc`/`ring`/`aws-lc`) pour que la cross-compilation fonctionne partout sans
toolchain C. TLS via `native-tls` (SChannel sur Windows, SecureTransport sur macOS,
OpenSSL système sur Linux — fourni par l'OS, pas compilé par nos soins).

## 3. Architecture logicielle

Monorepo Cargo workspace, un binaire `auris` :

```
ui/ (Vite+TS) --build--> ui/dist/ --rust-embed--> auris-web --embed--> binaire auris
crates/auris        CLI (clap) + wiring (config, démarrage, arrêt propre)
crates/auris-core   modèles, erreurs, événements
crates/auris-sources  backends: local, SMB2/3, HTTP(S) range, UPnP/DLNA
crates/auris-player   décodage + moteur de lecture + sortie audio (cpal)
crates/auris-library  index embarqué (sled), scan, recherche
crates/auris-web      API REST v1 + WebSocket + UI embarquée
crates/auris-updater  self-update GitHub (self_update)
```

Threads : moteur de commandes (1 thread) + worker de décodage par piste (1 thread)
+ horloge de position (1 thread) + callbacks cpal (temps réel) + runtime tokio
pour le web. Communication inter-threads : canaux std + `ringbuf` pour l'audio.

## 4. Pipeline audio (cœur du projet)

```
fichier (local/SMB/HTTP/UPnP) -> SeekRead -> décodeur -> ringbuf -> callback cpal -> DAC
                                     |                                    ^
                                     +-- PCM: symphonia -> f32 interleaved |
                                     +-- DSD: DSF/DFF -> DoP 24 bits -------+
```

- **PCM** : FLAC, WAV, AIFF, CAF, ALAC, MP3, AAC/M4A (ISOBMFF + ADTS), OGG Vorbis,
  WavPack (.wv, via `symphonia-codec-wavpack`, lossless ou lossy) — le tout via
  symphonia en premier. Sortie f32, aucun ré-échantillonnage (le flux est créé à
  la cadence native du fichier).
- **Repli ffmpeg** : si symphonia ne connait pas le format (Opus, WMA, APE, TTA,
  Musepack...) et qu'un binaire `ffmpeg` est disponible (env `AURIS_FFMPEG` ou
  PATH ; installé par `install.sh`), le fichier est piped depuis la source vers
  `ffprobe` (métadonnées/durée/spec) puis `ffmpeg -f f32le` (décodage). Seek par
  relance du processus avec `-ss` en sortie (un pipe n'est pas seekable). Si la
  durée manque (OGG via pipe), elle est mesurée par un décodage rapide complet.
- **DSD** : DSF et DFF parsés nativement (pas de DoP-in-the-box) ; DSD64/128/256
  encapsulés en **DoP 24 bits** (marqueurs 0x05/0xFA) et envoyés sur un flux cpal
  I24 (ou I32 décalé) à `dsd_rate/16`. Le DAC reçoit du DSD natif.
- **Gapless** : pistes de même spec enchaînées sans reconstruire le flux cpal.
- **Volume** : logiciel en f32 (0–100 %), appliqué dans le callback ; option
  bit-perfect (volume matériel uniquement) en v1.1.
- **Seek** : `SeekMode::Coarse` symphonia + reset décodeur ; les chunks de l'ancienne
  génération sont invalidés (compteur de génération dans le ringbuf).

## 5. Sources de musique

| Source | Protocole | Notes |
|---|---|---|
| Local | fs | walk récursif, chemins `src://<id>/...` |
| SMB/CIFS (NAS) | SMB2/3 pur Rust (`smb2`) | lectures par plages `read_at`, thread worker dédié |
| HTTP(S) | requêtes Range | webradios, playlists m3u/m3u8 |
| UPnP/DLNA | SSDP + SOAP ContentDirectory | découverte auto du LAN |

## 6. Bibliothèque

- Index embarqué **sled** dans `<data_dir>/library/` (pas de serveur externe).
- Scan incrémental des sources, probe des métadonnées (titre, artiste, album,
  piste, disque, année, genre, durée, pochette embarquée).
- Agrégation albums/artistes, recherche plein texte (titre/artiste/album/genre).
- Pochettes servies en JPEG/PNG (`/api/library/cover/{track_id}`).

## 7. Interface web & API

- UI embarquée dans le binaire (rust-embed) : aucun serveur de fichiers externe.
- **REST v1** (`/api/...`) + **WebSocket** `/ws` (état temps réel).
  Contrats complets : [API.md](API.md).
- Authentification : v1 = LAN de confiance (bind `0.0.0.0:3000` par défaut,
  restreignable via `--host`) ; v1.1 = token optionnel.
- CORS permissif en v1 (usage LAN), resserré quand l'auth arrivera.

## 8. Mise à jour automatique

- Releases GitHub `v*` du repo `mjcc30/auris`, assets `auris-<target>.tar.gz` (+ `.zip` Windows) + `checksums.txt` (SHA-256).
- `self_update` (backend GitHub, TLS native) : vérification toutes les 6 h en
  arrière-plan, application via `POST /api/system/update`, redémarrage manuel
  (v1.1 : redémarrage automatique via service systemd).

## 9. Plugins communautaires

Système façon Volumio mais ouvert et déclaratif : manifeste `auris-plugin.toml` +
processus externe parlant JSON-RPC 2.0 sur stdio. Spec complète :
[PLUGIN_SYSTEM.md](PLUGIN_SYSTEM.md).

## 10. Budgets de performance

| Métrique | Cible (Raspberry Pi 4) |
|---|---|
| Démarrage à froid → UI servie | < 2 s |
| TTI de l'interface web | < 1,5 s |
| Mémoire (RSS) au repos | < 150 Mo |
| Scan bibliothèque (local) | ≥ 20 fichiers/s |
| Latence WebSocket état | < 100 ms |

## 11. Stockage

```
<data_dir>/            (défaut: ~/.local/share/auris, /var/lib/auris en service)
├── config.toml        sources, DAC, volume, préférences (permissions 600)
├── library/           base sled
├── plugins/           plugins installés
└── cache/             pochettes, jaquettes
```

## 12. Qualité & conformité

- **TDD** : tests avec le code, `cargo test` vert obligatoire (voir CONTRIBUTING.md).
- **Couverture** : ≥ 60 % sur les crates non-UI (cargo-llvm-cov, rapport en CI).
- **Commits conventionnels** vérifiés par cocogitto en CI.
- **Accessibilité** : WCAG 2.1 AA (contrastes, focus clavier, aria).
- **i18n** : fr + en (v1), fichiers de traduction dans `ui/src/i18n/`.
- **Licence** : AGPL v3 pour tout le code ; plugins sous licence compatible AGPL.

## 13. Hors scope v1 (non-goals)

- Multiroom synchronisé, services DRM (Spotify Connect, Tidal Connect...) —
  délégués aux plugins communautaires.
- Applications mobiles natives (l'UI web responsive suffit).
- Sortie DSD native (sans DoP) via ASIO Windows / ALSA `DSD` — piste v1.1.
- Égaliseur / DSP — via plugins.
