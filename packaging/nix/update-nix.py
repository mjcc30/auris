#!/usr/bin/env python3
"""Met à jour packaging/nix/flake.nix pour une release donnée.

Usage : python3 packaging/nix/update-nix.py v0.8.0
Réécrit `version = "..."` et les sha256 des trois assets (x86_64-linux,
aarch64-linux, aarch64-darwin) en les téléchargeant.
"""
import hashlib
import re
import sys
import urllib.request

REPO = "mjcc30/auris"
FLAKE = "packaging/nix/flake.nix"
ASSETS = {
    "x86_64-linux": "auris-x86_64-unknown-linux-gnu.tar.gz",
    "aarch64-linux": "auris-aarch64-unknown-linux-gnu.tar.gz",
    "aarch64-darwin": "auris-aarch64-apple-darwin.tar.gz",
}


def sha256_of(url: str) -> str:
    h = hashlib.sha256()
    with urllib.request.urlopen(urllib.request.Request(url, headers={"User-Agent": "auris-packaging"})) as r:
        while True:
            b = r.read(1 << 20)
            if not b:
                break
            h.update(b)
    return h.hexdigest()


def main() -> int:
    tag = sys.argv[1] if len(sys.argv) > 1 else "latest"
    if tag == "latest":
        import json

        req = urllib.request.Request(
            f"https://api.github.com/repos/{REPO}/releases/latest",
            headers={"User-Agent": "auris-packaging"},
        )
        tag = json.load(urllib.request.urlopen(req))["tag_name"]
    version = tag.lstrip("v")

    src = open(FLAKE, encoding="utf-8").read()
    src = re.sub(r'^  version = "[^"]+";', f'  version = "{version}";', src, count=1, flags=re.M)
    for system, fname in ASSETS.items():
        url = f"https://github.com/{REPO}/releases/download/{tag}/{fname}"
        sha = sha256_of(url)
        # réécrit le bloc de l'asset concerné : file inchangé, sha256 + url versionnée
        pat = re.compile(
            r'(\n\s+' + re.escape(system) + r' = \{\n\s+file = "' + re.escape(fname) + r'";\n\s+sha256 = ")[0-9a-f]{64}(";)'
        )
        src, n = pat.subn(rf"\g<1>{sha}\g<2>", src, count=1)
        if n != 1:
            print(f"ERREUR: bloc {system} introuvable dans {FLAKE}", file=sys.stderr)
            return 1
        print(f"{system}: {fname} sha256={sha}")
    open(FLAKE, "w", encoding="utf-8").write(src)
    print(f"{FLAKE}: version={version}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
