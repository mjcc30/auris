# Contribuer à Auris

Merci de votre intérêt. Ce projet suit une discipline stricte mais simple —
elle tient en quatre règles : **commits conventionnels, TDD, couverture, revue**.

## 1. Mise en place

```bash
git clone https://github.com/<vous>/auris.git && cd auris
cargo test            # tout doit passer avant de commencer
cargo install cocogitto cargo-llvm-cov   # outils de CI en local (optionnel)
```

## 2. Workflow

1. Choisissez un ticket sur le [kanban](docs/BOARD.md) (labels `good first issue` / `help wanted` pour démarrer).
2. Créez une branche nommée `type/court-desc` : `feat/lecture-smb`, `fix/seek-dsd`, `docs/api`...
3. **TDD** : écrivez le test qui échoue, puis le code qui le fait passer (red → green → refactor).
   - Tout nouveau code logique dans `auris-core`, `auris-sources`, `auris-player`, `auris-library`, `auris-web`, `auris-updater` doit arriver **avec** ses tests (`#[cfg(test)]` ou `tests/`).
   - `cargo test` doit être vert sur votre branche, sans exception.
4. Ouvrez une PR vers `main` (voir `.github/PULL_REQUEST_TEMPLATE.md`).

## 3. Commits conventionnels (obligatoire)

Format : `type(scope): sujet` — sujet en français ou anglais, impératif, < 72 caractères.

| Type | Usage |
|---|---|
| `feat` | nouvelle fonctionnalité |
| `fix` | correction de bug |
| `docs` | documentation seule |
| `style` | formatage (rustfmt) sans changement de logique |
| `refactor` | refactoring sans changement de comportement |
| `perf` | amélioration de performance |
| `test` | ajout/modification de tests seuls |
| `build` | dépendances, build, CI |
| `ci` | workflows GitHub |
| `chore` | tâche diverses |
| `revert` | annulation d'un commit |

Exemples : `feat(player): décodage DFF -> DoP`, `fix(web): 404 sur /api/cover/{id}`.

La CI vérifie chaque commit de la PR avec **cocogitto** (`cog check`). Un commit
non conforme bloque la fusion. En local : `cog check`, `cog bump --auto` pour
les versions.

## 4. Qualité & couverture

- `cargo fmt --check` et `cargo clippy --all-targets -- -D warnings` doivent passer (CI).
- **Couverture** : `cargo llvm-cov --lcov --output-path lcov.info` ; minimum **60 %**
  sur les crates non-UI, objectif 80 % sur `auris-core`. La CI publie le rapport en
  artifact et commente la PR.
- UI (`ui/`) : tests Vitest pour la logique d'état (lecteur, WebSocket) ; `npm test`.

## 5. Revue

- Une PR = un sujet. Pas de "tant qu'à faire".
- 1 approbation requise ; les mainteneurs mergent en **squash** (le message final
  reste un commit conventionnel grâce au titre de la PR — nommez-le en conséquence).
- Le ticket lié passe en `status/review` à l'ouverture, `status/done` à la fusion.

## 6. Plugins communautaires

Toute contribution de plugin doit être sous **licence compatible AGPL v3** et suivre
le manifeste de [docs/PLUGIN_SYSTEM.md](docs/PLUGIN_SYSTEM.md). Les plugins sont
déposés dans leur propre repo et référencés dans le registre communautaire.

## 7. Signaler un bug / une idée

Ouvrez une issue avec le template adapté (bug / feature / plugin). Pas de bug
de sécurité en public : voir [SECURITY.md](SECURITY.md).
