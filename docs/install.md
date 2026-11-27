# Installer Auris

## Raspberry Pi (clé en main)

```bash
curl -fsSL https://raw.githubusercontent.com/mjcc30/auris/main/install.sh | sudo bash
# puis : http://raspberrypi.local:3000
```

Le script installe le binaire `aarch64` dans `/usr/local/bin/auris`, crée
l'utilisateur `auris`, le data-dir `/var/lib/auris` et l'unité systemd
`auris.service` (démarrage auto au boot). Désinstallation : `sudo bash uninstall.sh`.

## Paquet .deb (Debian / Ubuntu / Raspberry Pi OS)

```bash
V=v0.8.0   # dernière version sur https://github.com/mjcc30/auris/releases
curl -fsSLO "https://github.com/mjcc30/auris/releases/download/$V/auris_${V#v}-1_amd64.deb"   # ou _arm64.deb
sudo apt install ./auris_*-1_amd64.deb
# le service auris.service est activé et démarré automatiquement
```

Vérifiez l'intégrité avec le fichier `.sha256` publié à côté du paquet.
Suppression : `sudo apt purge auris`.

## Paquet .rpm (Fedora / RHEL / openSUSE)

```bash
V=v0.8.0
curl -fsSLO "https://github.com/mjcc30/auris/releases/download/$V/auris-${V#v}-1.x86_64.rpm"  # ou .aarch64.rpm
sudo dnf install ./auris-*-1.x86_64.rpm
```

## Homebrew (macOS Apple Silicon)

```bash
brew tap mjcc30/auris
brew install auris
brew services start auris    # service launchd, http://localhost:3000
```

## winget (Windows 10/11)

```powershell
winget install mjcc30.Auris
auris serve --port 3000    # ou via le menu Démarrer
```

## Installeur Windows (.exe, Inno Setup)

Téléchargez `auris-setup-x64.exe` dans les [releases](https://github.com/mjcc30/auris/releases)
(vérifiez avec le `.sha256` voisin) et lancez-le : il installe Auris dans
`C:\Program Files\Auris`, crée les raccourcis et peut démarrer Auris au login
(interface web http://localhost:3000).

## Flatpak (Linux x86_64)

```bash
# bundle signé de la release :
curl -fsSLO https://github.com/mjcc30/auris/releases/download/v0.8.0/auris.flatpak
flatpak install --user auris.flatpak
flatpak run audio.auris.Auris serve --port 3000
# ou via Flathub quand le paquet y sera soumis : flatpak install flathub audio.auris.Auris
```

## Nix (Linux x86_64/aarch64, macOS arm64)

```bash
nix run github:mjcc30/auris/packaging/nix#auris -- serve --port 3000
# ou : nix profile install github:mjcc30/auris/packaging/nix#auris
```

Le flake empaquette le binaire de la release correspondant à votre plateforme
(`packaging/nix/flake.nix`, validé en CI par `nix build`).

## Image disque prête à flasher

Depuis les [releases](https://github.com/mjcc30/auris/releases), téléchargez :

- `auris-pc-vX.Y.Z.img.xz` — PC x86_64 (Debian stable embarquée, bootable BIOS/UEFI)
- `auris-rpi-vX.Y.Z.img.xz` — Raspberry Pi 4/5 (Raspberry Pi OS Lite arm64, ssh actif au premier boot)

```bash
xz -dkc auris-rpi-vX.Y.Z.img.xz | sudo dd of=/dev/sdX bs=4M status=progress conv=fsync
# ou flashez directement le .img avec Raspberry Pi Imager
```

## Binaires seuls (Linux / macOS / Windows)

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

### Dépendances de build natif Linux (ex. WSL / Ubuntu / Debian)

Le build depuis les sources a besoin des en-têtes système d'OpenSSL et d'ALSA
(crates `openssl-sys` et `alsa-sys` via `cpal`) :

```bash
sudo apt update && sudo apt install -y build-essential pkg-config libssl-dev libasound2-dev
```

Sous WSL, utilisez un répertoire cible séparé pour ne pas mélanger les
artefacts Windows et Linux dans `target/` :

```bash
cd /mnt/c/Users/mjcc/Documents/GitHub/volumio-bin
CARGO_TARGET_DIR=/tmp/auris-wsl-target cargo run -- serve --port 3000
```

Alternative sans build Linux : lancer le binaire Windows via l'interop WSL
(aucune dépendance ALSA nécessaire côté Windows) :

```bash
cd /mnt/c/Users/mjcc/Documents/GitHub/volumio-bin/target/debug
./auris.exe serve --port 3000 --data-dir /tmp/auris-wsl
# puis ouvrir http://localhost:3000 dans le navigateur Windows
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
