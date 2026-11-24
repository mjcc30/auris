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
| DELETE | `/api/queue/{index}` | retire |
| POST | `/api/queue/clear` | vide (et stoppe) |

## Bibliothèque

| Méthode | Route | Description |
|---|---|---|
| GET | `/api/library/tracks` | toutes les pistes |
| GET | `/api/library/albums` | albums (+ `cover_url`) |
| GET | `/api/library/albums/{id}` | `{album, tracks, cover_url}` |
| GET | `/api/library/artists` · `/api/library/artists/{id}` | artistes |
| GET | `/api/library/search?q=...` | recherche |
| GET | `/api/library/cover/{track_id}` | image (JPEG/PNG) |
| POST | `/api/library/scan` | `{source_id?}` lance un scan (toutes sources si omis) |
| GET | `/api/library/scan/status` | progression du scan |

## Sources & navigation

| Méthode | Route | Description |
|---|---|---|
| GET | `/api/sources` | sources configurées |
| POST | `/api/sources` | ajoute une source (voir ci-dessous) |
| DELETE | `/api/sources/{id}` | supprime |
| GET | `/api/browse?uri=...` | contenu d'un dossier (`src://id/...`, `upnp://...`, URL m3u) |
| GET | `/api/upnp/discover` | serveurs UPnP du LAN |

Création d'une source SMB :

```json
{ "name": "NAS", "kind": "smb", "host": "192.168.1.10", "share": "music",
  "user": "...", "password": "...", "base_path": "" }
```

(`kind`: `local` `{path}`, `smb` `{host,share,user,password,base_path}`,
`http` `{base_url?}`, `upnp` `{url}`.)

## Audio & système

| Méthode | Route | Description |
|---|---|---|
| GET | `/api/audio/devices` | DAC détectés (capacités, taux) |
| POST | `/api/audio/device` | `{name?}` sélectionne le DAC |
| GET | `/api/system/info` | version, OS, uptime, counts |
| GET | `/api/system/update` | dernière vérification de mise à jour |
| POST | `/api/system/update` | applique la mise à jour (redémarrage requis) |

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
