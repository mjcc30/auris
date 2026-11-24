# User stories — Auris

Format : *En tant que <rôle>, je veux <action> afin de <bénéfice>.*
Chaque story = un ticket kanban avec critères d'acceptation testables.

## Écoute

1. **En tant qu'audiophile avec un NAS**, je veux ajouter mon NAS en SMB dans
   l'interface web afin d'écouter ma bibliothèque sans la copier.
   *AC : source ajoutée via formulaire UI, scan lancé, pistes listées.*
2. **En tant qu'utilisateur de Raspberry Pi + DAC USB**, je veux brancher mon DAC
   et entendre du son sans configurer quoi que ce soit.
   *AC : `auris serve` détecte le DAC par défaut, lecture d'un FLAC OK.*
3. **En tant que propriétaire de fichiers DSD**, je veux écouter du DSD256.
   *AC : DSF 5,6 MHz lu, badge "DSD256" affiché, spectre DoP correct côté DAC.*
4. **En tant qu'auditeur**, je veux un gapless entre les pistes d'un album.
   *AC : aucune coupure entre deux FLAC 44,1 kHz enchaînés.*
5. **En tant qu'utilisateur**, je veux piloter la lecture depuis mon téléphone.
   *AC : depuis Safari/Chrome mobile, play/pause/next/volume fonctionnent, état
   temps réel via WebSocket.*

## Bibliothèque

6. **En tant qu'utilisateur**, je veux retrouver un morceau par son nom ou artiste.
   *AC : recherche < 300 ms sur 10 000 pistes, résultats triés par pertinence.*
7. **En tant qu'utilisateur**, je veux voir mes albums avec leurs pochettes.
   *AC : grille d'albums, pochette extraite des métadonnées, fallback icône.*
8. **En tant qu'utilisateur**, je veux naviguer dans les dossiers de mon NAS.
   *AC : arborescence dans l'UI, lecture d'un fichier au clic.*

## Administration

9. **En tant qu'admin de Raspberry**, je veux qu'Auris se mette à jour tout seul.
   *AC : nouvelle release GitHub détectée en < 6 h, appliquée via l'UI, binaire
   remplacé + checksum vérifié.*
10. **En tant qu'admin**, je veux qu'Auris démarre automatiquement au boot du Pi.
    *AC : `install.sh` crée l'unité systemd `auris.service`, active au boot.*
11. **En tant qu'admin**, je veux choisir le DAC utilisé quand plusieurs sont branchés.
    *AC : liste des DACs dans l'UI, sélection persistée dans config.toml.*

## Communauté

12. **En tant que développeur communautaire**, je veux écrire un plugin (ex. source
    Qobuz, scrobbler Last.fm) en Python sans toucher au Rust.
    *AC : plugin packagé avec `auris-plugin.toml`, installé depuis le registre,
    fonctionnel dans l'UI.*
13. **En tant que contributeur**, je veux savoir exactement comment contribuer.
    *AC : CONTRIBUTING.md lu, première PR mergée en < 1 semaine avec un
    `good first issue`.*
