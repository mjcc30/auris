## Quoi ?

<!-- Résumé en une phrase. Le titre de la PR doit être un commit conventionnel :
     feat(scope): ..., fix(scope): ... -->

Closes #

## Type de changement

- [ ] `feat` fonctionnalité
- [ ] `fix` bug
- [ ] `docs` documentation
- [ ] `refactor` / `perf` / `test` / `ci` / `chore` / `style` / `build`

## Comment tester ?

<!-- Commandes / étapes de vérification manuelle -->

```bash
cargo test
```

## Checklist

- [ ] `cargo fmt --check` passe
- [ ] `cargo clippy --all-targets -- -D warnings` passe
- [ ] `cargo test` passe (TDD : tests ajoutés avec le code)
- [ ] Couverture non dégradée (`cargo llvm-cov`)
- [ ] `cog check` passe (commits conventionnels)
- [ ] Doc mise à jour si besoin (docs/, README)
- [ ] Ticket kanban passé en `status/review`
