# Kanban — comment on travaille

Board GitHub Projects v2 : **https://github.com/users/mjcc30/projects/2**
/vue **Kanban** (layout board, colonnes = champ « Statut »).

## Colonnes (champ « Statut »)

| Colonne | Signification |
|---|---|
| Backlog | idée/ticket non priorisé |
| Ready | prêt à démarrer (Definition of Ready remplie) |
| In progress | quelqu'un travaille dessus (1 ticket par personne max) |
| Review | PR ouverte, en revue |
| Done | mergé / livré |
| Blocked | bloqué, préciser pourquoi en commentaire |

## Règles

- **Labels = source de vérité** : chaque issue/PR a un label `status/*` qui reflète
  sa colonne (les deux sont maintenus à la main ; en cas de divergence, le label gagne).
- **Definition of Ready** : ticket en `Ready` = user story claire, critères
  d'acceptation, estimation (S/M/L), pas de dépendance bloquante.
- **Definition of Done** : code mergé sur `main`, tests verts en CI, couverture
  maintenue, doc à jour, ticket passé en `Done`.
- **Triage** : revue du backlog chaque lundi (30 min) — on priorise, on labellise
  `prio/p0..p3`, on met des tickets en `Ready`.
- WIP limit : 1 ticket `In progress` par personne.

## Liens utiles

- Tout ouvert : https://github.com/mjcc30/auris/issues
- En cours : https://github.com/mjcc30/auris/issues?q=label%3Astatus%2Fin-progress
- En revue : https://github.com/mjcc30/auris/pulls?q=label%3Astatus%2Freview
- Par jalon : milestones sur https://github.com/mjcc30/auris/milestones
- Bon premier ticket : https://github.com/mjcc30/auris/issues?q=label%3A%22good+first+issue%22
