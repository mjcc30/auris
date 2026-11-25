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

## Au-delà (idées, non planifiées)

- v1.1 : DSD natif (ASIO Windows, ALSA DSD), redémarrage auto après update,
  plugins UI (iframe bac à sable), égaliseur via plugin, application mobile native.
- v1.2 : multiroom synchronisé, chromecast/airplay via plugins output.
