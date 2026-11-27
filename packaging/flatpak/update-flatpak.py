#!/usr/bin/env python3
"""Met à jour packaging/flatpak/audio.auris.Auris.yml pour une release donnée.

Usage : python3 packaging/flatpak/update-flatpak.py v0.8.0
Réécrit l'url de la source archive et son sha256 (asset linux x86_64).
"""
import hashlib
import re
import sys
import urllib.request

REPO = "mjcc30/auris"
MANIFEST = "packaging/flatpak/audio.auris.Auris.yml"
ASSET = "auris-x86_64-unknown-linux-gnu.tar.gz"


def main() -> int:
    tag = sys.argv[1] if len(sys.argv) > 1 else "latest"
    if tag == "latest":
        import json

        req = urllib.request.Request(
            f"https://api.github.com/repos/{REPO}/releases/latest",
            headers={"User-Agent": "auris-packaging"},
        )
        tag = json.load(urllib.request.urlopen(req))["tag_name"]
    url = f"https://github.com/{REPO}/releases/download/{tag}/{ASSET}"
    h = hashlib.sha256()
    with urllib.request.urlopen(urllib.request.Request(url, headers={"User-Agent": "auris-packaging"})) as r:
        while True:
            b = r.read(1 << 20)
            if not b:
                break
            h.update(b)
    sha = h.hexdigest()

    src = open(MANIFEST, encoding="utf-8").read()
    src = re.sub(
        r"releases/download/v[^/]+/" + re.escape(ASSET),
        f"releases/download/{tag}/{ASSET}",
        src,
        count=1,
    )
    src = re.sub(r"^(\s+)sha256: [0-9a-f]{64}$", rf"\1sha256: {sha}", src, count=1, flags=re.M)
    open(MANIFEST, "w", encoding="utf-8").write(src)
    print(f"{MANIFEST}: {tag} sha256={sha} ({ASSET})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
