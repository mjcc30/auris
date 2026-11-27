# Sources, arborescence & import/export

Auris lit (et écrit) depuis plusieurs types de sources, toutes navigables
depuis l'onglet **Parcourir** de l'UI.

## Types de sources

| Kind | Champs | Lecture | Écriture (copie vers) |
|---|---|---|---|
| `local` | `path` (dossier local, ex. NTFS monté) | oui | oui |
| `usb` | `path` (clé/disque amovible) | oui | oui |
| `smb` | `host`, `share`, `user`, `password`, `base_path?` (NAS) | oui | oui |
| `http` | `base_url?` (m3u/m3u8 ou fichier direct) | oui | non |
| `upnp` | `url` (serveur DLNA découvert) | oui | non |
| `radio` | `url` (flux webradio) | oui | non |
| `archive` | — (Internet Archive, audio libre, sans compte — #84) | oui | non |
| `deezer` | — (extraits 30 s, API publique keyless — #85) | oui (30 s) | non |
| `jamendo` | `client_id?` (gratuit, developer.jamendo.com — #86) | oui (CC) | non |
| `qobuz` / `tidal` | credentials perso (#25) | oui | non |
| `linein` | `device?` (entrée audio live — #75) | oui | non |

- **USB** : bouton « Détecter les disques USB » (`GET /api/usb/devices`) liste
  les montages amovibles connus de l'OS (`/media`, `/run/media`, `/mnt` sous
  Linux, `/Volumes` sous macOS, lettres de lecteur sous Windows hors disque
  système). On peut aussi saisir le chemin à la main.
- Les sources sont **persistées** dans `<data-dir>/config.toml` à chaque
  ajout/suppression/activation (`POST`/`DELETE`/`PATCH /api/sources`).
- Un `id` en double est refusé (HTTP 400).
- **SMB / NAS** : laisser `user` vide = connexion « invité » (guest), souvent en
  lecture seule. Si le NAS répond « session guest » alors que vous avez saisi un
  utilisateur, c'est que le couple utilisateur/mot de passe est refusé
  (`map to guest = bad user` côté Samba) : vérifiez le compte sur le NAS, que le
  service SMB est actif et que le port 445 est ouvert
  (`Test-NetConnection 192.168.50.129 -Port 445` sous Windows). Le partage doit
  aussi accorder les droits à l'utilisateur dans ses permissions.

## Navigation dans l'arborescence

- `GET /api/browse?uri=src://<id>/<dossier>` : contenu d'un dossier
  (`{name, uri, is_dir, is_audio, size, format}`).
- `GET /api/browse/tree?uri=...` : parcours récursif (fichiers audio ; si
  l'URI est un fichier, il se retourne lui-même).
- Lecture d'un dossier entier : `POST /api/player/play {"uri": "src://..."}`
  ou `POST /api/queue/add {"uri": ...}` — le dossier est élargi en file.
- Les URI `src://` acceptent les espaces bruts ou l'encodage percent (`%20`).

## Import / export

- **Copier entre sources** : `POST /api/transfer/copy {"from": "src://nas/1970", "to": "src://cle/import"}`
  → `{files, bytes, dirs, errors}`. Récursif pour les dossiers. Refusé (400)
  si la destination est en lecture seule (radio/HTTP/UPnP) ou si le chemin
  contient `..`.
- **Exporter en ZIP** : `GET /api/export?uri=src://nas/1970&name=1970`
  (fichier, dossier complet avec pochettes, ou album via
  `/api/library/albums/{id}/export`). ZIP « stored » écrit en pure Rust
  (aucune dépendance C), vérifié par `zipfile` côté tests.

## Playlists

`GET/POST /api/playlists`, `GET/DELETE /api/playlists/{id}`,
`POST /api/playlists/{id}/play`. `POST /api/playlists {"name": ...}` sans
`track_ids` sauvegarde la **file d'attente courante** (les pistes hors scan y
sont indexées à la volée pour pouvoir être rejouées).
