#!/usr/bin/env python3
"""Met à jour packaging/brew/auris.rb pour une release donnée (version + sha256).

Usage : python3 packaging/brew/update-formula.py v0.8.0
Le sha256 est calculé en téléchargeant l'asset aarch64-apple-darwin de la release.
"""
import hashlib
import re
import sys
import urllib.request

REPO = "mjcc30/auris"
FORMULA = "packaging/brew/auris.rb"
ASSET = "auris-aarch64-apple-darwin.tar.gz"


def main() -> int:
    tag = sys.argv[1] if len(sys.argv) > 1 else "latest"
    if tag == "latest":
        req = urllib.request.Request(
            f"https://api.github.com/repos/{REPO}/releases/latest",
            headers={"User-Agent": "auris-packaging"},
        )
        import json

        tag = json.load(urllib.request.urlopen(req))["tag_name"]
    version = tag.lstrip("v")
    url = f"https://github.com/{REPO}/releases/download/{tag}/{ASSET}"
    h = hashlib.sha256()
    with urllib.request.urlopen(urllib.request.Request(url, headers={"User-Agent": "auris-packaging"})) as r:
        while True:
            b = r.read(1 << 20)
            if not b:
                break
            h.update(b)
    sha = h.hexdigest()

    src = open(FORMULA, encoding="utf-8").read()
    src = re.sub(r'version "[^"]+"', f'version "{version}"', src, count=1)
    src = re.sub(
        r"releases/download/v[^/]+/" + re.escape(ASSET),
        f"releases/download/{tag}/{ASSET}",
        src,
        count=1,
    )
    src = re.sub(r'sha256 "[0-9a-f]{64}"', f'sha256 "{sha}"', src, count=1)
    open(FORMULA, "w", encoding="utf-8").write(src)
    print(f"{FORMULA}: version={version} sha256={sha} ({ASSET})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
