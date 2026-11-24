# Politique de sécurité

## Signaler une vulnérabilité

**Ne créez pas d'issue publique pour une faille de sécurité.**

Utilisez l'onglet **Security → Advisories → "Report a vulnerability"** de
https://github.com/mjcc30/auris — la divulgation est privée et coordonnée
(90 jours pour corriger avant publication).

## Ce que couvre Auris

- Le binaire et les crates Rust du dépôt
- L'interface web (API REST, WebSocket) exposée sur le LAN
- Le système de mise à jour (intégrité des binaires téléchargés : sommes SHA-256
  des assets de release, vérifiées par `self_update` ; signatures cryptographiques
  prévus en v1.1 via Sigstore)
- Le système de plugins (un plugin est un processus externe : voir
  [docs/PLUGIN_SYSTEM.md](docs/PLUGIN_SYSTEM.md) pour le bac à sable)

## Bonnes pratiques côté déploiement

- Par défaut, Auris écoute sur `0.0.0.0:3000` pour être joignable depuis le LAN.
  Sur un réseau non fiable, restreignez : `auris serve --host 127.0.0.1` + tunnel SSH,
  ou ajoutez un reverse proxy avec authentification (v1.1 ajoutera un token natif).
- Le fichier `config.toml` contient les identifiants SMB du NAS : il est créé avec
  les permissions `600` (owner seul). Ne le commitez jamais.
- Aucune télémétrie : Auris ne contacte que votre NAS et (si activé) l'API GitHub
  pour les mises à jour.
