# API REST v1 + WebSocket — Auris

Base : `http://<hôte>:3000`. Toutes les réponses sont en JSON. Les erreurs :
`{"error": "<message>"}` avec un code HTTP 4xx/5xx.

## Lecteur

| Méthode | Route | Body | Description |
|---|---|---|---|
| GET | `/api/state` | — | état complet du lecteur (`PlayerState`) |
| POST | `/api/player/play` | `{uri}` ou `{track_id}` ou `{queue:[Track], index}` | joue |
| POST | `/api/player/pause` · `/resume` · `/stop` · `/next` · `/prev` | — | contrôles |
| POST | `/api/player/seek` | `{secs: number}` | position |
| POST | `/api/player/volume` | `{volume: 0-100}` | volume |
| POST | `/api/player/mute` | `{muted: bool}` | muet |
| POST | `/api/player/repeat` | `{mode: "off"|"all"|"one"}` | répétition |
| POST | `/api/player/shuffle` | `{enabled: bool}` | aléatoire |

`PlayerState` :

```json
{
  "status": "playing|paused|stopped",
  "current": { "id": "...", "title": "...", "artist": "...", "album": "...",
               "duration_secs": 245.3, "uri": "src://...", "format": {"kind":"pcm","sample_rate":96000,"bits":24,"channels":2}, ... },
  "position_secs": 12.4, "duration_secs": 245.3,
  "volume": 80, "muted": false, "repeat": "off", "shuffle": false,
  "queue": [ /* Track[] */ ], "queue_index": 0,
  "audio_device": "DAC USB", "current_format": {"kind":"dsd","sample_rate":5644800,"channels":2}
}
```

## File d'attente

| Méthode | Route | Description |
|---|---|---|
| GET | `/api/queue` | la file |
| POST | `/api/queue` | ajoute une `Track` |
| POST | `/api/queue/add` | `{uri}` ajoute un fichier **ou tout un dossier** (`src://id/...`) |
| DELETE | `/api/queue/{index}` | retire |
| POST | `/api/queue/clear` | vide (et stoppe) |

## Bibliothèque

| Méthode | Route | Description |
|---|---|---|
| GET | `/api/library/tracks` | toutes les pistes |
| GET | `/api/library/albums` | albums (+ `cover_url`) |
| GET | `/api/library/albums/{id}` | `{album, tracks, cover_url}` |
| GET | `/api/library/artists` | artistes |
| GET | `/api/library/artists/{id}` | `{artist, albums, tracks, cover_url}` — page artiste complète |
| GET | `/api/library/search?q=...` | recherche plein texte (titre/artiste/album/genre) sur les pistes |
| GET | `/api/library/cover/{track_id}` | image (JPEG/PNG) |
| GET | `/api/library/albums/{id}/export` | exporte l'album en ZIP (pistes + pochettes) |
| POST | `/api/library/scan` | `{source_id?}` lance un scan (toutes sources si omis) |
| GET | `/api/library/scan/status` | progression du scan |

## Sources & navigation

| Méthode | Route | Description |
|---|---|---|
| GET | `/api/sources` | sources configurées |
| POST | `/api/sources` | ajoute une source (id unique requis, sinon 400) |
| DELETE | `/api/sources/{id}` | supprime |
| PATCH | `/api/sources/{id}` | `{enabled}` active/désactive (persisté) |
| PUT | `/api/sources/{id}` | remplace la source (corps = source complète, `id` identique requis, sinon 400 ; persisté) |
| GET | `/api/browse?uri=...` | contenu d'un dossier (`src://id/...`, `upnp://...`, URL m3u) |
| GET | `/api/browse/tree?uri=...` | parcours **récursif** (fichiers audio ; un fichier = lui-même) |
| GET | `/api/browse/cover?uri=...` | pochette d'un dossier : `cover.jpg`/`folder.jpg`/… du dossier, sinon pochette embarquée du 1er fichier audio (404 si aucune) |
| GET | `/api/usb/devices` | disques amovibles détectés par l'OS (`[{label, path}]`) |
| GET | `/api/upnp/discover` | serveurs UPnP du LAN |

## Import / export & playlists

| Méthode | Route | Description |
|---|---|---|
| POST | `/api/transfer/copy` | `{from, to}` copie fichier/dossier entre sources (`src://a/x` → `src://b/y`) → `{files, bytes, dirs, errors}` ; destination en lecture seule ou chemin `..` → 400 |
| GET | `/api/export?uri=...&name=...` | exporte un fichier/dossier en **ZIP** (attachement) |
| GET | `/api/playlists` | liste des listes de lecture |
| POST | `/api/playlists` | `{name, track_ids?}` ; sans `track_ids` = sauvegarde de la file courante |
| GET | `/api/playlists/{id}` | `{playlist, tracks}` (pistes résolues) |
| DELETE | `/api/playlists/{id}` | supprime |
| POST | `/api/playlists/{id}/play` | joue la liste |

Création d'une source SMB :

```json
{ "name": "NAS", "kind": "smb", "host": "192.168.1.10", "share": "music",
  "user": "...", "password": "...", "base_path": "" }
```

(`kind`: `local` `{path}` (dossier, ex. NTFS monté), `usb` `{path}` (clé/disque
amovible, inscriptible), `smb` `{host,share,user,password,base_path?}` (NAS,
inscriptible), `radio` `{url}` (flux), `http` `{base_url?}`, `upnp` `{url}`.
Les sources `local`, `usb` et `smb` sont **inscriptibles** : on peut y copier
depuis n'importe quelle autre source. Les URI `src://` acceptent l'encodage
percent (`%20`) ou les espaces bruts dans le chemin.)

