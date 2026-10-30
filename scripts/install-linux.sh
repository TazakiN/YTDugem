#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
for tool in cargo mpv yt-dlp ffmpeg ffprobe; do
    command -v "$tool" >/dev/null || { echo "Missing $tool. Install Rust, mpv, yt-dlp and FFmpeg first." >&2; exit 1; }
done
cargo build --release --locked
prefix="${PREFIX:-$HOME/.local}"
install -Dm755 target/release/ytdugem "$prefix/bin/ytdugem"
install -Dm644 assets/icon.png "$prefix/share/icons/hicolor/512x512/apps/in.ytdugem.desktop.png"
install -Dm644 packaging/in.ytdugem.desktop.desktop "$prefix/share/applications/in.ytdugem.desktop.desktop"
echo "Installed $prefix/bin/ytdugem"
