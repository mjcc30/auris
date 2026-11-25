# Spécification de parité — Volumio 4 vs Auris

Analyse des fonctionnalités, specs et docs de **Volumio 4** (dernière version : **4.245**, publiée le **6 novembre 2026**) et comparaison avec Auris. Volumio 4 est un « changement de moteur complet » : base **Debian Bookworm**, noyau Linux récent, **MPD à jour**, plateformes **Raspberry Pi + PC x86/x64**, installation propre depuis Volumio 3 (Pi 1/Zero abandonnés). L'interface « **Nova** » est repensée (grandes pochettes).

> Contrainte Auris (AGPL-3.0-only) : pas de DRM ni d'API sous licence. Les fonctions « Premium » de Volumio sont remplacées par des **équivalents ouverts** (protocoles ouverts, intégrations communautaires avec compte utilisateur, ou code embarqué 100 % local).

## Tableau comparatif

| Domaine | Volumio 4.245 | Auris | Statut / ticket |
|---|---|---|---|
| **Système** | Debian Bookworm, noyau récent, OTA, logs | Binaire Rust unique cross-plateforme, **mise à jour auto faite** | Fait (#7) |
| **Lecture** | Bit-perfect, gapless, MPD récent (grosses bibliothèques) | Gapless + lecture bit-perfect faits | Fait |
| **DSD** | DoP jusqu'à DSD256, **Direct DSD jusqu'à DSD512** sur DAC compatibles | DSD64–256 via DoP fait, DSD natif (ASIO/ALSA DSD) prévu v1.1 | Partiel (#6 en cours) |
| **Resampling / DSP** | Resampling (soxr), EQ, crossfade, correction de pièce via plugins (FusionDsp) | **EQ 10 bandes fait** ; resampling soxr prévu v0.5 ; crossfade/loudness/IR non faits | Backlog #37 |
| **Sorties audio** | USB DAC (liste élargie), HDMI/I2S, sortie **Bluetooth** (Wireless Output Manager, expérimental), « jouer ici » | Sélection DAC **multi-hôtes cpal fait** (commit `1221c75`), « Jouer ici » navigateur fait, HDMI/I2S via ALSA | BT : backlog #34 |
| **Récepteurs / Connect** | AirPlay, Chromecast, **Spotify Connect**, TIDAL/Qobuz Connect, **Roon Ready** | Prévus v0.6 (protocoles ouverts / libres) | #24, #25, #28 |
| **Renderer DLNA** | Peut envoyer la lecture vers un appareil DLNA tiers | Source UPnP faite (#11), renderer non fait | Backlog #35 |
| **Réseau** | Ethernet + Wi-Fi **simultanés**, hotspot, proxy | Réseau implicite | Backlog #44 |
| **NAS** | SMB/NFS/UPnP, scan auto, pochettes | **Sources SMB/UPnP faites, scan + pochettes faits** | Fait |
| **Radios web** | Lecteur de radios (streams infinis, favoris, métadonnées ICY) | Non fait | Backlog #36 |
| **Multiroom** | Groupement synchronisé jusqu'à **6 pièces** | Prévu v0.6 (style SnapCast, protocole ouvert) | #23 |
| **CD** | Rip CD + lecture « low-noise » depuis graveur USB | Rip via whipper prévu | #27 |
| **UI Nova** | Grandes pochettes, grille par genres/décennies, recherche globale, pages artiste (bio, similaires, top titres), réglages **cherchables** | Pochettes bibliothèque + Parcourir faites (`1221c75`), recherche globale faite, page artiste faite, réglages façon Nova prévus v0.5 | Facettes/enrichissement : #41 ; données bio/similaires : #26 |
| **Recherche** | Recherche globale + « super search » multi-sources (Premium, assistée IA) | Recherche biblio faite ; multi-sources sémantique embarquée non faite | Backlog #38 |
| **Paroles** | Lyrics (Premium) | Non fait — LRCLib (API ouverte) | Backlog #39 |
| **Sauvegarde** | Cloud backup config (Premium) | Non fait — ZIP local + WebDAV (ouvert) | Backlog #40 |
| **Plugins** | Repo communautaire `volumio-plugins-sources-bookworm`, plugins GPIO buttons, etc. | **Système de plugins communautaires v0.4** (JSON-RPC stdio, registre) | #8 |
| **API** | socket.io **v1.7.4** + REST (app mobile BLE) | REST v1 + WebSocket JSON natifs | Compat socket.io : #45 |
| **Écran** | HDMI tactile, mode affichage « de loin » (pochette + horloge) | UI web uniquement | Backlog #42 |
| **GPIO / IR** | Boutons GPIO (plugin), télécommande IR | Non fait | Backlog #43 |
| **Installation** | Images disque RPi + PC x86/x64 prêtes à flasher | install.sh + paquets prévus v1.0 | #12, images : #46 |
| **Mobile** | App iOS/Android (BLE + socket.io) | UI web responsive ; remote via API native | App native : au-delà v1.0 |

## Fonctions Premium Volumio → équivalents ouverts Auris (v0.6)

| Volumio Premium | Équivalent Auris (contrainte AGPL) | Ticket |
|---|---|---|
| TIDAL / Qobuz natifs | API publiques communautaires, compte perso requis | #25 |
| Spotify Connect | librespot (MIT) | #24 |
| Multiroom jusqu'à 6 | Multiroom synchronisé ouvert (style SnapCast) | #23 |
| Rip + lecture CD | whipper / AccurateRip (Linux) | #27 |
| Cloud backup | ZIP local + WebDAV | #40 |
| Recherche « IA » | Sémantique 100 % embarquée (embeddings locaux, aucune API tierce) | #38 |
| Lyrics | LRCLib (ouvert) | #39 |
| Connect (TIDAL/Qobuz/Spotify) | #24 + #25 | #24, #25 |
| AirPlay / Chromecast / Roon Ready | Protocoles ouverts (shairport, CastV2, RAAT) | #28 |

## Bugs Volumio 4 constatés côté utilisateur (corrigés chez Auris)

Tickets **#29–#33** (corrigés dans le commit `1221c75`, fermés) :

- **#29** — le DAC branché au PC n'apparaissait pas : `/api/audio/devices` n'énumère plus qu'un hôte cpal → **tous les hôtes** + bouton « actualiser » + indice WSL/Windows.
- **#30** — lecture NAS impossible même « Jouer ici » : le moteur exigeait un périphérique cpal → **repli « navigateur uniquement »** (thread drain temps réel → `/ws/audio`).
- **#31** — pas de vignettes dans Parcourir → **`GET /api/browse/cover?uri=`** (cover.jpg/folder.jpg/… ou pochette embarquée).
- **#32** — recherche globale sans effet → **vue résultats dédiée** (Artistes + Albums + Titres).
- **#33** — clic sur un artiste sans effet → **`GET /api/library/artists/{id}`** renvoie albums + titres + pochette, page artiste dédiée.

## Écarts Volumio 4 restants → nouveaux tickets

- **#34** — Sortie audio Bluetooth (équivalent Wireless Output Manager)
- **#35** — Renderer DLNA/UPnP (diffuser vers un appareil tiers)
- **#36** — Radios web (streams infinis, favoris, ICY)
- **#37** — DSP avancé local : crossfade, loudness, correction de pièce (type FusionDsp)
- **#38** — Super recherche multi-sources + sémantique embarquée
- **#39** — Paroles synchronisées (LRCLib)
- **#40** — Sauvegarde/restauration config (ZIP + WebDAV)
- **#41** — UI façon Nova : facettes Genres/Décennies, grandes pochettes, page artiste enrichie
- **#42** — Mode affichage HDMI « de loin » (pochette + horloge)
- **#43** — Boutons GPIO + télécommande IR (RPi)
- **#44** — Ethernet + Wi-Fi simultanés
- **#45** — Compatibilité API socket.io Volumio (clients existants)
- **#46** — Images d'installation prêtes (RPi/PC x86, .deb/.rpm)

Tous ajoutés au [kanban](https://github.com/users/mjcc30/projects/2) en Backlog (milestone v0.6.0 #7 ou v1.0.0 #5).

## Sources

- https://volumio.com/experience/ — « Volumio OS 4 : the engine » (Debian Bookworm, MPD récent, RPi + PC x86/x64, v4.245 du 06/11/2026)
- https://help.volumio.com/en/articles/74-everything-you-need-to-know-about-updating-to-volumio-4 — annonce officielle Volumio 4
- https://volumio.com/direct-dsd-support-volumio-dsd512/ — Direct DSD jusqu'à DSD512
- https://community.volumio.com/t/fusiondsp-the-complete-dsp-center-for-volumio3-and-volumio4/50474 — DSP communautaire
- https://community.volumio.com — multiroom (jusqu'à 6 appareils), changelog
- https://github.com/volumio/docs et https://github.com/volumio/volumio-plugins-sources-bookworm — doc développeur + dépôt plugins
- https://volumio.com/get-started/ — plateformes et images d'installation
