# Roadmap — Auris

Jalons GitHub (milestones) + board kanban : https://github.com/users/mjcc30/projects/2

## v0.1.0 — MVP (31/12/2026)

Binaire unique, lecture locale PCM, UI web embarquée style Spotify,
API REST v1 + WS, CLI, CI complète (fmt/clippy/tests/couverture/commits).
*Sortie : `cargo build --release` produit un binaire fonctionnel sur 5 cibles.*

## v0.2.0 — NAS & bibliothèque (31/01/2027)

Sources SMB/UPnP/HTTP, scan + index + recherche + pochettes,
gestion des sources dans l'UI.
*Sortie : lecture depuis un NAS Synology/QNAP sans configuration manuelle avancée.*

## v0.3.0 — Audio audiophile (28/02/2027)

DSD64–256 via DoP, gapless, sélection DAC, badges qualité.
*Sortie : DSD256 lu bit-perfect sur un DAC compatible DoP.*

## v0.4.0 — Plugins communautaires v1 (31/03/2027)

Manifeste + protocole JSON-RPC stdio, registre `auris-plugins`, UI de gestion,
SDK Rust + exemples Python/Go.
*Sortie : un plugin tiers installé et fonctionnel en < 5 min.*

## v1.0.0 — Stable (30/04/2027)

Mise à jour auto, install clé en main (install.sh RPi/systemd, brew, winget),
site vitrine + doc sur GitHub Pages (domaine auris.audio), token d'accès optionnel,
i18n fr/en, accessibilité AA auditée.
*Sortie : 1.0 publique, annonces, paquets officiels.*

## v0.5.0 — Réglages façon Nova (30/06/2027)

Parité avec le menu **Réglages** de Volumio Nova, implémentée en propre :

- **Playback** : courbe de volume (logarithmique/naturelle), volume max, mode de
  volume (logiciel / matériel / bit-perfect désactivé), resampling (off par défaut,
  option soxr), mode DSD (DoP / PCM), gapless on/off, contrôle de mixer ALSA.
- **Apparence** : thème sombre / clair / système, couleur d'accent, fond d'écran,
  langue fr/en, « display mode » HDMI (pochette + heure lisibles de loin).
- **Réseau** : nom d'hôte, IP fixe (Linux), mode hotspot WiFi (Linux), proxy.
- **Système** : nom de l'appareil, mise à jour (déjà fait), redémarrer / éteindre,
  logs téléchargeables, réinitialisation usine, usage stockage, fuseau horaire.
- **Sources** : les toggles de la page Sources de Nova (métadonnées, CD rip,
  entrées numériques, Connect) — les sources elles-mêmes sont déjà là.
- Page Réglages avec **recherche de réglage** (comme Nova : taper « volume » →
  le réglage direct).

*Sortie : menu Réglages complet, persisté, cherchable.*

## v0.6.0 — Équivalents ouverts des fonctions Premium (31/08/2027)

Contrainte AGPL : pas de DRM ni d'API sous licence — uniquement des protocoles
ouverts ou des intégrations communautaires avec compte utilisateur :

- **« Jouer ici » (fait)** : pseudo-sortie « Navigateur » — PCM f32 décodé streamé en
  binaire sur `/ws/audio`, lu en WebAudio côté UI ; popup façon Manifest
  (Jouer ici / Regroupement / Caster / Autres dispositifs).
- **Liens de partage (faits)** : `POST /api/shares` crée un lien public
  `/share/<token>` (piste / album / playlist) avec mini-page HTML + flux audio
  (`/share/<token>/stream[/<track_id>]`), révocation et expiration ; boutons
  « Partager » (barre de lecture, album, playlist) + gestion dans Réglages > Partage.
