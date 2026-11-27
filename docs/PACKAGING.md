# Packaging & distribution

Auris est distribué sous plusieurs formes, toutes construites à partir des mêmes
binaires de release (assets `auris-<target>`) :

| Méthode | Fichier / emplacement | Construit par |
|---|---|---|
| Script one-liner (RPi/Linux/macOS) | `install.sh` / `uninstall.sh` (racine) | — (télécharge la release) |
| Paquet `.deb` (amd64, arm64) | job `deb` de `.github/workflows/release.yml` | CI |
| Paquet `.rpm` (x86_64, aarch64) | `packaging/rpm/auris.spec` + job `rpm` | CI (`rpmbuild`) |
| Installeur Windows `.exe` (x64) | `packaging/windows/auris.iss` + job `installer-win` | CI (Inno Setup) |
| Formule Homebrew (macOS arm64) | `packaging/brew/auris.rb` | manuel / tap `mjcc30/homebrew-auris` |
| Manifeste winget (Windows x64) | `packaging/winget/mjcc30.Auris.yaml` | PR vers `microsoft/winget-pkgs` |
| Bundle Flatpak (x86_64) | `packaging/flatpak/audio.auris.Auris.yml` + job `flatpak` | CI (`flatpak-builder`) ; Flathub via PR dédié |
| Paquet Nix (linux x86_64/aarch64, darwin arm64) | `packaging/nix/flake.nix` | validé en CI (job `nix`), build côté client |
| Images disque (PC x86_64, RPi arm64) | `packaging/image/build-image.sh` | workflow `.github/workflows/image.yml` |

## Cycle de release

1. Pousser un tag `vX.Y.Z` → `release.yml` build les binaires (Linux gnu x86_64/aarch64,
   macOS arm64, Windows x64), les paquets `.deb`/`.rpm`, et `checksums.txt`.
2. `image.yml` (manuel ou hebdo) construit les images disque et les attache à la release.
3. Mettre à jour les gestionnaires de paquets externes :

```bash
python3 packaging/brew/update-formula.py vX.Y.Z     # met à jour version/url/sha256 de la formule
python3 packaging/winget/update-manifest.py vX.Y.Z  # idem pour le manifeste winget
python3 packaging/rpm/update-spec.py vX.Y.Z         # idem pour le spec RPM (utilisé aussi en CI)
python3 packaging/windows/update-iss.py vX.Y.Z      # idem pour l'installeur Inno Setup
python3 packaging/flatpak/update-flatpak.py vX.Y.Z  # idem pour le manifeste flatpak-builder
python3 packaging/nix/update-nix.py vX.Y.Z          # idem pour le flake Nix (3 assets)
```

4. Ouvrir les PR externes :
   - Homebrew : `gh repo create mjcc30/homebrew-auris --public` (une fois), puis copier
     `packaging/brew/auris.rb` dans `Formula/auris.rb` du tap.
   - winget : PR sur `microsoft/winget-pkgs`, branche `mjcc30.Auris-<version>`, fichier dans
     `manifests/m/mjcc30/Auris/<version>/mjcc30.Auris.yaml`, validé avec `winget validate`.

## Signature GPG (optionnel)

Si les secrets `GPG_PRIVATE_KEY` et `GPG_PASSPHRASE` sont définis sur le dépôt, le job
`release` signe chaque `.deb`/`.rpm` (`.asc`) à la publication. Sans secret, les paquets
sont publiés non signés ; l'intégrité reste assurée par `checksums.txt` / les `.sha256`.

## Lint local

```bash
python3 packaging/lint.py   # vérifie scripts bash, formule, manifeste, spec, workflows, docs
```

Le job `packaging-lint` de la CI exécute ce script à chaque push/PR.

## Images disque (build manuel)

Sur un hôte Linux x86_64 en root :

```bash
sudo apt install debootstrap parted kpartx qemu-user-static xz-utils
sudo ./packaging/image/build-image.sh pc  v0.8.0    # -> auris-pc-v0.8.0.img (+ .sha256)
sudo ./packaging/image/build-image.sh rpi v0.8.0    # -> auris-rpi-v0.8.0.img (+ .sha256)
```
