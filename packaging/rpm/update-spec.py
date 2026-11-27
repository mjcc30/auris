#!/usr/bin/env python3
"""Met à jour packaging/rpm/auris.spec pour une release donnée.

Usage : python3 packaging/rpm/update-spec.py v0.8.0
Réécrit la ligne `Version:` du spec (le numéro de version des sources).
"""
import re
import sys

SPEC = "packaging/rpm/auris.spec"


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    version = sys.argv[1].lstrip("v")
    src = open(SPEC, encoding="utf-8").read()
    src, n = re.subn(r"^Version:\s+.*$", f"Version:        {version}", src, count=1, flags=re.M)
    if n != 1:
        print(f"ERREUR: ligne Version introuvable dans {SPEC}", file=sys.stderr)
        return 1
    open(SPEC, "w", encoding="utf-8").write(src)
    print(f"{SPEC}: Version={version}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