## Audio & système

| Méthode | Route | Description |
|---|---|---|
| GET | `/api/audio/devices` | DAC détectés sur **tous** les hôtes audio (WASAPI/ALSA/CoreAudio…), dédupliqués : un DAC branché à chaud apparaît sans redémarrer (capacités, taux, `is_default`) |
| POST | `/api/audio/device` | `{name?}` sélectionne le DAC |
| GET | `/api/system/info` | version, OS, uptime, counts |
| GET | `/api/system/update` | dernière vérification de mise à jour |
| POST | `/api/system/update` | applique la mise à jour (redémarrage requis) |
| GET | `/api/system/storage` | usage disque du data-dir (`used/total/free_bytes`) |
| GET | `/api/system/logs` | 400 dernières lignes du journal (texte brut, ANSI retiré) |
| POST | `/api/system/reset` | reset usine : body `{"confirm":"RESET"}` (sinon 400) |
| POST | `/api/system/reboot` | redémarre la machine (403 si `power_control = false`) |
| POST | `/api/system/shutdown` | éteint la machine (403 si `power_control = false`) |

## Réglages (menu « Réglages » façon Volumio Nova)

| Méthode | Route | Description |
|---|---|---|
| GET | `/api/settings` | réglages courants (persistés dans `config.toml`, section `[settings]`) |
| PUT | `/api/settings` | remplace les réglages (validés, 400 si invalide), appliqués à chaud au moteur |

Sections : `playback` (courbe de volume, `max_volume`, `volume_mode` software/disabled,
gapless, resampling, `dsd_mode`, `sleep_timer` en minutes (one-shot, null = off),
`wake_time` « HH:MM » heure locale (null = off), `eq` : égaliseur paramétrique
`{enabled, bands: [{freq_hz, gain_db}]}` 10 bandes ISO 31 Hz – 16 kHz, ±20 dB),
`appearance` (thème dark/light/system, `accent` `#RRGGBB`, `background`),
`system` (`device_name`, `timezone`), `network` (`hostname`),
`sources` (`metadata_discovery`). `PlayerState` expose aussi `max_volume` et
`volume_mode` pour que l'UI masque le volume en mode bit-perfect.

| Méthode | Route | Description |
|---|---|---|
| GET | `/api/alarms` | état des alarmes : `{sleep_timer, sleep_remaining_secs, wake_time}` |

Une tâche de fond (1 tick/s) applique les alarmes : à l'expiration de la minuterie de
sommeil, la lecture passe en pause (one-shot) ; à l'heure de réveil, la lecture reprend
(pause → resume, arrêté → rejoue la file). Les deux envoient un événement WS `alarm`.
L'égaliseur est appliqué dans le callback audio (biquads RBJ, avant le gain de volume),
sur le DAC local comme en « Jouer ici », et se met à jour à chaud sans coupure.

## « Jouer ici » (sortie navigateur)

| Méthode | Route | Description |
|---|---|---|
| POST | `/api/player/output` | `{ "mode": "local" \| "browser" }` — « Jouer ici » : le navigateur courant devient la sortie audio |
| WS | `/ws/audio` | flux binaire : trame texte `audio_spec` (JSON) puis PCM f32 LE entrelacé, post-gain (volume UI actif) |

`PlayerState.output_mode` = `"local"` ou `"browser"`.

## Liens de partage (v0.6)

| Méthode | Route | Description |
|---|---|---|
| GET | `/api/shares` | liens actifs, chacun avec son `url` (`/share/<token>`) |
| POST | `/api/shares` | `{kind: "track"\|"album"\|"playlist", id, expires_in_secs?}` → 201 + lien ; ressource inconnue → 400 |
| DELETE | `/api/shares/{token}` | révoque le lien (404 si inconnu) |
| GET | `/api/public/shares/{token}` | JSON public : métadonnées + pistes avec `stream_url` |
| GET | `/share/{token}` | mini-page HTML publique façon Nova (lecteur audio intégré, liste de pistes) |
| GET | `/share/{token}/stream` | flux audio d'un lien « piste » (Content-Type selon l'extension) |
| GET | `/share/{token}/stream/{track_id}` | flux d'une piste d'un lien « album »/« playlist » (404 si hors partage) |

Les liens expirés sont purgés à la lecture ; le reset usine les supprime (stockés dans
la bibliothèque, arbre sled `shares`). Boutons « Partager » : barre de lecture (piste
en cours), fiche album, détail de playlist ; gestion dans Réglages > Partage.

## WebSocket `/ws`

Connexion upgrade → l'UI reçoit immédiatement `{"type":"state","payload":{...}}`,
puis à chaque changement :

```json
{"type": "state",  "payload": { /* PlayerState */ }}
{"type": "queue",  "payload": [ /* Track[] */ ]}
{"type": "error",  "payload": {"message": "..."}}
{"type": "scan",   "payload": { /* ScanStatus */ }}
```

## Exemples

```bash
# jouer un fichier du NAS
curl -X POST localhost:3000/api/player/play -d '{"uri":"src://<source_id>/Flac/album/track01.flac"}' -H 'Content-Type: application/json'

# régler le volume
curl -X POST localhost:3000/api/player/volume -d '{"volume": 65}' -H 'Content-Type: application/json'

# ajouter un NAS
curl -X POST localhost:3000/api/sources -H 'Content-Type: application/json' -d \
  '{"name":"NAS","kind":"smb","host":"192.168.1.10","share":"music","user":"u","password":"p","base_path":""}'
```