- Plugins « 1st party » : Last.fm (scrobble + « j'aime »), Spotify Connect
  (librespot, MIT), AirPlay, Roon Ready (RAAT), Chromecast, Tidal/Qobuz via leurs
  API publiques communautaires (abonnement requis côté utilisateur).
- Multiroom synchronisé (style SnapCast) entre plusieurs instances Auris.
- « Super search » : recherche simultanée dans toutes les sources + services.
- Lecture infinie des radios, sauvegarde/restauration de la config (ZIP local ou WebDAV).
- CD ripping (via `whipper`/AccurateRip sur Linux), crédits/métadonnées (MusicBrainz).
- **Sleep timer / réveil + égaliseur (faits)** : minuterie one-shot (pause auto),
  réveil « HH:MM » heure locale, `GET /api/alarms` ; EQ 10 bandes biquads RBJ appliqués
  à chaud (DAC + navigateur), réglages dans `playback.sleep_timer` / `playback.wake_time`
  / `playback.eq`.

*Sortie : un écosystème de plugins fonctionnel + multiroom de base.*

## v0.7.0 — Podcasts & éditorial (taguée)

- Abonnements podcasts (RSS), épisodes, lecture dans la file, page dédiée.
- Radios web : lecture infinie, recherche, favoris.
- API REST v1 façon Volumio (`/api/v1/*`, #76) pour apps tierces.

## v0.8.0 — Sources gratuites sans compte (taguée)

- **Internet Archive** (#84) : populaires, ajouts récents, recherche, lecture directe.
- **Deezer extraits 30 s** (#85) : chart + recherche via API keyless.
- Auto-ajout des deux sources dans les nouvelles installations.

## v0.9.0 — Beta : tout le backlog restant (en cours)

**Politique de version** : aucun tag `v0.9.x` n'est posé — la **v0.9.0 est la release
beta** (milestone GitHub « v0.9.0 - Beta ») qui embarque tout ce qui reste du backlog ;
la **v1.0.0 est réservée à la vérification finale après les tests beta** (tag posé
uniquement quand tout est validé).

Livré dans cette beta :

- **API Subsonic serveur v1 lecture seule** (#79) sur `/rest/*` : ping, artists,
  albums, search3, playlists, coverArt, stream (Range/206) — DSub, Ultrasonic,
  Strawberry… se connectent à Auris comme à un serveur perso.
- **Serveur MPD v1** (#80) sur port TCP dédié (`mpd_port`, défaut 6600) : ~40 commandes
  (play/pause/stop/seek/setvol/playlistinfo/add/lsinfo/search/outputs…) — Cantata,
  Mpdroid, MALP, mconnectHD… pilotent Auris.
- **Source Jamendo** (#86) `kind = "jamendo"` : musique Creative Commons en lecture
  complète via l'API v3.0 (`client_id` perso gratuit, developer.jamendo.com) —
  recherche + populaires du mois ; sans `client_id` la source reste désactivée
  avec un message clair. Ajoutée désactivée par défaut, onglet « Jamendo (CC) »
  dans le formulaire de source.
- **Source YouTube via yt-dlp** (#87) `kind = "youtube"` : tendances + recherche
  `ytsearch:` + lecture audio `bestaudio`, sans compte Google (contenu public).
  Binaire `yt-dlp` résolu via `AURIS_YTDLP` puis le PATH ; message clair s'il
  manque. Auto-ajoutée activée ; installée par `install.sh` sur Linux.
- **API socket.io Volumio v1.7.4** (#45) : compat Engine.IO v3 EIO=3 (polling +
  websocket) — getState/getQueue/play/seek/setVolume/repeat/shuffle/queue… et push
  `pushState`/`pushQueue` ; vérifiée en live.
- **Pont CamillaDSP** (#82) : sonde et pilote une instance CamillaDSP via son
  websocket (`GetVersion`/`GetStatus`/`GetConfigNames`/`SetConfigName`) ; délègue
  le traitement (FIR/convolution) en routant le flux vers le périphérique de
  capture (mode loopback) ou un FIFO (mode pipe, PCM f32le), avec repli sur le
  DSP interne si l'instance est injoignable. UI : carte « CamillaDSP » dans
  Réglages (mode, URL ws, capture, sélecteur de profil, statut) + étage DSP dans
  le Signal Path. API : `/api/camilla/status|profile|refresh`.
- **Packaging clé en main complet** (#12) : `install.sh`/`uninstall.sh` (RPi + systemd),
  paquets `.deb` (amd64/arm64) et `.rpm` (x86_64/aarch64) publiés à chaque release avec
  `checksums.txt` et `.sha256`, signature GPG optionnelle (secrets), formule Homebrew
  (`packaging/brew/auris.rb` + tap `mjcc30/homebrew-auris`), manifeste winget
  (`packaging/winget/mjcc30.Auris.yaml` → PR `microsoft/winget-pkgs`), installeur Windows
  `.exe` (Inno Setup), bundle Flatpak, flake Nix, scripts de mise à jour auto
  (`update-formula.py`, `update-manifest.py`, `update-spec.py`, `update-iss.py`,
  `update-flatpak.py`, `update-nix.py`), lint `packaging/lint.py` en CI.
- **Images disque prêtes à flasher** (#46) : `packaging/image/build-image.sh` + workflow
  `image.yml` — validé en CI (run du 27/11/2026) : `auris-pc-v0.8.0.img.xz` (Debian x86_64
  bootable) et `auris-rpi-v0.8.0.img.xz` (Raspberry Pi OS Lite arm64, ssh au 1er boot),
  publiés avec `.sha256` sur la release.
- À part : **plugin YouTube Music (compte Google + cookies)** dans un repo plugin
  séparé (catégorie `source`, manifeste `auris-plugin.toml`) — voir issue dédiée.

Reste du backlog embarqué dans cette beta (milestone v0.9.0) : #24 Spotify Connect,
#25 Tidal & Qobuz, #28 Cast (Chromecast/AirPlay), #34 sortie Bluetooth,
#42 affichage HDMI, #43 GPIO/IR, #44 Ethernet+Wi-Fi, #83 éditorial Qobuz,
#88 plugin YT Music.

## Au-delà (idées, non planifiées)

- v1.1 : DSD natif (ASIO Windows, ALSA DSD), redémarrage auto après update,
  plugins UI (iframe bac à sable), égaliseur via plugin, application mobile native.
- v1.2 : multiroom synchronisé, chromecast/airplay via plugins output.
