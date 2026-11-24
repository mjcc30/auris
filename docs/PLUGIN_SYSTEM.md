# Système de plugins communautaires

Objectif : un écosystème façon Volumio — n'importe qui peut ajouter une source
(Qobuz, Tidal, Last.fm, radios), une sortie, un traitement — **sans recompiler
Auris et dans n'importe quel langage**.

## Principes

1. **Un plugin = un processus externe + un manifeste.** Le cœur lance le binaire
   du plugin, parle avec lui en **JSON-RPC 2.0 sur stdio** (lignes JSON), et ne
   lui fait jamais confiance aveuglément (timeout, tailles plafonnées).
2. **Déclaratif.** `auris-plugin.toml` décrit le plugin, ses capacités, sa
   configuration (schéma JSON) — l'UI génère le formulaire de réglage.
3. **Licence compatible AGPL v3 obligatoire** (le manifeste doit le déclarer).
4. **Registre communautaire** : un dépôt `mjcc30/auris-plugins` avec un
   `index.json` ; installation en une commande/API.

## Manifeste `auris-plugin.toml`

```toml
[plugin]
id = "qobuz"
name = "Qobuz"
version = "0.2.1"
category = "source"            # source | output | metadata | ui
authors = ["Jean Dupont <jean@example.org>"]
license = "AGPL-3.0-only"      # doit être compatible AGPL
homepage = "https://github.com/jean/aurius-qobuz"
description = "Source Qobuz (abonnement requis)"
min_auris = "0.4.0"

[install]
# asset binaire ou archive a telecharger depuis la release du plugin
asset_url = "https://github.com/jean/auris-qobuz/releases/download/v0.2.1/plugin-linux-x86_64.tar.gz"
sha256 = "..."
bin = "auris-plugin-qobuz"    # binaire lance par le coeur

[config_schema]                # formulaire auto dans l'UI (JSON Schema)
type = "object"
properties = { token = { type = "string", title = "Token Qobuz", "x-secret" = true } }
```

## Protocole (v1) — JSON-RPC 2.0 sur stdio

Le cœur envoie des requêtes, le plugin répond (une ligne JSON par message) :

| Méthode | Rôle |
|---|---|
| `manifest.get` | renvoie le manifeste (pour vérification) |
| `capabilities.list` | liste les capacités déclarées |
| `source.browse` `{path}` | liste un dossier (équivalent `/api/browse`) |
| `source.open` `{uri}` → `handle` | ouvre un flux, renvoie un handle |
| `stream.read` `{handle, offset, len}` → bytes (base64) | lecture par plage |
| `stream.close` `{handle}` | ferme |
| `metadata.enrich` `{uri}` | métadonnées additionnelles (optionnel) |
| `output.push` / `output.set_volume` (category=output) | sortie audio custom |
| `ui.menu` (category=ui) | entrées de menu pour l'UI web |

Réponses d'erreur JSON-RPC standard (`{"error":{"code":..,"message":..}}`).
Timeout par défaut 10 s ; processus tué si non conforme (pas de JSON valide).

## Bac à sable (v1)

- Processus fils avec env nettoyé (pas de secrets du cœur), répertoire de travail
  `<data_dir>/plugins/<id>/`.
- Pas d'accès direct à la base sled ni au réseau du cœur ; le réseau est laissé
  au plugin (il en a besoin) mais journalisé (dns/logs) en mode debug.
- Taille max message : 8 Mo. Un plugin qui déborde est désactivé.

## Cycle de vie

`install` (télécharge + vérifie sha256 + rend exécutable) → `enable` →
`start` (processus lancé, handshake `manifest.get`) → `stop` → `disable` →
`uninstall`. L'UI expose tout ça dans **Réglages → Plugins**.

## Catégories

- `source` : fournit musique/ métadonnées (ex. Qobuz, Last.fm scrobble, radios).
- `output` : sortie audio alternative (ex. Chromecast, AirPlay, Roon endpoint).
- `metadata` : enrichit la bibliothèque (paroles, crédits, pochettes HD).
- `ui` (v1.1) : composant web embarqué en iframe bac à sable (`sandbox="allow-scripts"`).

## SDK

Un SDK Rust (`auris-plugin-sdk`, crate à venir) fournit le squelette stdio ;
des exemples en Python et Go seront fournis dans `examples/plugins/`.
