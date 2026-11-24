# ADR-0001 — Monorepo, workspace Cargo unique

- Statut : accepté
- Date : 2026-11-24

## Contexte

Un seul binaire distribuable, mais un code propre et testable par morceau.
Faut-il un repo par composant ou un monorepo ?

## Décision

Monorepo `mjcc30/auris`, workspace Cargo (`crates/*` + `ui/`), un seul binaire
`auris` en sortie. Les releases GitHub servent les binaires par cible.

## Conséquences

- + releases simples (un tag = un binaire par plateforme)
- + CI unique, couverture globale, refactorings cross-crates faciles
- + UI embarquée dans le binaire (rust-embed sur `ui/dist`)
- − dépôt plus gros ; mitigé par `.gitignore` (target/, node_modules/)
