# Non-features volontaires — ce qu'Auris ne fera pas, et pourquoi

Auris est un lecteur audiophile **local et ouvert** (AGPL v3). Certaines fonctions
courantes dans l'audio connecté sont volontairement absentes. Cette page documente
ces choix, leurs raisons, et les alternatives ouvertes quand elles existent.

> Principe directeur : **pas de DRM, pas d'API sous licence propriétaire,
> pas de dépendance cloud obligatoire**. Tout ce qui peut être fait 100 % local
> et ouvert est privilégié ; le reste est documenté ici plutôt que promis.

## Tableau récapitulatif

| Fonction | Pourquoi Auris ne la fait pas | Alternative / ticket |
|---|---|---|
| Roon Ready / RAAT | Protocole RAAT fermé (licence Roon Labs). Incompatible AGPL et philosophie local-first. | Lecture réseau ouverte : AirPlay 2 + Chromecast + DLNA en cours (#28). |
| HQPlayer NAA (Network Audio Adapter) | NAA est un binaire propriétaire distribué par Signalyst. | Équivalent ouvert : sortie vers renderer DLNA/UPnP tiers, ou mode « jouer ici » navigateur (fait). |
| MQA (TIDAL Masters) | MQA est un codec propriétaire sous licence ; le décodage « full » nécessite un DAC certifié MQA. | FLAC 24 bit Hi-Res identique en qualité perçue, sans licence. TIDAL Hi-Res FLAC via API communautaire : #25. |
| Alexa / Google Assistant intégrés | Assistants vocaux = cloud propriétaire, micros toujours à l'écoute, données personnelles envoyées à des tiers (RGPD). | Commandes locales : API REST + WebSocket ouverte, app mobile ou télécommande IR/GPIO prévue (#43). |
| Lecture offline des catalogues Qobuz/TIDAL/Spotify | Le DRM de ces services interdit légalement le stockage local. Aucun logiciel ouvert ne peut le faire. | Bibliothèque locale propre (scan NAS/USB) + services avec compte perso joués en streaming. |
| Multi-utilisateurs avec comptes | Un lecteur domestique local n'a pas de modèle de comptes ; l'authentification ajoute une surface d'attaque inutile. | Un seul profil local ; multi-utilisateurs avec token optionnel étudié pour v1.0 (#97). |
| Vidéo / karaoké | Auris est un lecteur **audio** ; la vidéo sort du périmètre (pas de décodeur vidéo, pas d'UI TV). | Sortie HDMI audio seule prévue (#42) ; la vidéo reste du ressort de lecteurs dédiés. |
| Super search sémantique assistée par IA (Volumio Premium) | Envoi des requêtes à un LLM cloud = vie privée + dépendance. | Recherche locale plein texte + facettes (genres, décennies) faite ; recherche multi-sources locale à l'étude. |
| Cloud backup automatique de la config | Backup cloud = copie de la config sur serveurs tiers. | Export/import local de la bibliothèque et des réglages (ZIP) fait. |
| App iOS/Android native dédiée | L'UI web responsive + API native couvrent le besoin ; une app native est un coût de maintenance doublé. | PWA installable prévue v1.0 (#95). |

## Détails et nuances

### Roon Ready / RAAT — non, par licence

RAAT (Roon Advanced Audio Transport) est le protocole propriétaire qui fait la
force de Roon. Il n'est ni ouvert ni libre, et son implémentation dans un projet
AGPL créerait un conflit de licence. Auris mise sur les protocoles **ouverts** :
AirPlay 2, Chromecast (Cast), DLNA/UPnP (#28), et le multiroom synchronisé ouvert
style Snapcast (#23, #94). Un appareil Auris reste utilisable comme **source**
depuis Roon (sortie réseau standard) si l'utilisateur possède déjà un serveur Roon.

### HQPlayer NAA — non, logiciel propriétaire

Le « Network Audio Adapter » de HQPlayer est un binaire fermé distribué par
Signalyst. Auris ne le distribuera pas. Côté client, Auris vise le **bit-perfect
direct** (WASAPI exclusif sous Windows, ALSA/ASIO ailleurs) plutôt que le
resampling réseau d'un logiciel tiers.

### MQA — non, codec sous licence

Le décodage MQA complet requiert un matériel certifié ( unfold matériel ). Auris
lit les fichiers Hi-Res FLAC 24/96 et 24/192 — même information musicale, sans
licence ni royalties. Le support TIDAL Hi-Res (FLAC) via l'API communautaire avec
compte personnel reste prévu (#25) ; le « MQA » de TIDAL ne sera pas décodé.

### Assistants vocaux — non, vie privée

Intégrer Alexa ou Google Home signifie : micro piloté par un cloud tiers,
enregistrement et traitement des voix sur des serveurs extérieurs, conditions
RGPD complexes. Auris reste pilotable localement : API REST/WebSocket ouverte,
appareil mobile sur le même réseau, télécommande IR/GPIO (#43), raccourcis clavier.

### DRM offline — non, illégal

Qobuz, TIDAL et Spotify chiffrent leurs catalogues (Widevine/FairPlay). Stocker
des morceaux hors ligne depuis ces services est interdit par leurs CGU et protégé
par des DRM ; aucun projet ouvert ne le fait. Auris ne tentera jamais de contourner
ces protections. Ce qui est légal et supporté : votre **propre** bibliothèque
(fichiers achetés sans DRM, CD ripés — #27 rip CD), servie depuis un NAS ou un
disque USB.

### Vidéo — hors périmètre

Auris ne lit pas la vidéo, ne fait pas de karaoké, et l'UI n'est pas pensée pour
un écran TV. La sortie HDMI prévue (#42) sert uniquement l'**audio** (et un mode
affichage « de loin » pochette + horloge, déjà fait).

## Ce qui est prévu à la place

Voir le [kanban](https://github.com/mjcc30/auris/projects) et la
[roadmap](ROADMAP.md) : AirPlay 2 / Cast / DLNA (#28), Spotify Connect via
librespot (#24), TIDAL/Qobuz Hi-Res via API communautaire (#25), multiroom
ouvert (#23/#94), PWA installable (#95), i18n complet fr/en (#96), smart
playlists à règles (#98).
