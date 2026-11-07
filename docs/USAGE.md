# Using YTDugem

## Install

### Windows portable ZIP

Extract **all** of `YTDugem-windows-x86_64.zip`, then double-click `YTDugem/YTDugem.exe`. Keep its `tools` folder beside the executable. mpv, yt-dlp, FFmpeg/ffprobe and Deno are included. You do not need to install audio tools, use administrator access, or change the system `PATH`. **The `.exe` alone is not a portable distribution.** The app prepends its own `tools` folder to the environment inherited by its audio processes.

### From source

Install [Rust](https://rustup.rs), **mpv**, **yt-dlp**, and **FFmpeg** (including ffprobe), then:

```sh
cargo run --release --locked
```

On Windows, use Rust with the MSVC build tools. Build the portable ZIP with `cargo xtask package`. It needs Python, plus 7-Zip or native Windows `tar` with 7z support; set `SEVENZIP` to override the 7-Zip path. Runtime downloads are version-pinned and SHA-256 checked. Without the portable package, install mpv, yt-dlp and FFmpeg yourself or use the executable overrides below.

### Linux

On Debian/Ubuntu: `sudo apt install mpv yt-dlp ffmpeg libxkbcommon0 libgl1`. A graphical X11/Wayland session is required. Keep yt-dlp current when YouTube changes its player.

For a user-local installation, run `sh scripts/install-linux.sh`. The resulting executable is `~/.local/bin/ytdugem`, so add `~/.local/bin` to `PATH`. The script also installs the desktop launcher and icon.

### WSL/WSLg

YTDugem automatically uses X11 and Mesa's software renderer to work around EGL/Zink driver initialization failures. Run it from your WSL terminal, not a separate Windows shell.

- Set `BITCHORD_WSL_COMPAT=0` to opt back into the default graphics path if your WSL GPU driver works.
- To apply the workaround to an older build, run `env -u WAYLAND_DISPLAY LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe cargo run --release --locked`.
- If WSLg still fails to present the window, use the native Windows executable instead. It does not depend on WSLg, Mesa or the WSL display server.

## Features

- **Home and search:** Desktop Home with real YouTube Music shelves, browsable albums and collections, live search and Explore, artwork, and a local audio library. Selecting a Home song starts playback.
- **Explore:** New releases, Charts and Moods & genres buttons, YouTube Music's Explore shelves, and mood and genre tiles that open their playlists. Each sub-page has Back to Explore; the refresh button reloads everything.
- **Expanded player:** artwork view with a blurred background, controls, synchronized lyrics and queue views.
- **Google sign-in:** sign-in runs in a normal isolated Chrome/Edge/Chromium window. After that window closes, the session is captured securely in a headless browser. You can also enter cookies manually in a masked field (Cookie header, Netscape or JSON).
  - Encrypted saved accounts support adding, switching and removing sessions.
  - Authenticated Home, history and library requests are implemented, and so are playback and download cookies.
  - Real-account sign-in and personalized results still require user verification. Brand-channel discovery is not yet ported.
- **Playback:** real local and online playback through mpv, with play/pause, seeking, previous/next, volume, speed (0.5–2×), shuffle and repeat.
- **Queue:** an editable queue with play-next, remove, reorder, clear and play-all.
- **Library:** persistent liked songs, recently played tracks, local playlists and settings, with atomic saves and a backup. A corrupt library is preserved and saving is disabled rather than silently overwriting it.
- **Local import:** recursive import with metadata, plus file and folder drag-and-drop. Nothing is uploaded.
- **Downloads:** audio downloads with embedded metadata through yt-dlp/FFmpeg. Downloaded files become available in the local library.
- **Lyrics:** fifteen upstream lyrics-provider handlers, with ordered enable/disable/reset settings, manual provider selection and retry, source attribution, line and word timing, and a plain-text fallback. Embedded local lyrics are supported. Availability and catalogue coverage depend on external providers; parser tests are not proof that each live service works.
- **Audio:** a persisted seven-band manual equalizer, upstream presets and stereo balance through mpv/FFmpeg filters. You can select an audio device reported by mpv and set a Windows exclusive-output preference. This is not upstream's custom DSP or dynamic tone-pad EQ, and it does not guarantee bit-perfect output.
- **Timers:** sleep timer, stop-after-track, and keyboard shortcuts.
- **System media controls:** Linux MPRIS and Windows system media-control integration, when available in the desktop session.
- **Discord Rich Presence:** coming soon. Settings shows it as "Coming soon" and nothing is sent to Discord until YTDugem has its own Discord application.

This is a working desktop rewrite, **not yet feature parity with every upstream desktop integration or a pixel-identical UI**. The following have not been ported:

- remote playlist editing and account synchronization
- brand-channel discovery
- Spotify and other music source modules
- Last.fm and ListenBrainz
- listen-together
- canvas video
- Automix and crossfade
- dynamic equalizer
- installer and update automation

Android, TV, watch, backend and web targets are deliberately excluded.

## Lyrics providers

Settings lists the implemented upstream sources in priority order: lrc.red, BiniLyrics, BetterLyrics, BetterLyrics Portato, PaxSenix, PaxSenix: Spotify, PaxSenix: Musixmatch, LyricsPlus, Unison, YouTube captions, YouTube Music, KuGou, LRCLIB, Musixmatch, and Genius. You can enable or disable individual sources, reorder them with the buttons, or reset to defaults. The expanded player's lyrics panel can request one source explicitly and shows the source that actually answered. Line and word highlighting and seeking depend on the timing data available.

PaxSenix's Spotify and Musixmatch routes require your own PaxSenix API key. Enter it only in the masked Settings field and save; remove it there when you no longer need it. Keys are stored separately from the library and encrypted with Windows DPAPI. Standalone Musixmatch negotiates its public client token and does not use that key. Never paste keys or cookies into chat or issue reports.

HTTP errors, geoblocks and rate limits can stop any individual source from working, and a fallback result from another provider does not verify that source. See [Windows verification](VERIFYING_WINDOWS.md) for manual and live checks.

## Keyboard shortcuts

| Shortcut | Action |
|---|---|
| Ctrl+F | Search |
| Ctrl+L | Local import |
| Enter | Submit |
| Ctrl+Enter | Play the first local, search or collection result |
| Space | Play/pause (outside text fields) |
| Alt+Left / Alt+Right | Previous / next |
| F11 | Expanded player |
| L / Q | Lyrics / queue in expanded mode |
| Esc | Close |
| Ctrl+Q | Quit |

## Configuration and data

Public search needs no account credentials or API keys. Online playback depends on YouTube availability, authentication restrictions, network policy, and yt-dlp support. Failures are displayed rather than treated as successful playback. Only download media you are authorized to save.

| Variable | Purpose |
|---|---|
| `BITCHORD_MPV` | mpv executable override |
| `BITCHORD_YTDLP` | yt-dlp executable override (also passed to mpv) |
| `BITCHORD_DATA_DIR` | Override the directory containing `library.json` |
| `BITCHORD_AUDIO_OUTPUT=null` | Null audio output for headless tests |
| `BITCHORD_WSL_COMPAT=0` | Disable the WSL graphics workaround |

The library and credentials keep the legacy BitChord application-data directory (`~/.local/share/bitchord` on Linux; Settings shows the exact path), so existing data survived the rename. The `BITCHORD_*` environment variables also remain compatible. New libraries download to `Music/YTDugem` by default, and existing libraries keep their configured download directory.

Account credentials are stored separately in `auth/` beside the library. They are protected with Windows DPAPI on Windows and with 0700/0600 directory and file permissions on Unix. Manual cookies grant account access, so never share them.

Sign-in uses a separate temporary browser profile, not the cookies from your normal browser. Chrome, Edge or Chromium must be installed for browser sign-in, and Google may restrict automated browser login. Logging out removes the saved session. Playback that is already running may keep its private temporary cookie file until the player stops.

A playback diagnostic for a local file or HTTPS stream is built in:

```powershell
.\YTDugem.exe --check-playback "C:\Music\track.flac" "C:\Temp\playback-report.json"
```
