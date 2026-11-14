# Using YTDugem

## Install

### Windows installer

Run `YTDugem-windows-x86_64-setup.exe` from the [latest release](https://github.com/TazakiN/YTDugem/releases/latest). It installs for the current user into `%LOCALAPPDATA%\Programs\YTDugem` without administrator access, and adds a Start menu entry (and, optionally, a desktop shortcut). It contains the same files as the portable ZIP. Uninstall it from **Settings → Apps**. Your library and sign-in are kept.

### Windows portable ZIP

Extract **all** of `YTDugem-windows-x86_64.zip`, then double-click `YTDugem/YTDugem.exe`. Keep its `tools` folder beside the executable. mpv, yt-dlp, FFmpeg/ffprobe and Deno are included. You do not need to install audio tools, use administrator access, or change the system `PATH`. **The `.exe` alone is not a portable distribution.** The app prepends its own `tools` folder to the environment inherited by its audio processes.

#### Updates

At startup, YTDugem checks the [latest release](https://github.com/TazakiN/YTDugem/releases/latest) on GitHub. **Settings → About → Check for updates** runs the same check. When a newer stable release exists, a dialog offers **Update now**. The app downloads the release ZIP into a `.update` folder beside `YTDugem.exe` and checks its SHA-256 against the release's `.sha256` file. When you choose **Restart now**, it stops playback, swaps in the new files and restarts. Your library and sign-in live in the application-data directory and are not touched. If any file cannot be replaced, the folder is restored and an error is shown. Pre-releases are never offered.

Installed and portable copies update the same way. In-place updates need a folder you can write to, with `YTDugem.exe` and `tools/mpv.exe` beside it. The installer's folder qualifies, and for an installed copy the update also changes the version shown in Apps & features. Elsewhere (a build from `target/`, a renamed exe, Linux), the dialog opens the release page instead. Set `YTDUGEM_NO_UPDATE_CHECK=1` to skip the startup check. This differs from upstream, whose dialog only opens the installer in the browser.

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

- **Home and search:** Desktop Home with real YouTube Music shelves, browsable albums and collections, live search (Artists and Playlists rows above the matching songs; a playlist card opens the playlist) and Explore, artwork, and a local audio library. Selecting a Home song starts playback.
- **Explore:** New releases, Charts and Moods & genres buttons, YouTube Music's Explore shelves, and mood and genre tiles that open their playlists. Each sub-page has Back to Explore; the refresh button reloads everything.
- **Artist pages:** artist banner, top songs, releases and related artists, opened from a song's artist name, an artist card or search results. See [Artist pages](#artist-pages).
- **Expanded player:** artwork view with a blurred background, controls, synchronized lyrics and queue views.
- **Google sign-in:** sign-in runs in a normal isolated Chrome/Edge/Chromium window. After that window closes, the session is captured securely in a headless browser. You can also enter cookies manually in a masked field (Cookie header, Netscape or JSON).
  - Encrypted saved accounts support adding, switching and removing sessions.
  - Authenticated Home, history and library requests are implemented, and so are playback and download cookies.
  - Real-account sign-in and personalized results still require user verification. Brand-channel discovery is not yet ported.
- **Playback:** real local and online playback through mpv, with play/pause, seeking, previous/next, volume, speed (0.5–2×), shuffle and repeat.
- **Playback controls:** **Settings → Appearance → Playback controls** chooses where the controls live: the **Top bar** (default) or a **Floating bar**, a pill over the bottom of the page with the transport, what is playing with a seekable progress line, the track menu, lyrics, queue and a volume popup. The floating bar is a YTDugem addition, not an upstream BitChord layout.
- **Skip non-music sections:** **Settings → Playback → Skip non-music sections (SponsorBlock)** (off by default) skips the intros, outros and talking parts of music videos that SponsorBlock users marked as `music_offtopic`. A section is skipped once, when playback reaches it. Seeking back into it plays it, and a section that runs to the end moves on to the next track. A failed or slow lookup just means nothing is skipped. See [Configuration and data](#configuration-and-data) for what is sent. This is a YTDugem addition, not an upstream BitChord feature.
- **Queue:** an editable queue with play-next, remove, reorder, clear and play-all.
- **Library:** persistent liked songs, recently played tracks, local playlists and settings, with atomic saves and a backup. A corrupt library is preserved and saving is disabled rather than silently overwriting it.
- **YouTube Music account playlists and ratings** (signed in only): the Library lists your account playlists next to your local ones. "Add to playlist…" offers both, and it can create a new account playlist seeded with the song. Owned account playlists support removing and reordering songs, renaming, editing the description and privacy, and deleting, and you can save or remove other playlists in your library. Like and Dislike are in the player menu, the full player's heart and every YouTube song row's menu. Row menus read the rating from your Liked Music playlist (up to its first 10 pages) and the playing song's rating from YouTube. Signed out, or for local files, only the local playlists are used, and local playlists are never changed by account edits. The request building and response parsing are unit tested against made-up fixtures; **real Google-account verification of these edits is still pending.**
- **Local import:** recursive import with metadata, plus file and folder drag-and-drop. Nothing is uploaded.
- **Recent searches and suggestions:** clicking the sidebar search box (or Ctrl+F) while it is empty lists your last 20 searches, most recent first; click one to run it again, its **×** to remove it, or **Clear recent searches** (also in **Settings → Privacy → Recent searches**). While you type, matching recent searches come first, followed by YouTube Music's search suggestions, fetched about 250 ms after you stop typing without your sign-in, so typing does not enter your account's search history. If suggestions cannot be fetched, only your recent searches are listed. Up/Down choose an entry, Enter searches for it (or for what you typed), and Esc closes the list, then leaves the box. Searches are remembered when submitted, ignoring case and surrounding spaces, and are stored in `library.json`. A YTDugem addition.
- **Listening stats:** **Library → Listening stats** (or **Settings → Privacy → Listening stats → Open**) shows your listening for the last 7 days, the last 30 days, this year or all time: total listening time, plays, distinct songs and artists, top songs, artists and albums with play counts and artwork, and listening by hour of day and by day of week, plus a **Wrapped** card for the current year. Clicking a top song plays it; clicking an artist opens their page (or looks the name up). A play counts once you have heard 30 seconds of a song, or half of a song shorter than a minute. Listening time follows the playback position, so pauses and seeks do not add to it. Hours and days use the computer's current time zone (on Linux, UTC). **Settings → Privacy → Record listening stats** (on by default) pauses recording, and **Clear listening stats** deletes every recorded play after a confirmation. Stats stay on this computer: see [Configuration and data](#configuration-and-data). A YTDugem addition.
- **Filter and sort:** Local music, History and local playlists have a filter box (case-insensitive over title, artist and album) and a sort menu: the list's own order, recently added (Local music and playlists), title, artist or duration (unknown durations last). Play all and the queue use the filtered, sorted list. A YTDugem addition.
- **Playlist files:** a local playlist page has **Export playlist** and Local music has **Export library**: type the path of a new file and choose **M3U8** (extended M3U with `#EXTINF`; local songs as file paths, online songs as `https://music.youtube.com/watch?v=` links) or **CSV** (`title,artist,album,duration,url`). An existing file is never replaced. **Library → On this computer → Import playlist file** reads an `.m3u` or `.m3u8` file into a new local playlist named after the file: paths (relative ones from the playlist's folder) become local tracks when a supported audio file exists there, YouTube and YouTube Music video links become online tracks, and missing files, other links and duplicates are skipped and counted. There is no file picker: paste a path (Explorer's "Copy as path" quotes are accepted). A YTDugem addition.
- **Downloads:** audio downloads with embedded metadata through yt-dlp/FFmpeg. Downloaded files become available in the local library.
- **Lyrics:** fifteen upstream lyrics-provider handlers, with ordered enable/disable/reset settings, manual provider selection and retry, source attribution, line and word timing, and a plain-text fallback. Embedded local lyrics are supported. Availability and catalogue coverage depend on external providers; parser tests are not proof that each live service works.
- **Audio:** a persisted seven-band manual equalizer, upstream presets and stereo balance through mpv/FFmpeg filters. Two optional toggles, both off by default, use the same filter chain and apply to the song that is playing: **Loudness normalization** (FFmpeg `dynaudnorm`, a single-pass gain rider over a ~15 s window, peak target 0.9, at most 10x gain; it evens out volume between songs but is not ReplayGain or EBU R128 two-pass normalization) and **Skip silence** (FFmpeg `silenceremove`: leading silence below -60 dB is dropped, and any silent stretch below -60 dB longer than 2 s, including trailing silence, is shortened to about 2.5 s; shorter or louder musical pauses are kept). With Skip silence on, the elapsed time runs behind the track length by the amount trimmed, so a song ends that much before its displayed duration; a crossfade is shortened by the same amount, or replaced by a normal track change when more than the crossfade length was trimmed. You can select an audio device reported by mpv and set a Windows exclusive-output preference. This is not upstream's custom DSP or dynamic tone-pad EQ, and it does not guarantee bit-perfect output.
- **Timers:** sleep timer, stop-after-track, and keyboard shortcuts.
- **System media controls:** Linux MPRIS and Windows system media-control integration, when available in the desktop session.
- **Discord Rich Presence:** coming soon. Settings shows it as "Coming soon" and nothing is sent to Discord until YTDugem has its own Discord application.

This is a working desktop rewrite, **not yet feature parity with every upstream desktop integration or a pixel-identical UI**. The following have not been ported:

- syncing local playlists or local liked songs to the account (account and local playlists are kept separate)
- brand-channel discovery
- Spotify and other music source modules
- Last.fm and ListenBrainz
- listen-together
- canvas video
- Automix
- dynamic equalizer
- MSI package (the release has an Inno Setup `setup.exe` instead)

Android, TV, watch, backend and web targets are deliberately excluded.

## Artist pages

To open an artist, click the artist name wherever it is shown: song rows, the player bar, the full player, the queue, an album page header, or the subtitle of a card. You can also choose "Open artist" in the player menu, click an artist card (for example in "Fans might also like"), or use the Artists row at the top of search results.

An artist page shows the banner image and name, the subscriber count and monthly audience, and Play, Shuffle, Mix (artist radio) and, when you are signed in, Subscribe. Below these are a Latest release card, Top songs, and the shelves YouTube Music provides (Albums, Singles & EPs, Videos, Live performances, Featured on, Playlists, Fans might also like), with "See all" where YouTube Music offers one. About shows the artist's biography.

When a song credits several artists, each linked name opens its own page. Tracks saved before artist pages were added, and downloaded or imported files, carry no artist links. Clicking their artist name searches for an artist with exactly that name and opens the page only on an exact match; otherwise a message is shown. Local audio files without a YouTube match have no artist page.

Known gaps compared with the Apple Music-style design:

- Latest release is chosen by year only, because YouTube gives no release date. Among releases from the same year, the first one listed wins.
- YouTube Music does not provide upcoming concerts, editorial "Essential albums" quotes, radio shows and interviews ("More to hear", "More to see"), or structured artist facts such as hometown, birth date and genre, so these are not shown. About shows only the biography text.
- "Compilations" is approximated by YouTube's "Featured on" shelf.
- Artist names in the tray tooltip, Discord status and Windows media controls are not clickable.

## Lyrics providers

Settings lists the implemented upstream sources in priority order: lrc.red, BiniLyrics, BetterLyrics, BetterLyrics Portato, PaxSenix, PaxSenix: Spotify, PaxSenix: Musixmatch, LyricsPlus, Unison, YouTube captions, YouTube Music, KuGou, LRCLIB, Musixmatch, and Genius. You can enable or disable individual sources, reorder them with the buttons, or reset to defaults. The expanded player's lyrics panel can request one source explicitly and shows the source that actually answered. Line and word highlighting and seeking depend on the timing data available. Background (answering) vocals get their own smaller line under the lyric they answer, shown only while they are sung: read from TTML's `ttm:role="x-bg"` spans, or split off a trailing bracket for other sources, as upstream. Not yet ported: the backing line is not translated or romanized, and it does not scale in as upstream does.

PaxSenix's Spotify and Musixmatch routes require your own PaxSenix API key. Enter it only in the masked Settings field and save; remove it there when you no longer need it. Keys are stored separately from the library and encrypted with Windows DPAPI. Standalone Musixmatch negotiates its public client token and does not use that key. Never paste keys or cookies into chat or issue reports.

HTTP errors, geoblocks and rate limits can stop any individual source from working, and a fallback result from another provider does not verify that source. See [Windows verification](VERIFYING_WINDOWS.md) for manual and live checks.

## Keyboard shortcuts

| Shortcut | Action |
|---|---|
| Ctrl+F | Search |
| Ctrl+L | Local import |
| Enter | Submit |
| Up / Down, Enter, Esc | In the search box: choose a recent search or suggestion, search for it, close the list |
| Ctrl+Enter | Play the first local, search or collection result |
| Space | Play/pause (outside text fields) |
| Alt+Left / Alt+Right | Previous / next |
| F11 | Expanded player |
| L / Q | Lyrics / queue in expanded mode |
| Esc | Close |
| Ctrl+Q | Quit |

## Configuration and data

Public search needs no account credentials or API keys. Online playback depends on YouTube availability, authentication restrictions, network policy, and yt-dlp support. Failures are displayed rather than treated as successful playback. Only download media you are authorized to save.

**Skip non-music sections** is off by default because it contacts a third-party service, [SponsorBlock](https://sponsor.ajay.app). When it is on, YTDugem sends only the first four hex digits of the SHA-256 hash of each YouTube video ID it plays. The response covers many videos, and YTDugem picks out the one it is playing locally. No account or other data is sent, and local files are never looked up.

| Variable | Purpose |
|---|---|
| `BITCHORD_MPV` | mpv executable override |
| `BITCHORD_YTDLP` | yt-dlp executable override (also passed to mpv) |
| `BITCHORD_DATA_DIR` | Override the directory containing `library.json` |
| `BITCHORD_AUDIO_OUTPUT=null` | Null audio output for headless tests |
| `BITCHORD_WSL_COMPAT=0` | Disable the WSL graphics workaround |
| `YTDUGEM_NO_UPDATE_CHECK=1` | Skip the update check at startup |

The library and credentials keep the legacy BitChord application-data directory (`~/.local/share/bitchord` on Linux; Settings shows the exact path), so existing data survived the rename. The `BITCHORD_*` environment variables also remain compatible. New libraries download to `Music/YTDugem` by default, and existing libraries keep their configured download directory.

Listening stats are kept in `stats.json` beside `library.json`, never in the library file, and are never sent anywhere. Each play stores the song (id, title, artist, album, artwork link), when it started (UTC) and how many seconds were heard. Plays older than two years, and the oldest beyond 40,000 plays, are dropped. New plays are saved in the background about 30 seconds after they are recorded and when YTDugem exits. The save is atomic with a `stats.json.bak` copy, and a `stats.json` that cannot be read is left untouched: recording pauses and an error is shown instead of overwriting it.

Account credentials are stored separately in `auth/` beside the library. They are protected with Windows DPAPI on Windows and with 0700/0600 directory and file permissions on Unix. Manual cookies grant account access, so never share them.

Sign-in uses a separate temporary browser profile, not the cookies from your normal browser. Chrome, Edge or Chromium must be installed for browser sign-in, and Google may restrict automated browser login. Logging out removes the saved session. Playback that is already running may keep its private temporary cookie file until the player stops.

A playback diagnostic for a local file or HTTPS stream is built in:

```powershell
.\YTDugem.exe --check-playback "C:\Music\track.flac" "C:\Temp\playback-report.json"
```
