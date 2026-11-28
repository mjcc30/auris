# Alternatives à Volumio, moOde, Navidrome, Roon : le comparatif (2026)

Vous cherchez une **alternative à Volumio**, un **serveur de musique auto-hébergé**
gratuit et open source, ou un lecteur audiophile pour votre NAS + DAC USB ?
Cette page compare **Auris** à Volumio, moOde, Navidrome et Roon, sans langue de bois.

> TL;DR — **Auris** vise le même usage que Volumio (Raspberry Pi + DAC + interface web
> depuis le téléphone) mais en **un seul binaire Rust natif**, 100 % open source (AGPL v3),
> avec sortie **bit-perfect jusqu'au DSD256** et **mise à jour automatique**.

## Tableau comparatif

| Critère | **Auris** | Volumio | moOde | Navidrome | Roon |
|---|---|---|---|---|---|
| Licence | AGPL v3 (libre) | MIT (freemium « MyVolumio ») | GPL v3 | GPL v3 | Propriétaire |
| Prix | **Gratuit** | Gratuit, mais fonctionnalités cloud payantes | **Gratuit** | **Gratuit** | ~15 €/mois |
| Forme | **Un seul binaire** (Rust) | Image OS complète (Debian) | Image OS complète | Binaire + Docker | Abonnement + serveur « Roon Core » |
| Taille / empreinte | ~15 Mo, faible conso | Image ~1 Go, conso moyenne | Image ~1 Go | ~30 Mo | ~200 Mo+ |
| NAS : SMB/CIFS, UPnP/DLNA, HTTP | ✅ natif | ✅ | ✅ | ✅ (via Subsonic) | ⚠️ via autres |
| Sortie bit-perfect DAC USB | ✅ PCM jusqu'au DXD, **DSD256 via DoP** | ✅ (DSD via DoP selon version) | ✅ DSD natif | ⚠️ via clients Subsonic | ✅ (Roon Ready / RAAT) |
| Interface web moderne (style Spotify) | ✅ embarquée, responsive mobile | ✅ | ✅ (MPD web) | ✅ | ✅ (app officielle) |
| **Clients mobiles natifs** | ✅ via API **OpenSubsonic** (Symfonium, DSub, play:Sub…) | ⚠️ app MyVolumio payante / plugins | ⚠️ clients MPD génériques | ✅ **OpenSubsonic natif** | ✅ app officielle incluse |
| Mise à jour automatique | ✅ depuis les releases GitHub | ⚠️ manuelle / cloud | ⚠️ manuelle | ✅ Watchtower | ✅ |
| Multi-room (multiroom) | ✅ natif | ✅ (payant en partie) | ❌ | ⚠️ via clients | ✅ zones |
| Enregistrement des radios web | ✅ (découpe auto par morceau) | ❌ | ❌ | ❌ | ❌ |
| Paroles (LRCLIB) + métadonnées MusicBrainz | ✅ | ⚠️ plugins | ❌ | ⚠️ via plugins | ✅ (payant) |
| Radio web « artiste » (Instant Mix-like) | ✅ | ⚠️ plugins | ❌ | ✅ (smart playlists) | ✅ (Roon Radio, payant) |
| PWA installable / hors-ligne | ✅ | ❌ | ❌ | ❌ | ✅ app |
| Docker / CasaOS / Yunohost | ✅ (image officielle) | ⚠️ image officielle | ❌ | ✅ | ❌ |
| Auto-hébergé, zéro cloud obligatoire | ✅ | ⚠️ compte MyVolumio poussé | ✅ | ✅ | ❌ (compte Roon requis) |

## Quand choisir quoi ?

- **Volumio** : interface très aboutie et écosystème de plugins, mais image OS lourde,
  fonctionnalités cloud de plus en plus payantes et code partiellement propriétaire côté cloud.
  → Auris en est l'**alternative open source et légère**, même usage (Pi + DAC + navigateur).
- **moOde** : excellent lecteur audiophile gratuit pour Raspberry Pi, très complet côté
  rendu audio (MPD). → Moins « serveur » : pas d'API moderne pour apps mobiles, pas de multi-room natif.
- **Navidrome** : la référence du **serveur de musique auto-hébergé** avec clients Subsonic.
  → Auris expose justement une **API OpenSubsonic compatible** : les mêmes apps mobiles
  (Symfonium, play:Sub, DSub, Ultrasonic) se connectent directement à Auris.
- **Roon** : l'expérience la plus léchée du marché (métadonnées, Roon Radio, multi-room),
  mais **payante et propriétaire**, avec compte cloud obligatoire.
  → Auris en reprend gratuitement les idées fortes : page artiste enrichie (MusicBrainz,
  bio Wikipedia, artistes similaires), « signal path », enregistrement radio façon K7.

## Auris en 30 secondes

```bash
# Raspberry Pi / Linux / macOS — installation en une ligne :
curl -fsSL https://raw.githubusercontent.com/mjcc30/auris/main/install.sh | sudo bash
# puis ouvrir depuis n'importe quel appareil du réseau :
http://<ip-de-la-machine>:3000
```

- Un seul binaire Rust, zéro dépendance à installer (hors `libasound2`, déjà présent sur Raspberry Pi OS).
- Pointez vers votre **NAS** (SMB/CIFS, UPnP/DLNA) ou un dossier local : la bibliothèque se scanne toute seule.
- **Bit-perfect** : aucun ré-échantillonnage ; DSD64–DSD256 en DoP vers votre DAC USB.
- Interface web **style Spotify** embarquée, pilotable depuis téléphone, tablette ou PC.
- **Mise à jour automatique** depuis les releases GitHub.
- API **OpenSubsonic** (`/rest/…`) pour brancher les apps mobiles existantes.

## Questions fréquentes (FAQ)

**Auris est-il une alternative gratuite à Volumio ?**
Oui : 100 % gratuit et open source (AGPL v3), sans compte cloud ni version payante,
légèrement plus sobre qu'une image OS complète.

**Puis-je utiliser mes apps Subsonic (Symfonium, DSub…) avec Auris ?**
Oui : Auris implémente l'API OpenSubsonic (`/rest/ping`, `getArtists`, `getAlbumList2`,
`search3`, `stream`, `getCoverArt`, `star`, `scrobble`…). Indiquez l'URL du serveur et,
si elle est configurée, le token d'accès dans les réglages.

**Le DAC USB est-il lu en bit-perfect, y compris le DSD ?**
Oui : PCM jusqu'au DXD et DSD64–DSD256 via DoP, sans conversion (voir
[Signal Path](SPECIFICATIONS.md)).

**Ça tourne sur Raspberry Pi ?**
Oui, image et binaire `aarch64` officiels, install.sh en une ligne.

---
*Voir aussi : [fonctionnalités](FEATURES.md), [parité avec Volumio 4](SPEC_VOLUMIO.md),
[installation](install.md), [manifeste](https://github.com/mjcc30/auris/blob/main/MANIFESTO.md).*
