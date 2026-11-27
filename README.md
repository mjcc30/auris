# Auris

Lecteur audiophile cross-plateforme en **un seul binaire Rust** — l'esprit Volumio, en natif.

- Musique hébergée sur un **NAS** (SMB/CIFS, UPnP/DLNA, HTTP) ou en local
- Sortie **bit-perfect** vers un DAC USB, du PCM jusqu'au **DSD256** (via DoP)
- Pilotable depuis n'importe quel navigateur (téléphone, tablette, PC) avec une interface **style Spotify**
- **Mise à jour automatique** depuis les releases GitHub
- Clé en main : **Raspberry Pi**, Linux, macOS, Windows — un seul fichier à lancer

[![CI](https://github.com/mjcc30/auris/actions/workflows/ci.yml/badge.svg)](https://github.com/mjcc30/auris/actions/workflows/ci.yml)
[![License: AGPL v3](https://img.shields.io/badge/License-AGPL%20v3-blue.svg)](https://www.gnu.org/licenses/agpl-3.0.html)
[![Release](https://img.shields.io/github/v/release/mjcc30/auris)](https://github.com/mjcc30/auris/releases)

> Licence **AGPL v3** : le code est libre, et toute version modifiée distribuée doit
> remettre son code source à disposition. Voir [MANIFESTO.md](MANIFESTO.md).

## Démarrage rapide

```bash
# Raspberry Pi / Linux / macOS — installation en une ligne :
curl -fsSL https://raw.githubusercontent.com/mjcc30/auris/main/install.sh | sudo bash

# puis ouvrir depuis n'importe quel appareil du réseau :
http://<ip-de-la-machine>:3000
```

Autres méthodes : paquet `.deb` / `.rpm`, `brew install mjcc30/auris`, `winget install mjcc30.Auris`,
ou image disque prête à flasher (PC x86_64 / Raspberry Pi) — voir [docs/install.md](docs/install.md).

Depuis les sources :

```bash
git clone https://github.com/mjcc30/auris.git && cd auris
cargo run -- serve --port 3000
```

Windows : téléchargez `auris-x86_64-pc-windows-msvc.zip` dans les [releases](https://github.com/mjcc30/auris/releases), dézippez, lancez `auris.exe serve`.

## Plateformes

| Plateforme | Cible | Statut |
|---|---|---|
| Raspberry Pi 4/5 (64 bits) | `aarch64-unknown-linux-gnu` | ✅ CI |
| Linux x86_64 | `x86_64-unknown-linux-gnu` | ✅ CI |
| macOS Intel / Apple Silicon | `x86_64-apple-darwin`, `aarch64-apple-darwin` | ✅ CI |
| Windows x86_64 | `x86_64-pc-windows-msvc` | ✅ CI |

## Architecture

```
crates/
├── auris/          binaire final (CLI : serve, update, audio-devices, version)
├── auris-core/     modèles + erreurs
├── auris-sources/  local, SMB (NAS), HTTP, UPnP/DLNA
├── auris-player/   décodage PCM (symphonia) + DSD (DSF/DFF → DoP), sortie cpal
├── auris-library/  index sled, scan, recherche, pochettes
├── auris-web/      API REST + WebSocket + UI embarquée (rust-embed)
└── auris-updater/  self-update depuis les releases GitHub
ui/                 interface web (Vite + TypeScript), compilée puis embarquée dans le binaire
docs/               spécifications, charte graphique, API, système de plugins, site vitrine
```

Un seul binaire = zéro dépendance externe à installer (hors bibliothèque système audio :
`libasound2` sur Linux, présente par défaut sur Raspberry Pi OS).

## Documentation

- [Spécifications techniques](docs/SPECIFICATIONS.md)
- [Fonctionnalités (MoSCoW)](docs/FEATURES.md) et [user stories](docs/USER_STORIES.md)
- [Charte graphique](docs/CHARTE_GRAPHIQUE.md)
- [API REST v1 + WebSocket](docs/API.md)
- [Système de plugins communautaires](docs/PLUGIN_SYSTEM.md)
- [Roadmap & jalons](docs/ROADMAP.md)
- [Kanban (board projet)](docs/BOARD.md)
- [Manifeste](MANIFESTO.md)

## Contribuer

Voir [CONTRIBUTING.md](CONTRIBUTING.md) : commits conventionnels (vérifiés en CI par
cocogitto), TDD (tests exigés avec le code), couverture minimale, revues de PR.
Le board kanban et les labels sont décrits dans [docs/BOARD.md](docs/BOARD.md).

Code of conduct : [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).
Signaler une faille : [SECURITY.md](SECURITY.md) (divulgation privée, pas d'issue publique).

## Licence

[AGPL v3](LICENSE) — © 2026 contributeurs Auris.
