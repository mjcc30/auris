# Auris — l'oreille avant tout

**Lecteur audiophile cross-plateforme en un seul binaire Rust.**

Branchez un Raspberry Pi + un DAC USB, pointez-le vers votre NAS,
ouvrez `http://<ip>:3000` depuis votre téléphone : votre musique joue,
en bit-perfect, jusqu'au **DSD256**.

![palette](assets/logo.svg)

## Pourquoi Auris ?

- **Un seul binaire**, clé en main : Raspberry Pi, Linux, macOS, Windows.
- **Votre NAS** (SMB/CIFS, UPnP/DLNA) ou vos fichiers locaux, sans cloud.
- **Bit-perfect** : aucun ré-échantillonnage, DSD64–256 via DoP jusqu'au DAC.
- **Interface web style Spotify**, embarquée dans le binaire, pilotable depuis
  n'importe quel écran du réseau.
- **Mise à jour automatique** depuis les releases GitHub.
- **AGPL v3** : le code reste libre, les plugins communautaires inclus.

## Installation en une ligne

```bash
curl -fsSL https://raw.githubusercontent.com/mjcc30/auris/main/install.sh | sudo bash
```

Puis ouvrez **http://<ip-de-la-machine>:3000**.

## Liens

- [Code source](https://github.com/mjcc30/auris) · [Releases](https://github.com/mjcc30/auris/releases)
- [Kanban](https://github.com/users/mjcc30/projects/2) · [Roadmap](ROADMAP.md)
- [Manifeste](../MANIFESTO.md) · [Contribuer](../CONTRIBUTING.md)
