#!/usr/bin/env python3
"""Bump the pinned runtime tools in runtime-tools.json to their latest GitHub releases.

Prints one markdown bullet per bump and nothing when every tool is current.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import sys
import urllib.request

TABLE = Path(__file__).with_name("runtime-tools.json")


def pick_update(entry, release):
    matches = [asset for asset in release["assets"] if re.fullmatch(entry["asset_pattern"], asset["name"])]
    if len(matches) != 1:
        names = ", ".join(asset["name"] for asset in matches) or "none"
        raise RuntimeError(f"{entry['repo']} {release['tag_name']}: expected one asset matching {entry['asset_pattern']}, found {names}")
    asset, tag = matches[0], release["tag_name"]
    if (tag, asset["name"]) == (entry["tag"], entry["asset"]):
        return None
    return {
        "tag": tag,
        "asset": asset["name"],
        "url": f"https://github.com/{entry['repo']}/releases/download/{tag}/{asset['name']}",
        "digest": asset.get("digest"),
    }


def request(url, accept):
    headers = {"Accept": accept, "User-Agent": "runtime-tools-updater"}
    token = os.environ.get("GITHUB_TOKEN")
    if token and url.startswith("https://api.github.com/"):
        headers["Authorization"] = f"Bearer {token}"
    return urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=120)


def sha256_of(url):
    digest = hashlib.sha256()
    with request(url, "application/octet-stream") as response:
        while chunk := response.read(1 << 20):
            digest.update(chunk)
    return digest.hexdigest()


def main():
    sys.stdout.reconfigure(encoding="utf-8")
    tools = json.loads(TABLE.read_text(encoding="utf-8"))
    bullets = []
    for name, entry in tools.items():
        with request(f"https://api.github.com/repos/{entry['repo']}/releases/latest", "application/vnd.github+json") as response:
            release = json.load(response)
        update = pick_update(entry, release)
        if update is None:
            continue
        actual = sha256_of(update["url"])
        expected = update["digest"]
        if expected and expected.startswith("sha256:") and expected[len("sha256:"):] != actual:
            raise RuntimeError(f"{name}: downloaded SHA-256 {actual} does not match GitHub's {expected}")
        old_tag = entry["tag"]
        entry.update(tag=update["tag"], asset=update["asset"], sha256=actual)
        bullets.append(f"- {name}: {old_tag} → {update['tag']} (https://github.com/{entry['repo']}/releases/tag/{update['tag']})")
    if bullets:
        TABLE.write_text(json.dumps(tools, indent=2) + "\n", encoding="utf-8", newline="\n")
        print("\n".join(bullets))


if __name__ == "__main__":
    main()
