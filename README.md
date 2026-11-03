# YTDugem

A native desktop rewrite of [BitChord](https://github.com/kushagrasinghx/BitChord), based on its `desktop-build-30` desktop target (`7c673dc`). **Windows only for the current development phase**; existing Linux compatibility is retained. Rust application and UI; no Kotlin/JVM, Android, web frontend, or embedded browser. Native rendering with egui, mpv for audio decoding/output, YouTube Music's public Innertube endpoints for discovery.

See [project guidelines](docs/PROJECT_GUIDELINES.md), [AI agent rules](AGENTS.md), and the [desktop parity inventory](docs/DESKTOP_PARITY.md). The inventory distinguishes implemented paths from missing features and unverified behavior.

## Run

Install [Rust](https://rustup.rs), **mpv**, **yt-dlp**, and **FFmpeg** (including ffprobe), then:

```sh
cargo run --release --locked
```

Linux (Debian/Ubuntu): `sudo apt install mpv yt-dlp ffmpeg libxkbcommon0 libgl1`. A graphical X11/Wayland session is required. Keep yt-dlp current when YouTube changes its player.

**Windows portable:** extract **all** of `dist/YTDugem-windows-x86_64.zip`, then double-click `YTDugem/YTDugem.exe`. Keep its `tools` folder beside the executable. mpv, yt-dlp, FFmpeg/ffprobe and Deno are included; no audio-tool installation, administrator access, or system `PATH` changes are needed. **The `.exe` alone is not a portable distribution.** The app prepends its own `tools` folder to the environment inherited by its audio processes.

To build from source on Windows, use Rust with the MSVC build tools and `cargo build --release --locked`. Package the runtime with `python scripts/package-windows.py` (requires Python and 7zip or native Windows `tar` with 7z support; set `SEVENZIP` to override). Downloads are version-pinned and SHA-256 checked. Without the portable package, install mpv, yt-dlp and FFmpeg yourself or use executable overrides below.

**WSL/WSLg:** BitChord automatically uses X11 and Mesa's software renderer as a workaround for EGL/Zink driver initialization failures. Run from your WSL terminal, not a separate Windows shell. Set `BITCHORD_WSL_COMPAT=0` to opt back into the default graphics path if your WSL GPU driver works. To apply the workaround to an older build: `env -u WAYLAND_DISPLAY LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe cargo run --release --locked`. If WSLg still fails to present the window, use the native Windows executable instead; it does not depend on WSLg, Mesa or the WSL display server.

Linux user-local installation: `sh scripts/install-linux.sh`. The resulting executable is `~/.local/bin/ytdugem`; add `~/.local/bin` to `PATH`. It also installs the desktop launcher and icon.

## Implemented

- Desktop Home with real YouTube Music shelves, browsable albums/collections, live search and Explore, artwork, local audio library. Selecting a Home song starts playback.
- Expanded artwork player with blurred background, controls, synchronized lyrics and queue views.
- Google sign-in through a normal isolated Chrome/Edge/Chromium window followed by secure headless capture after that window closes, or masked manual cookie input (Cookie header, Netscape or JSON). Encrypted saved accounts support adding, switching, and removing sessions. Authenticated Home/history/library requests and playback/download cookies are implemented; real-account sign-in and personalized results still require user verification. Brand-channel discovery is not yet ported.
- Real local and online playback through mpv; play/pause, seeking, previous/next, volume, speed (0.5–2×), shuffle and repeat.
- Editable queue with play-next, remove, reorder, clear and play-all.
- Persistent liked songs, recently played tracks, local playlists and settings, with atomic saves and a backup. A corrupt library is preserved and saving is disabled rather than silently overwriting it.
- Recursive local import with metadata, file/folder drag-and-drop; no uploads.
- Audio downloads with embedded metadata through yt-dlp/FFmpeg; downloaded files become available in the local library.
- Fifteen upstream lyrics-provider handlers, ordered enable/disable/reset settings, manual provider selection/retry, source attribution, line/word timing and plain-text fallback. Embedded local lyrics are supported. Availability and catalogue coverage depend on external providers; parser tests are not proof that each live service works.
- Persisted seven-band manual equalizer, upstream presets and stereo balance through mpv/FFmpeg filters; mpv-reported audio-device selection and a Windows exclusive-output preference. This is not upstream's custom DSP, dynamic tone-pad EQ, or a bit-perfect-output guarantee.
- Sleep timer and stop-after-track; keyboard shortcuts.
- Linux MPRIS and Windows system media-control integration, when available in the desktop session.
- Opt-in Discord Rich Presence via the local Windows Discord IPC pipe (no account token): title/artist, artwork, playback countdown, configurable activity name and two button visibility options. Pause/stop/disable clears presence; background IPC has bounded I/O, coalesced updates and reconnects. Real named-pipe fake-server tests pass; real Discord profile rendering still needs [interactive verification](docs/VERIFYING_WINDOWS.md#discord-presence-interactive-verification).

This is a working desktop rewrite, **not yet feature parity with every upstream desktop integration or a pixel-identical UI**. Remote playlist editing/account synchronization, brand-channel discovery, Spotify/music source modules, Last.fm/ListenBrainz, listen-together, canvas video, Automix/crossfade, dynamic equalizer and installer/update automation have not been ported. Android, TV, watch, backend and web targets are deliberately excluded. The [parity inventory](docs/DESKTOP_PARITY.md) records remaining gaps.

### Lyrics provider setup

Settings lists the implemented upstream sources, in priority order: lrc.red, BiniLyrics, BetterLyrics, BetterLyrics Portato, PaxSenix, PaxSenix: Spotify, PaxSenix: Musixmatch, LyricsPlus, Unison, YouTube captions, YouTube Music, KuGou, LRCLIB, Musixmatch, and Genius. Enable/disable individual sources and use the reorder buttons, or reset to defaults. The expanded player's lyrics panel can explicitly request one source and displays the actual returned source. Line/word highlighting and seeking depend on available timing data.

PaxSenix's Spotify/Musixmatch routes require your own PaxSenix API key. Enter it only in the masked Settings field and save; remove it there when no longer needed. Keys are stored separately from the library, encrypted with Windows DPAPI. Standalone Musixmatch negotiates its public client token and does not use that key. Never paste keys or cookies into chat or issue reports. HTTP errors, geoblocks and rate limits can prevent any individual source from working; another provider's fallback result does not verify that source. See [Windows verification](docs/VERIFYING_WINDOWS.md) for manual/live checks.

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
# Isolated import/playback persistence, keyboard navigation, resizing and screenshots (null audio).
powershell -NoProfile -File scripts/windows-ui-smoke.ps1
# Extract the actual ZIP into a fresh folder with spaces and clear all audio-tool PATH entries.
# Default: decoded audio sent to null output, suitable for CI without speakers.
powershell -NoProfile -File scripts/windows-playback-smoke.ps1
# Real Windows sound output and live YouTube playback (plays a short tone/music excerpt):
powershell -NoProfile -File scripts/windows-playback-smoke.ps1 -Online -RealAudio
```

The GUI smoke check creates an isolated library, imports and plays a generated WAV, exercises search, and captures screenshots in `/tmp/opencode/bitchord-smoke`. Set `BITCHORD_TEST_OUTPUT` to change that directory, `BITCHORD_TEST_BINARY` to test the release executable, or `BITCHORD_TEST_DISPLAY=:0` to use an existing test display.

The Windows playback check runs **the packaged YTDugem.exe**, not a substitute player. It verifies decoded samples, advancing playback, pause, seek, observed volume/speed changes, resume and shutdown, writing JSON reports to `dist/playback-test-results`. `-RealAudio` rejects null audio output. CI runs the extracted-package local-audio check on Windows to catch missing-runtime regressions.

Windows playback must be verified against the current packaged build using the commands above. A historical successful run does not prove the current build works; null output does not prove audible playback, and a non-null backend does not prove a listener heard sound.

You can also diagnose any local file or HTTPS stream directly:

```powershell
.\YTDugem.exe --check-playback "C:\Music\track.flac" "C:\Temp\playback-report.json"
```

## Configuration and data

No account credentials or API keys are required for public search. Online playback is subject to YouTube availability, authentication restrictions, network policy, and yt-dlp support; failures are displayed rather than treated as successful playback. Only download media you are authorized to save.

| Variable | Purpose |
|---|---|
| `BITCHORD_MPV` | mpv executable override |
| `BITCHORD_YTDLP` | yt-dlp executable override (also passed to mpv) |
| `BITCHORD_DATA_DIR` | Override the directory containing `library.json` |
| `BITCHORD_AUDIO_OUTPUT=null` | Null audio output for headless tests |

Library and credentials retain the legacy BitChord OS application-data directory (`~/.local/share/bitchord` on Linux; the exact path is shown in Settings), so renaming does not lose existing data. The `BITCHORD_*` environment variables remain compatible. New libraries default to `Music/YTDugem` for downloads; existing libraries retain their configured download directory.

Account credentials are stored separately in `auth/` beside the library, protected with Windows DPAPI or Unix directory/file permissions (0700/0600). Manual cookies grant account access: never share them. Sign-in uses a separate temporary browser profile, not cookies from your normal browser. Chrome, Edge or Chromium must be installed for browser sign-in; Google may restrict automated browser login. Logout removes the saved session; playback already running can retain its private temporary cookie file until the player stops.

## License

GPL-3.0-only. Based on the original BitChord by kushagrasinghx and contributors. See [LICENSE](LICENSE). YTDugem uses a newly generated app icon. Independent third-party client; not affiliated with YouTube or Google.
