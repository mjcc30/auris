# Installer Auris

## Raspberry Pi (clé en main)

```bash
curl -fsSL https://raw.githubusercontent.com/mjcc30/auris/main/install.sh | sudo bash
# puis : http://raspberrypi.local:3000
```

Le script installe le binaire `aarch64` dans `/usr/local/bin/auris`, crée
l'utilisateur `auris`, le data-dir `/var/lib/auris` et l'unité systemd
`auris.service` (démarrage auto au boot).

## Linux / macOS / Windows

Téléchargez l'asset de votre plateforme dans les
[releases](https://github.com/mjcc30/auris/releases) :

- `auris-x86_64-unknown-linux-gnu.tar.gz`
- `auris-aarch64-unknown-linux-gnu.tar.gz` (Raspberry Pi 4/5 64 bits)
- `auris-x86_64-apple-darwin.tar.gz` / `auris-aarch64-apple-darwin.tar.gz`
- `auris-x86_64-pc-windows-msvc.zip`

Vérifiez l'intégrité : `sha256sum -c checksums.txt`.

```bash
tar -xzf auris-x86_64-unknown-linux-gnu.tar.gz
./auris serve --port 3000
```

## Depuis les sources

```bash
git clone https://github.com/mjcc30/auris.git && cd auris
cargo build --release
./target/release/auris serve
```

## Configuration

Fichier `<data_dir>/config.toml` (créé au premier lancement) :

```toml
port = 3000
host = "0.0.0.0"
audio_device = "Mon DAC USB"   # optionnel, défaut = périphérique par défaut
volume = 80
auto_update = true

[[sources]]
name = "NAS"
kind = "smb"
host = "192.168.1.10"
share = "music"
user = "..."
password = "..."   # fichier en permissions 600, ne jamais commiter
```

Variables d'environnement : `AURIS_PORT`, `AURIS_HOST`, `AURIS_DATA_DIR`,
`AURIS_DEVICE`, `AURIS_VOLUME`, `AURIS_AUTO_UPDATE`.

## Commandes CLI

```bash
auris serve [--port 3000] [--host 0.0.0.0] [--data-dir ...] [--device "DAC"]
auris audio-devices     # liste les DAC détectés
auris update            # vérifie + applique une mise à jour
auris version
```
