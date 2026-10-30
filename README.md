# BitChord Desktop — Rust

A native desktop rewrite of [BitChord](https://github.com/kushagrasinghx/BitChord), based on its `desktop-build-30` desktop target (`7c673dc`). Rust application and UI; no Kotlin/JVM, Android, web frontend, or embedded browser. Native rendering with egui, mpv for audio decoding/output, YouTube Music's public Innertube endpoints for discovery.

## Run

Install [Rust](https://rustup.rs), **mpv**, **yt-dlp**, and **FFmpeg** (including ffprobe), then:

```sh
cargo run --release --locked
```

Linux (Debian/Ubuntu): `sudo apt install mpv yt-dlp ffmpeg libxkbcommon0 libgl1`. A graphical X11/Wayland session is required. Keep yt-dlp current when YouTube changes its player.

**Windows portable:** extract **all** of `dist/BitChord-windows-x86_64.zip`, then double-click `BitChord/BitChord.exe`. Keep its `tools` folder beside the executable. mpv, yt-dlp, FFmpeg/ffprobe and Deno are included; no audio-tool installation, administrator access, or system `PATH` changes are needed. **The `.exe` alone is not a portable distribution.** The app prepends its own `tools` folder to the environment inherited by its audio processes.

To build from source on Windows, use Rust with the MSVC build tools and `cargo build --release --locked`. Package the runtime with `python scripts/package-windows.py` (requires Python and 7zip; set `SEVENZIP` if needed). Downloads are version-pinned and SHA-256 checked. Without the portable package, install mpv, yt-dlp and FFmpeg yourself or use executable overrides below.

**WSL/WSLg:** BitChord automatically uses X11 and Mesa's software renderer as a workaround for EGL/Zink driver initialization failures. Run from your WSL terminal, not a separate Windows shell. Set `BITCHORD_WSL_COMPAT=0` to opt back into the default graphics path if your WSL GPU driver works. To apply the workaround to an older build: `env -u WAYLAND_DISPLAY LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe cargo run --release --locked`. If WSLg still fails to present the window, use the native Windows executable instead; it does not depend on WSLg, Mesa or the WSL display server.

Linux user-local installation: `sh scripts/install-linux.sh`. The resulting executable is `~/.local/bin/bitchord-desktop`; add `~/.local/bin` to `PATH`. It also installs the desktop launcher and icon.

## Implemented

- Desktop Home with real YouTube Music shelves, browsable albums/collections, live search and Explore, artwork, local audio library. Selecting a Home song starts playback.
- Expanded artwork player with blurred background, controls, synchronized lyrics and queue views.
- Google sign-in through an isolated Chrome/Edge/Chromium window, or masked manual cookie input (Cookie header, Netscape or JSON). Authenticated Home/history/library requests and playback/download cookies are implemented; real-account sign-in and personalized results still require user verification.
- Real local and online playback through mpv; play/pause, seeking, previous/next, volume, speed (0.5–2×), shuffle and repeat.
- Editable queue with play-next, remove, reorder, clear and play-all.
- Persistent liked songs, recently played tracks, local playlists and settings, with atomic saves and a backup. A corrupt library is preserved and saving is disabled rather than silently overwriting it.
- Recursive local import with metadata, file/folder drag-and-drop; no uploads.
- Audio downloads with embedded metadata through yt-dlp/FFmpeg; downloaded files become available in the local library.
- Synced LRCLIB lyrics with active-line highlighting, click-to-seek and plain-text fallback.
- Sleep timer and stop-after-track; keyboard shortcuts.
- Linux MPRIS and Windows system media-control integration, when available in the desktop session.

This is a working desktop rewrite, **not yet feature parity with every upstream desktop integration or a pixel-identical UI**. Remote playlist editing/account synchronization, Spotify, source modules, multi-provider word-level lyrics, Discord presence, Last.fm/ListenBrainz, listen-together, canvas video, Automix/crossfade, equalizer and installer/update automation have not been ported. Android, TV, watch, backend and web targets are deliberately excluded.

Shortcuts: Ctrl+F search, Ctrl+L local import, Enter submits, Ctrl+Enter plays the first local/search/collection result, Space play/pause outside text fields, Alt+Left/Right previous/next, F11 expanded player, L/Q lyrics/queue in expanded mode, Esc closes, Ctrl+Q quits.

## Checks

```sh
cargo test --locked
BITCHORD_AUDIO_OUTPUT=null cargo test plays_local_audio -- --ignored --nocapture
cargo test live_music_search_and_browse -- --ignored --nocapture
cargo test live_lyrics -- --ignored --nocapture
cargo test plays_youtube_audio -- --ignored --nocapture
dbus-run-session -- cargo test live_mpris -- --ignored --nocapture
# Real window smoke test: requires Xvfb, xauth, xdotool and ffmpeg.
cargo build --locked
python3 scripts/gui-smoke.py
# Windows: verify the native executable with an isolated library.
powershell -NoProfile -File scripts/windows-smoke.ps1
# Extract the actual ZIP into a fresh folder with spaces and clear all audio-tool PATH entries.
# Default: decoded audio sent to null output, suitable for CI without speakers.
powershell -NoProfile -File scripts/windows-playback-smoke.ps1
# Real Windows sound output and live YouTube playback (plays a short tone/music excerpt):
powershell -NoProfile -File scripts/windows-playback-smoke.ps1 -Online -RealAudio
```

The GUI smoke check creates an isolated library, imports and plays a generated WAV, exercises search, and captures screenshots in `/tmp/opencode/bitchord-smoke`. Set `BITCHORD_TEST_OUTPUT` to change that directory, `BITCHORD_TEST_BINARY` to test the release executable, or `BITCHORD_TEST_DISPLAY=:0` to use an existing test display.

The Windows playback check runs **the packaged BitChord.exe**, not a substitute player. It verifies decoded samples, advancing playback, pause, seek, observed volume/speed changes, resume and shutdown, writing JSON reports to `dist/playback-test-results`. `-RealAudio` rejects null audio output. CI runs the extracted-package local-audio check on Windows to catch missing-runtime regressions.

Verified on Windows with no installed audio tools in `PATH`: a local WAV and a YouTube music stream both opened **WASAPI audio output at 44.1 kHz** and passed every playback/control check.

You can also diagnose any local file or HTTPS stream directly:

```powershell
.\BitChord.exe --check-playback "C:\Music\track.flac" "C:\Temp\playback-report.json"
```

## Configuration and data

No account credentials or API keys are required for public search. Online playback is subject to YouTube availability, authentication restrictions, network policy, and yt-dlp support; failures are displayed rather than treated as successful playback. Only download media you are authorized to save.

| Variable | Purpose |
|---|---|
| `BITCHORD_MPV` | mpv executable override |
| `BITCHORD_YTDLP` | yt-dlp executable override (also passed to mpv) |
| `BITCHORD_DATA_DIR` | Override the directory containing `library.json` |
| `BITCHORD_AUDIO_OUTPUT=null` | Null audio output for headless tests |

By default, library data lives in the OS application-data directory (`~/.local/share/bitchord` on Linux; the exact path is shown in Settings). Downloads default to `Music/BitChord` and can be changed in Settings. No upstream user data is modified or migrated automatically.

Account credentials are stored separately in `auth/` beside the library, protected with Windows DPAPI or Unix directory/file permissions (0700/0600). Manual cookies grant account access: never share them. Sign-in uses a separate temporary browser profile, not cookies from your normal browser. Chrome, Edge or Chromium must be installed for browser sign-in; Google may restrict automated browser login. Logout removes the saved session; playback already running can retain its private temporary cookie file until the player stops.

## License

GPL-3.0-only. Original BitChord and its icon: kushagrasinghx and contributors. See [LICENSE](LICENSE). Independent third-party client; not affiliated with YouTube or Google.
