#!/usr/bin/env python3
"""Met à jour packaging/windows/auris.iss pour une release donnée.

Usage : python3 packaging/windows/update-iss.py v0.8.0
Réécrit la ligne `#define MyAppVersion`.
"""
import re
import sys

ISS = "packaging/windows/auris.iss"


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    version = sys.argv[1].lstrip("v")
    src = open(ISS, encoding="utf-8").read()
    src, n = re.subn(
        r'^#define MyAppVersion ".*"$',
        f'#define MyAppVersion "{version}"',
        src,
        count=1,
        flags=re.M,
    )
    if n != 1:
        print(f"ERREUR: MyAppVersion introuvable dans {ISS}", file=sys.stderr)
        return 1
    open(ISS, "w", encoding="utf-8").write(src)
    print(f"{ISS}: MyAppVersion={version}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
