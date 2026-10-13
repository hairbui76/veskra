#!/usr/bin/env python3
"""Copy the newest Veskra release's APKs into the website so they download from the site's own
domain (GitHub's release file host sends no CORS headers, so a browser cannot fetch them directly).

Writes into <out_dir> (default site/download):
  veskra-arm64-v8a.apk, veskra-x86_64.apk, veskra-universal.apk  stable names for the page's links
  SHA256SUMS.txt                                                  as published with the release
  latest.json   tag, release URL, publish date, release notes, and per asset: file, versioned name,
                size, sha256 (also read by the app's "Check for updates", ADR-020)

"Newest" includes pre-releases (GitHub's /releases/latest skips them). Uses the GitHub CLI (`gh`),
authenticated through GH_TOKEN on CI. Checksums are verified against the release's SHA256SUMS.txt.
"""

import hashlib
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = "hairbui76/veskra"
ABIS = ("arm64-v8a", "x86_64", "universal")


def gh(*args: str) -> str:
    return subprocess.run(["gh", *args], check=True, capture_output=True, text=True).stdout


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main() -> int:
    out = Path(sys.argv[1] if len(sys.argv) > 1 else "site/download")
    releases = json.loads(gh("api", f"repos/{REPO}/releases?per_page=20"))
    release = next(
        (r for r in releases if not r["draft"] and any(a["name"].endswith(".apk") for a in r["assets"])),
        None,
    )
    if release is None:
        print("no release with APKs yet; nothing to bundle", file=sys.stderr)
        return 1
    tag = release["tag_name"]

    with tempfile.TemporaryDirectory() as tmp:
        gh("release", "download", tag, "-R", REPO, "-D", tmp, "-p", "*.apk", "-p", "SHA256SUMS.txt")
        files = {p.name: p for p in Path(tmp).iterdir()}
        sums_file = files.get("SHA256SUMS.txt")
        published = {}
        if sums_file:
            for line in sums_file.read_text().splitlines():
                digest, _, name = line.strip().partition(" ")
                published[name.lstrip(" *")] = digest

        if out.exists():
            shutil.rmtree(out)
        out.mkdir(parents=True)
        assets = {}
        for abi in ABIS:
            name = next((n for n in files if n.endswith(f"-{abi}.apk")), None)
            if name is None:
                continue
            digest = sha256(files[name])
            if name in published and published[name] != digest:
                print(f"checksum mismatch for {name}", file=sys.stderr)
                return 1
            stable = f"veskra-{abi}.apk"
            shutil.copyfile(files[name], out / stable)
            assets[abi] = {"file": stable, "name": name, "size": files[name].stat().st_size, "sha256": digest}
        if not assets:
            print(f"release {tag} has no recognised APKs", file=sys.stderr)
            return 1
        if sums_file:
            shutil.copyfile(sums_file, out / "SHA256SUMS.txt")
            assets["sums"] = {"file": "SHA256SUMS.txt", "name": f"veskra-{tag.lstrip('v')}-SHA256SUMS.txt", "size": sums_file.stat().st_size}

    latest = {
        "tag": tag,
        "url": release["html_url"],
        "published": release["published_at"] or release["created_at"],
        "prerelease": release["prerelease"],
        # Shown in the app's update dialog; capped so latest.json stays small.
        "notes": (release.get("body") or "")[:4000],
        "assets": assets,
    }
    (out / "latest.json").write_text(json.dumps(latest, indent=2) + "\n")
    print(f"bundled {tag}: " + ", ".join(f"{a['file']} ({a['size']} B)" for a in assets.values()))
    return 0


if __name__ == "__main__":
    sys.exit(main())
