#!/usr/bin/env python3
"""Lint des artefacts de packaging (exécuté en CI, job `packaging-lint`).

Vérifie, sans les construire :
  - install.sh / uninstall.sh / build-image.sh : syntaxe bash (bash -n)
  - packaging/brew/auris.rb            : classe, url, sha256 64 hex, version
  - packaging/winget/*.yaml            : YAML valide, PackageIdentifier,
                                         PackageVersion, InstallerUrl, InstallerSha256
  - packaging/rpm/auris.spec           : tags Name/Version/Release/%files/%post
  - .github/workflows/release.yml     : jobs deb + rpm présents, le job build
                                         publie des checksums à nom unique
  - docs/install.md mentionne chaque méthode (script, .deb, .rpm, brew, winget, image)

Usage : python3 packaging/lint.py   (exit 1 si au moins une erreur)
"""
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
errors: list[str] = []


def err(msg: str) -> None:
    errors.append(msg)
    print(f"ERREUR: {msg}")


def ok(msg: str) -> None:
    print(f"ok: {msg}")


def check_bash(path: Path) -> None:
    # Normalise CRLF -> LF dans un temporaire (checkout Windows = CRLF,
    # runners Linux = LF ; bash -n exige LF).
    import tempfile

    src = path.read_bytes().replace(b"\r\n", b"\n")
    tmp_name = f".lint-{path.name}"
    tmp = ROOT / tmp_name
    tmp.write_bytes(src)
    try:
        r = subprocess.run(
            ["bash", "-n", tmp_name], capture_output=True, text=True, cwd=ROOT
        )
    finally:
        tmp.unlink(missing_ok=True)
    if r.returncode != 0:
        err(f"{path.relative_to(ROOT)}: bash -n a échoué: {r.stderr.strip()}")
    else:
        ok(f"{path.relative_to(ROOT)}: syntaxe bash OK")


def check_brew() -> None:
    p = ROOT / "packaging/brew/auris.rb"
    src = p.read_text(encoding="utf-8")
    if not re.search(r"^class Auris < Formula", src, re.M):
        err("auris.rb: classe `Auris < Formula` manquante")
    if not re.search(r'^\s*url "https://github\.com/mjcc30/auris/releases/download/v[^"]+/auris-aarch64-apple-darwin\.tar\.gz"', src, re.M):
        err("auris.rb: url de l'asset darwin manquante ou invalide")
    m = re.search(r'^\s*sha256 "([0-9a-f]{64})"', src, re.M)
    if not m:
        err("auris.rb: sha256 (64 hex) manquant")
    if not re.search(r'^\s*version "[0-9]+\.[0-9]+\.[0-9]+"', src, re.M):
        err("auris.rb: version semver manquante")
    if m and re.search(r'^\s*version "([^"]+)"', src, re.M):
        v = re.search(r'^\s*version "([^"]+)"', src, re.M).group(1)
        url = re.search(r'^\s*url "[^"]*/(v[^/"]+)/', src, re.M)
        if url and url.group(1) != "v" + v:
            err(f"auris.rb: version {v} != tag de l'url {url.group(1)}")
    if "depends_on" not in src:
        err("auris.rb: depends_on manquant")
    ok("auris.rb: formule cohérente") if not errors else None


def check_winget() -> None:
    import yaml

    p = ROOT / "packaging/winget/mjcc30.Auris.yaml"
    data = yaml.safe_load(p.read_text(encoding="utf-8"))
    if data.get("PackageIdentifier") != "mjcc30.Auris":
        err(f"winget: PackageIdentifier invalide: {data.get('PackageIdentifier')}")
    if not re.fullmatch(r"\d+\.\d+\.\d+", str(data.get("PackageVersion", ""))):
        err(f"winget: PackageVersion invalide: {data.get('PackageVersion')}")
    installers = data.get("Installers") or []
    if not installers:
        err("winget: aucun installateur")
    for inst in installers:
        url = inst.get("InstallerUrl", "")
        if "github.com/mjcc30/auris/releases/download/" not in url:
            err(f"winget: InstallerUrl invalide: {url}")
        tag = re.search(r"/download/(v[^/]+)/", url)
        if tag and tag.group(1) != "v" + str(data.get("PackageVersion")):
            err(f"winget: PackageVersion {data.get('PackageVersion')} != tag url {tag.group(1)}")
        if not re.fullmatch(r"[0-9a-f]{64}", str(inst.get("InstallerSha256", ""))):
            err("winget: InstallerSha256 (64 hex) manquant")
    ok("winget: manifeste cohérent") if not errors else None


def check_rpm_spec() -> None:
    p = ROOT / "packaging/rpm/auris.spec"
    src = p.read_text(encoding="utf-8")
    for tag in ("Name:", "Version:", "Release:", "License:", "Source0:", "%files", "%post", "%prep", "%install"):
        if tag not in src:
            err(f"auris.spec: tag/section manquant: {tag}")
    if not re.search(r"^Version:\s+\d+\.\d+\.\d+", src, re.M):
        err("auris.spec: Version semver invalide")
    if "/usr/local/bin/auris" not in src:
        err("auris.spec: %files ne contient pas /usr/local/bin/auris")
    ok("auris.spec: cohérent") if not errors else None


def check_workflows() -> None:
    import yaml

    rel = yaml.safe_load((ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8"))
    jobs = rel.get("jobs", {})
    for j in ("build", "deb", "rpm", "release"):
        if j not in jobs:
            err(f"release.yml: job manquant: {j}")
    if "rpm" in jobs:
        needs = jobs["release"].get("needs", [])
        if isinstance(needs, str):
            needs = [needs]
        if "rpm" not in needs:
            err("release.yml: le job release ne dépend pas de rpm")
    # bug checksums : noms de fichiers uniques par target
    build_steps = jobs.get("build", {}).get("steps", [])
    uploads = [
        s.get("with", {}).get("path", "")
        for s in build_steps
        if s.get("uses", "").startswith("actions/upload-artifact")
    ]
    if any(p == "checksums.txt" for p in uploads):
        err("release.yml: un artefact checksums.txt sans suffixe de target (écrasement au merge)")
    img = ROOT / ".github/workflows/image.yml"
    if not img.exists():
        err("image.yml: workflow image disque manquant")
    else:
        d = yaml.safe_load(img.read_text(encoding="utf-8"))
        if "build-image.sh" not in str(d.get("jobs", {})):
            err("image.yml: n'appelle pas packaging/image/build-image.sh")
    ok("workflows: cohérents") if not errors else None


def check_docs() -> None:
    p = ROOT / "docs/install.md"
    if not p.exists():
        err("docs/install.md manquant")
        return
    src = p.read_text(encoding="utf-8").lower()
    for token in ("install.sh", ".deb", ".rpm", "brew", "winget", "img"):
        if token not in src:
            err(f"docs/install.md: méthode d'installation non documentée: {token}")
    if not errors:
        ok("docs/install.md: toutes les méthodes documentées")


def main() -> int:
    for f in ("install.sh", "uninstall.sh", "packaging/image/build-image.sh"):
        p = ROOT / f
        if p.exists():
            check_bash(p)
        else:
            err(f"{f} manquant")
    check_brew()
    check_winget()
    check_rpm_spec()
    check_workflows()
    check_docs()
    if errors:
        print(f"\n{len(errors)} erreur(s)")
        return 1
    print("\npackaging-lint: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
