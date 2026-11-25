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
- [x] Sélection du périphérique de sortie (DAC) depuis l'UI — liste de **tous** les hôtes audio, re-scan à chaud (bouton actualiser), étoile sur le défaut
- [x] **Sortie audio persistante** : le DAC choisi et le mode (`local`/`browser`) sont écrits dans `config.toml` et repris au démarrage — la sortie ne suit plus le périphérique par défaut de Windows quand on le change ; choisir un DAC repasse la lecture en local même en mode « Jouer ici » ; nom inconnu → 400 avec la liste ; DAC débranché au démarrage → repli sur la sortie par défaut avec avertissement
- [x] **Lancement sans friction** (double-clic du .exe) : ouverture automatique du navigateur sur l'UI (`--no-open` / `AURIS_NO_OPEN` pour désactiver), **instance unique** (un second lancement ouvre juste le navigateur sur l'instance existante), `auris shortcut` installe un raccourci Bureau + un lancement au démarrage de Windows
- [ ] CLI : `serve`, `version`, `audio-devices`, `usb-devices`, `shortcut`, `update`
- [ ] CI : fmt, clippy, tests, couverture, commits conventionnels

## Should have — v0.2.0 (NAS & bibliothèque)

- [x] Source SMB/CIFS vers NAS (Synology, QNAP, TrueNAS...) — inscriptible (copie vers le NAS)
- [x] Sources multiples: dossier local (NTFS monté), **USB** (détection des disques amovibles), SMB, HTTP, UPnP/DLNA, webradio
- [x] Source UPnP/DLNA (découverte SSDP, navigation ContentDirectory)
- [x] Webradios (m3u/m3u8) et URLs HTTP directes
- [x] Scan des sources, index sled, métadonnées + pochettes
- [x] Recherche plein texte (barre globale + bibliothèque) : résultats **artistes + albums + titres** ; pages albums et **artistes** (pochette, albums, tous les titres, « Tout lire »)
- [x] Gestion des sources depuis l'UI façon Nova (cartes avec interrupteur, modale d'édition 2 colonnes, ajout par onglets local/USB/NAS/radio/HTTP) — ajout/édition (PUT)/suppression/activation, persistées dans `config.toml`
- [x] UI conforme à la charte graphique (thème sombre `#121212`, accent `#1ED760`, icônes SVG, recherche globale en barre supérieure)
- [x] **Explorateur d'arborescence** dans chaque source (grille ou liste, fil d'Ariane, lecture d'un dossier entier) — **pochettes des dossiers** (`/api/browse/cover`) : `cover.jpg`/`folder.jpg` du dossier, sinon pochette embarquée du 1er fichier audio
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

## Should have — v0.5.0 (Réglages façon Nova)

- [x] Modèle `Settings` persisté dans `config.toml` (`[settings]`) + `GET/PUT /api/settings` (validation, 400 si invalide)
- [x] Réglages lecture appliqués à chaud au moteur : courbe de volume (log/linéaire), volume max (borne le volume courant), mode de volume logiciel / désactivé (bit-perfect, gain 1.0), gapless, resampling off, mode DSD (DoP/PCM)
- [x] Apparence : thème sombre / clair / système, couleur d'accent `#RRGGBB` personnalisable, image de fond optionnelle
- [x] Système : nom de l'appareil, fuseau horaire, usage stockage (`/api/system/storage`), logs (`/api/system/logs`, 400 dernières lignes), reset usine confirmé (`{"confirm":"RESET"}`), redémarrer/éteindre protégés par `power_control = true` (403 sinon)
- [x] Page Réglages façon Nova : recherche de réglage + sections (Lecture, Apparence, Système, Réseau, Sources)
- [x] Le slider de volume de la barre de lecture est masqué en mode bit-perfect

## Could have — v0.6.0 (équivalents ouverts du Premium)

- [x] **« Jouer ici »** : le navigateur courant devient la sortie audio — PCM f32 décodé
      streamé en binaire sur `/ws/audio`, lu en WebAudio (lookahead 120 ms, resync sur
      écart) ; `POST /api/player/output` bascule local/browser, le DAC local reçoit du
      silence en mode navigateur ; popup « Lire sur… » façon Manifest (Jouer ici /
      Regroupement / Caster / Autres dispositifs). **Repli navigateur** : si aucun
      périphérique audio local n'est disponible (ex. binaire WSL), « Jouer ici »
      fonctionne quand même (PCM forwardé au navigateur sans cpal)
- [x] **Liens de partage** : lien public `/share/<token>` pour une piste, un album ou une
      playlist — mini-page HTML façon Nova avec lecteur intégré, flux audio
      `/share/<token>/stream[/<track_id>]`, JSON public `/api/public/shares/<token>`,
      révocation (`DELETE /api/shares/<token>`), expiration optionnelle ; boutons
      « Partager » barre de lecture / album / playlist + liste dans Réglages > Partage
- [ ] Multiroom synchronisé (style SnapCast) entre instances Auris
- [ ] AirPlay / Roon Ready (RAAT) / Chromecast (protocoles ouverts)
- [ ] Spotify Connect via librespot (MIT), Tidal/Qobuz via API communautaires
- [ ] Last.fm (scrobble) + métadonnées MusicBrainz, rip CD (whipper/AccurateRip)
- [x] **Sleep timer / réveil + égaliseur paramétrique** : minuterie de sommeil one-shot
      (pause auto après N minutes), réveil quotidien « HH:MM » heure locale (relance la
      lecture), compte à rebours exposé via `GET /api/alarms` et badge dans le lecteur ;
      EQ 10 bandes ISO (31 Hz – 16 kHz, ±20 dB) en biquads RBJ appliqués à chaud dans le
      callback audio (DAC local et « Jouer ici »), sliders dans Réglages > Égaliseur
- [x] **Panneau « lecture en cours »** (style Volumio Nova) : clic sur la pochette de la
      barre de lecture → panneau latéral à 3 onglets — **Pochette** (grande pochette,
      artiste, album, format/année/genre), **Infos** (bio Wikipedia de l'artiste mise en
      cache dans sled, infos de l'album, commentaire embarqué du fichier), **Paroles**
      (paroles synchronisées LRC avec ligne active suivie en temps réel ; source : tag
      USLT/LYRICS embarqué d'abord, repli [lrclib.net](https://lrclib.net) via
      `GET /api/lyrics/{track_id}` et `GET /api/infos/{track_id}`)

## Should have — v1.0.0 (stable)

- [ ] Mise à jour automatique (check 6 h + bouton + checksums SHA-256)
- [ ] Install clé en main : `install.sh` Raspberry Pi + unité systemd,
      paquets macOS (brew) et Windows (winget) — dépôts publics
- [ ] Site vitrine + documentation (GitHub Pages, domaine auris.audio)
- [ ] Authentification par token (optionnelle) sur l'API
- [ ] i18n fr/en complet

## Won't have (v1)

- Multiroom, comptes cloud, DRM, applications natives iOS/Android.
