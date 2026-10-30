#!/usr/bin/env python3
"""Build a portable Windows ZIP with pinned, checksum-verified audio tools.

First cross-build BitChord.exe, then run this script with 7zz or 7z on PATH.
Set SEVENZIP to override the archive extractor. No machine-wide installation.
"""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[1]
CACHE = ROOT / "target/windows-runtime"
DIST = ROOT / "dist"
TOOLS = DIST / "tools"
EXECUTABLES = ("mpv.exe", "yt-dlp.exe", "ffmpeg.exe", "ffprobe.exe", "deno.exe")
ASSETS = {
    "mpv": (
        "https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/20261021/mpv-x86_64-20261021-git-e471496c56.7z",
        "4a355f3e89c83fabc730d33fb4e759dfec747806ed2287b61a90d1681b83e8d3",
    ),
    "ffmpeg": (
        "https://github.com/GyanD/codexffmpeg/releases/download/9.0.2/ffmpeg-9.0.2-essentials_build.7z",
        "4705843ccaaf54257c16ad90f3e952ece33c17df964ecf7bfdbb0f49c7171077",
    ),
    "yt-dlp": (
        "https://github.com/yt-dlp/yt-dlp/releases/download/2026.10.26/yt-dlp.exe",
        "f5389d3698ecde4ef06f4e6b1ce0294fe9ac1396228c8990b8d3416d6d177a7d",
    ),
    "deno": (
        "https://github.com/denoland/deno/releases/download/v2.9.8/deno-x86_64-pc-windows-msvc.zip",
        "cba84b26e0df5ae7f74fc66b86a95b866872a86292ebd84fa1e959927edd93bf",
    ),
}


def fetch(url, digest):
    path = CACHE / url.rsplit("/", 1)[1]
    if not path.exists():
        partial = path.with_suffix(path.suffix + ".part")
        print("Downloading", path.name, flush=True)
        with urllib.request.urlopen(url, timeout=120) as response, partial.open("wb") as output:
            shutil.copyfileobj(response, output)
        partial.replace(path)
    actual = hashlib.sha256(path.read_bytes()).hexdigest()
    if actual != digest:
        raise RuntimeError(f"Checksum mismatch for {path}: {actual}")
    return path


def main():
    binary = ROOT / "target/x86_64-pc-windows-gnu/release/bitchord-desktop.exe"
    if not binary.is_file():
        binary = ROOT / "target/release/bitchord-desktop.exe"
    if not binary.is_file():
        raise RuntimeError("Build the Windows release executable first")
    sevenzip = os.environ.get("SEVENZIP") or shutil.which("7zz") or shutil.which("7z")
    if not sevenzip:
        raise RuntimeError("Install 7zip or set SEVENZIP to its executable")
    CACHE.mkdir(parents=True, exist_ok=True)
    TOOLS.mkdir(parents=True, exist_ok=True)
    manifest = {}
    for name, (url, digest) in ASSETS.items():
        archive = fetch(url, digest)
        manifest[name] = {"url": url, "sha256": digest}
        if archive.suffix == ".exe":
            shutil.copy2(archive, TOOLS / archive.name)
            continue
        extracted = CACHE / (archive.stem + "-extracted")
        extracted.mkdir(exist_ok=True)
        if archive.suffix == ".zip":
            with zipfile.ZipFile(archive) as source:
                source.extractall(extracted)
        else:
            subprocess.run([sevenzip, "x", "-y", "-bso0", f"-o{extracted}", str(archive)], check=True)
        for path in extracted.rglob("*"):
            if not path.is_file():
                continue
            if path.name.lower() in EXECUTABLES or path.suffix.lower() == ".dll":
                shutil.copy2(path, TOOLS / path.name)
            elif path.suffix.lower() in (".txt", ".md") or "license" in path.name.lower() or "copyright" in path.name.lower():
                dest = DIST / "third-party" / name / path.relative_to(extracted)
                dest.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(path, dest)
    for name in EXECUTABLES:
        if not (TOOLS / name).is_file():
            raise RuntimeError(f"Package is missing tools/{name}")
    (DIST / "runtime-sources.json").write_text(json.dumps(manifest, indent=2) + "\n")
    temporary = DIST / "BitChord.exe.new"
    shutil.copy2(binary, temporary)
    temporary.replace(DIST / "BitChord.exe")
    shutil.copy2(ROOT / "LICENSE", DIST / "LICENSE")
    (DIST / "START-HERE.txt").write_text(
        "Extract the ENTIRE ZIP, then double-click BitChord.exe.\n"
        "Keep the tools folder beside BitChord.exe. No PATH changes or admin installation needed.\n"
        "Audio and download tools are bundled; online playback still requires internet access.\n"
        "Runtime versions/checksums: runtime-sources.json. BitChord source: source/.\n"
        "mpv/FFmpeg build recipes and upstream source: https://github.com/shinchiro/mpv-winbuild-cmake/tree/20261021\n"
        "mpv source: https://github.com/mpv-player/mpv/tree/e471496c56\n"
        "FFmpeg source: https://github.com/FFmpeg/FFmpeg/tree/n9.0.2\n"
        "FFmpeg Windows build: https://github.com/GyanD/codexffmpeg/releases/tag/9.0.2\n"
        "yt-dlp source/license: https://github.com/yt-dlp/yt-dlp/tree/2026.10.26\n"
        "Deno source/license: https://github.com/denoland/deno/tree/v2.9.8\n"
    )
    output = DIST / "BitChord-windows-x86_64.zip"
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=6) as bundle:
        for item in ("BitChord.exe", "LICENSE", "START-HERE.txt", "runtime-sources.json", "tools", "third-party"):
            path = DIST / item
            for file in ([path] if path.is_file() else path.rglob("*")):
                if file.is_file():
                    if item == "tools" and file.suffix.lower() == ".exe" and file.name.lower() not in EXECUTABLES:
                        continue
                    bundle.write(file, Path("BitChord") / file.relative_to(DIST))
        for item in ("Cargo.toml", "Cargo.lock", "README.md", "LICENSE", "src", "assets", "scripts", "packaging"):
            path = ROOT / item
            for file in ([path] if path.is_file() else path.rglob("*")):
                if file.is_file() and "__pycache__" not in file.parts:
                    bundle.write(file, Path("BitChord/source") / file.relative_to(ROOT))
    print("Portable package:", output, flush=True)
    print("Bundled tools:", ", ".join(EXECUTABLES), flush=True)


if __name__ == "__main__":
    main()
