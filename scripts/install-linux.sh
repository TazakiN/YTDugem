#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
for tool in cargo mpv yt-dlp ffmpeg ffprobe; do
    command -v "$tool" >/dev/null || { echo "Missing $tool. Install Rust, mpv, yt-dlp and FFmpeg first." >&2; exit 1; }
done
cargo build --release --locked
prefix="${PREFIX:-$HOME/.local}"
install -Dm755 target/release/bitchord-desktop "$prefix/bin/bitchord-desktop"
install -Dm644 assets/icon.png "$prefix/share/icons/hicolor/512x512/apps/in.bitchord.desktop.png"
install -Dm644 packaging/in.bitchord.desktop.desktop "$prefix/share/applications/in.bitchord.desktop.desktop"
echo "Installed $prefix/bin/bitchord-desktop"
