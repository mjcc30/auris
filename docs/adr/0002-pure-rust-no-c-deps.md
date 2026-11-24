# ADR-0002 — 100 % Rust, aucune dépendance C compilée

- Statut : accepté
- Date : 2026-11-24

## Contexte

Cibles : Raspberry Pi (aarch64), Linux, macOS, Windows. La cross-compilation
(aarch64-linux depuis x86_64, Windows depuis Linux en CI) échoue dès qu'une
dépendance compile du C (ring, aws-lc, rusqlite bundled...).

## Décision

Aucune crate nécessitant un compilateur C :
- TLS via `native-tls` (SChannel/SecureTransport/OpenSSL système)
- base embarquée `sled` (pas rusqlite)
- SMB2/3 pur Rust (`smb2`), HTTP via `reqwest` + `native-tls`
- mise à jour `self_update` avec features `native-tls` (pas rustls/ring)

Règle pour toute nouvelle dépendance : vérifier son arbre de dépendances
(`cargo tree`) avant ajout.

## Conséquences

- + build reproductible sur les 5 cibles en CI sans toolchain C
- − TLS système = dépendance à l'OS (accepté : OS maintenus)
- − pas de SQLite (sled suffit pour un index local)
