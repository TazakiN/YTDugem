# BitChord Desktop parity inventory

## Baseline and evidence

Reference: upstream `desktop-build-30` at **`7c673dcdbb5a6f8cc36538eb8813c559e37c26b2`**, not the Android app on `main`. Paths below are relative to `desktopApp/src/main/kotlin/com/music/bitchord/desktop/` unless stated otherwise. Inspect with:

```powershell
git -C references/BitChord show origin/desktop-build-30:desktopApp/src/main/kotlin/com/music/bitchord/desktop/DesktopApp.kt
```

This is an initial source-based inventory, not proof of complete parity. **Partial** means a reachable implementation exists but does not cover all upstream behavior. **Missing** means no corresponding implementation was found during this audit. Worker changes require review and tests before updating status.

## Feature mapping

| Desktop feature | Upstream reference | Rust mapping / current gap |
|---|---|---|
| Home, Explore, shelves and continuation | `DesktopApp.kt`, `DesktopSharedPages.kt`, `DesktopSharedCalls.kt` | Partial: `services.rs`, `ui.rs`; guest/authenticated feeds exist. Live account verification outstanding. |
| Search, collections, albums, artist browsing | `DesktopSearchClient.kt`, `DesktopSharedPages.kt` | Partial: search/collection playback exists; full typed filters and artist interactions need comparison. |
| Library, sorting, grids, remote synchronization | `DesktopSharedPages.kt`, `DesktopApp.kt` | Partial: local liked/library views and authenticated fetching; no complete remote edit/sort/grid behavior. |
| Playlist create/rename/delete/edit and ownership | `DesktopPlaylistUi.kt`, `DesktopSongMenuUi.kt`, `DesktopApp.kt` | Partial: account playlists on the Library page; create (optionally seeded with a song), rename, delete, add and remove (by `playlistSetVideoId`) through upstream's `playlist/create`, `playlist/delete` and `browse/edit_playlist`, with refusals (`status`) treated as errors and ownership from the editable header. Offline fixtures only; not exercised against a real account. Reordering (`ACTION_MOVE_VIDEO_BEFORE`) is missing. |
| History and listening statistics/replay | `DesktopListeningStats.kt`, `DesktopReplayUi.kt`, `DesktopPlaybackTracker.kt` | Partial: local recent tracks; statistical replay and account history parity missing. |
| Queue, play-next, shuffle/repeat, autoplay | `DesktopQueue.kt`, `DesktopAutoplay.kt`, `DesktopApp.kt` | Partial: controls, queue edits, radio/autoplay exist; upstream queue-section semantics/persistence need comparison. |
| Local file import, metadata and watching | `DesktopLocalMusic.kt`, `DesktopLocalMusicWatcher.kt` | Partial: recursive import and drag/drop; folder watching missing. |
| Stream and local playback, speed, seek, volume | `DesktopPlayback.kt`, `DesktopAudioEngine.kt`, `DesktopAudioSpeed.kt` | Partial: mpv and IPC in `player.rs`; portable Windows runtime and real output require current checks. |
| Audio quality/source selection and matching | `DesktopSourceConfig.kt`, `DesktopSourceRegistry.kt`, `DesktopTrackMatcher.kt` | Missing source registry/quality ladder. Current online engine uses YouTube/yt-dlp. |
| JioSaavn, addons and custom source modules | `DesktopJioSaavn.kt`, `DesktopAddon.kt`, `DesktopModules.kt` | Missing; do not show configured-source success without a real request. |
| Lossless/hi-res, WASAPI device/exclusive output | `DesktopAudioDevices.kt`, `DesktopAudioOutputUi.kt`, `DesktopWindowsAudio.kt` | Partial: enumerated mpv devices, saved selection and Windows exclusive preference are exposed. Bundled mpv device enumeration and normal WASAPI playback verified; exclusive-device/hardware fidelity and bit-perfect output are not established. |
| Audio pipeline information, normalization and analysis | `DesktopAudioPipelineUi.kt`, `DesktopAnalysisRuntime.kt`, `DesktopTrackAnalyzer.kt`, `DesktopAudioProcessors.kt` | Missing desktop diagnostic/settings parity. Decoder status alone is not equivalent. |
| Equalizer, tone controls, presets, stereo balance | `DesktopEqualizer.kt`, `DesktopEqualizerSettings.kt`, `DesktopEqualizerUi.kt` | Partial: seven-band manual EQ, upstream presets and balance through mpv/FFmpeg filters; actual bundled-mpv apply/reset/rejection tests pass with null audio. Dynamic tone pad/custom upstream DSP remain missing. |
| Automix, crossfade and transitions | `DesktopDecodeAhead.kt`, `DesktopTransitionEcho.kt`, `DesktopTransitionRide.kt`, `DesktopApp.kt` | Partial: Crossfade 0–12 s (upstream `crossfade_seconds`, default off) blends two mpv processes equal-power; the track Next would play (upstream `prepareNext`, nothing under repeat-one, shuffle picked early) has its stream resolved in the background, so it is audible ~0.3 s after starting instead of ~3.5 s. Automix (analysis-timed transitions) is missing. |
| Downloads, quality, progress, retry/cancel queue | `DesktopDownloadManager.kt`, `DesktopDownloadManagerUi.kt`, `DesktopDownloadQueue.kt` | Partial: queue with 4 workers (upstream `WORKERS`), Queued/Downloading N%/Failed rows, cancel, Cancel all, Clear and Retry; Original (no re-encode), MP3 or M4A format; cookie-less fast path with signed-in fallback. Not persisted across restarts (as upstream). Lossless sources do not exist with a YouTube-only engine. |
| Google browser sign-in and imported cookies | `DesktopBrowserSignIn.kt`, `DesktopBrowserCookies.kt`, `DesktopYouTubeAuth.kt`, `DesktopYouTubeSession.kt` | Partial: isolated Chromium/manual cookies, signed-in shell validation and session-scope extraction, optional account-menu details, shared signing/header/context path and DPAPI in `auth.rs`. Dummy loopback HTTP tests verify requests; actual account sign-in remains unverified. |
| Multiple Google accounts, channels/brand profiles, renewal | `DesktopAccounts.kt`, `DesktopAccountUi.kt`, `DesktopYouTubeSession.kt` | Partial: protected multi-account container, legacy migration, background revalidated switching/removal and add-account UI. Brand-channel discovery/profile switching and automatic renewal remain missing. Live account verification required. |
| Secrets, logout and persistence | `DesktopSecretStore.kt`, `DesktopWindowsCrypto.kt`, `DesktopPersistence.kt` | Partial: protected auth storage plus library backups/locking. Provider secrets must receive equivalent protection. |
| Lyrics retrieval, timing and provider provenance | `DesktopLyricsClient.kt`, `DesktopEmbeddedLyrics.kt` | Partial: 15 real handlers, embedded/sidecar lyrics, bounded responses, structured timing parsers and actual provenance in `services/lyrics.rs`. Offline fixtures pass; live catalogue/provider availability remains unverified. Automatic lookup scheduling/matching is not identical to upstream. |
| Lyrics source enable/order/reset and manual selection | `DesktopLyricsSourcesUi.kt`, `DesktopLyricsClient.kt` | Implemented paths: persisted enable/order/reset/global switch, predecessor-based order migration and seen-source state, protected PaxSenix key setup, explicit source override and retry. Full GUI/live verification pending. |
| Word/syllable lyrics, translations, language selection | `DesktopTranslationSetting.kt`, `DesktopTranslationLanguageUi.kt`, shared lyrics repository | Partial: word/syllable timestamps and active-word rendering/click-to-seek; lyrics translation through upstream's endpoint (marker batching, same-language detection, Translate toggle, language setting following Windows by default). Romanization, the on-disk translation cache and upstream vocal/section/gap rendering remain missing. |
| Expanded player, artwork mesh/backdrop, side panels | `DesktopPlayerSheet.kt`, `DesktopArtworkMesh.kt`, `DesktopSidePanel.kt`, `DesktopCanvasBackdrop.kt` | Partial: Now Playing sheet slides up/down with upstream sheet timings (360/280 ms); blurred artwork backdrop; wide layout puts artwork + controls beside a Lyrics/Up next panel, narrow layout switches them by tab; the selected track shows optimistically while its stream opens. Drag-to-dismiss, artwork mesh and the page-side lyrics/queue column (`DesktopSidePanel`) are not implemented. |
| Spotify/other Canvas video and credentials | `DesktopCanvas.kt`, `DesktopCanvasSources.kt`, `DesktopSpotifyCanvas.kt`, `DesktopIntegrationsUi.kt` | Missing. |
| Settings overlay/search, appearance, language, animation | `DesktopOverlays.kt`, `DesktopAppearanceSettings.kt`, `DesktopStrings.kt`, `DesktopApp.kt` | Partial: basic Settings page; no equivalent full settings hierarchy/search/theme/localization. |
| Windows chrome, compact/fullscreen modes, visibility | `DesktopWindowChrome.kt`, `DesktopWindowMode.kt`, `DesktopWindowsFrame.kt`, `DesktopWindowVisibility.kt` | Partial: eframe window/expanded player; detailed native behavior requires comparison and GUI tests. |
| Tray menu, close-to-tray and shortcuts | `DesktopTrayMenu.kt`, `DesktopAwtTray.kt`, `DesktopWindowVisibility.kt` | Partial: Windows tray icon and menu (now playing, Play/Pause, Next, Previous, Open, Settings, Quit); opt-in close-to-tray (upstream `keepRunningWhenClosed`, default off) minimizes the window as a tool window because eframe stops updating a hidden window. Window state and continued frames verified by probe; the menu itself was not clicked in an automated test. Linux tray missing. |
| OS media keys, metadata and playback controls | `DesktopWindowsMedia.kt` | Partial: `integrations.rs` via souvlaki; current Windows verification required. |
| Sleep timer and stop after track | `DesktopSleepTimer.kt` | Partial: existing UI implementation; persistence and edge behavior need comparison. |
| Discord rich presence | `DesktopDiscordRpc.kt`, `DesktopDiscordSettings.kt`, `DesktopPresence.kt` | Implemented local Windows IPC path (app ID `1411019391843172514`): opt-in persisted settings, metadata/artwork/buttons/timestamps, clear on pause/stop/disable, bounded background I/O, coalescing, acknowledgement-based status and reconnect. Actual Windows named-pipe fake-server tests pass. Real Discord rendering is unverified; advanced wording/type/status/custom-label controls and token/gateway fallback are not ported. |
| Last.fm and ListenBrainz | `DesktopScrobbling.kt`, `DesktopScrobbleSettings.kt`, `DesktopIntegrationsUi.kt` | Missing authentication, settings and scrobble flow. |
| Listen together/party and host permissions | `DesktopListenTogether.kt`, `DesktopListenTogetherUi.kt`, `DesktopPartySync.kt` | Missing desktop client integration. No new backend is authorized by this mapping. |
| Cache, track logs and diagnostic limits | `DesktopMediaCache.kt`, `DesktopTrackLog.kt`, `DesktopByteBudgetLru.kt` | Partial: runtime/image caching is not a complete user-controlled cache/diagnostics implementation. |
| Update detection, installer and licenses | `DesktopUpdateChecker.kt`, `DesktopLicenses.kt` | Partial: Windows portable packager and GPL license exist; in-app updates/installer/license UI missing. |

## Lyrics provider baseline

The authoritative provider list is `shared/src/jvmSharedMain/kotlin/com/music/bitchord/data/lyrics/LyricsSource.kt`. `DesktopLyricsClient.kt` delegates to the shared repository and also reads embedded local lyrics. Offered providers, in default priority order:

1. lrc.red
2. BiniLyrics
3. BetterLyrics
4. BetterLyrics Portato
5. PaxSenix
6. PaxSenix: Spotify (API key required)
7. PaxSenix: Musixmatch (API key required)
8. LyricsPlus
9. Unison
10. YouTube captions
11. YouTube Music
12. KuGou
13. LRCLIB
14. Musixmatch
15. Genius

SimpMusic and Megalobiz are explicitly hidden upstream and must not be offered merely because their code exists. Provider labels, enabled state, priority repair for newly added providers, word-timing preference, per-track override, and provenance are behavior, not cosmetic settings. Do not label an LRCLIB fallback as the provider the user requested. Failed/disabled providers must not silently produce a successful empty result.

All 15 offered providers now have request/response handlers; this does **not** assert 15 live-service passes. The standalone Musixmatch probe returned HTTP 403, and an earlier LRCLIB probe returned HTTP 503. PaxSenix's keyed routes require a user-provided key stored using Windows DPAPI. Source requests, match quality, word timing and GUI behavior still need representative live verification. SimpMusic/Megalobiz remain hidden.

The keyless-provider smoke returned **77 lyric lines from YouTube Music**. BetterLyrics returned HTTP 401 and Unison had no result for that recording. This proves a live lyrics path, not catalogue-wide success or timed-lyrics coverage for every service.

## Acceptance priorities

1. Safe Windows playback and account flow, no secrets in logs or personal-profile automation.
2. Real provider integrations with persistent configuration, fallback, provenance, retry and offline fixture tests.
3. Library/queue/search/settings behavior and visual comparison against desktop source.
4. Remaining audio, sources, downloads, account profiles and integrations in independently tested vertical slices.
5. Full package verification, current Windows GUI interaction evidence, and honest residual-gap reporting.

## Verification record

Initial Windows checkout check: `cargo test --locked` completed with **21 passed, 0 failed, 6 ignored**. The ignored tests cover external browser/audio/network behavior and are not passes. An existing dead-code warning for `Integrations::available` remains at this baseline. No current end-to-end sign-in, audible playback, pixel comparison or full-parity claim follows from these unit tests.

### Windows package checkpoint (2026-10-31)

Before final lyrics integration, the packaging worker built the release executable and SHA-256-verified the pinned runtime downloads. Native Windows `tar` extracted the actual mpv/FFmpeg `.7z` archives. `scripts/test-package-windows.py`: **2 passed**.

- `powershell -NoProfile -File scripts/windows-smoke.ps1`: native window visible, clean shutdown; independently rerun by the orchestrator with the same result.
- `powershell -NoProfile -File scripts/windows-playback-smoke.ps1`: extracted ZIP, system-only `PATH`, local null-output check passed.
- `powershell -NoProfile -File scripts/windows-playback-smoke.ps1 -Online -RealAudio`: local WAV and YouTube both passed decode/output/pause/seek/volume/speed/resume/stop; report files independently read by the orchestrator.
- Both reports: `ok: true`, `audio_backend: wasapi`, `sample_rate: 48000`. Reports are under ignored `dist/playback-test-results/`.

This proves non-null Windows output for that package checkpoint, not user-heard sound, live account sign-in, latest source integration, GUI interaction parity, or all lyrics providers. Rebuild/repackage and rerun after integration before delivery.

`scripts/windows-ui-smoke.ps1` was subsequently run and independently rerun after fixing DPI-aware window capture. It passed isolated import, persisted local playback history/queue checks, keyboard sequence, resizing and clean shutdown with null output. Report: `dist/ui-test-results/windows-ui-20261031-174238.json`; inspected screenshots show search text and the expanded lyrics panel. These screenshots are not pixel-parity or semantic tab assertions. See [Windows verification](VERIFYING_WINDOWS.md) for the exact boundaries and interactive account/provider checks.

### Cookie/session validation correction (2026-10-31)

- Upstream desktop forwards the pasted header to the Music shell, requires `LOGGED_IN: true`, adopts `SESSION_INDEX`, delegated page, the account half of `DATASYNC_ID`, visitor data and the live client version, then fetches optional profile details. The former Rust flow skipped the shell and required account-menu details using guessed identity; it also required a profile thumbnail and recognized only `runs`, not `simpleText`.
- The manual, account-switch and isolated-browser capture paths now share shell-first validation. Account details may fail without rejecting a confirmed signed-in shell that supplied a stable identity. No fake login success is inferred from merely possessing a signing-cookie name. Saved session metadata is reused by authenticated Home/search/collection requests; account switching persists the refreshed scope.
- Raw header parsing preserves irrelevant unknown names (including malformed export artifacts) rather than interpreting or repairing them. Exact recognized cookie names alone supply signing secrets. Priority is consistently `SAPISID`, `__Secure-3PAPISID`, `__Secure-1PAPISID`, matching upstream. Colons inside values are accepted; duplicate names, embedded extra HTTP headers/control characters and excessive input remain rejected. JSON/Netscape imports retain their domain/name checks.
- Offline regressions use dummy cookies only: opaque raw-header shape, secret priority/hash, shell identity and request headers/body, signed-out rejection, `simpleText` profile without photo, optional account-menu outage and session persistence. A loopback HTTP server observes the actual shell GET and account-menu POST. No user cookie was used, transmitted, stored in fixtures or committed.
- Checks: auth tests **20 passed, 1 ignored**; full suite **63 passed, 8 ignored**; formatting, strict Clippy and Windows release build passed. Real Google sign-in is still an interactive verification, not a claimed pass.
- Limits: no live-account verification; no automatic browser renewal or brand-channel picker. Unlike upstream's last-resort cookie-derived identity, a shell with neither a stable scope ID nor usable account metadata is rejected to avoid conflating saved accounts. Restoration still loads the saved scope rather than refreshing it over the network at UI startup.

### Discord IPC implementation checkpoint (2026-10-31)

- Implemented opt-in Windows local Discord IPC using the upstream application ID; old libraries default to disabled. Presence carries title/artist, available HTTPS artwork, album tooltip, safe YouTube Music/project buttons, and speed-adjusted timestamps. Pause/stop/disable and app drop send a null activity. The worker owns IPC; the GUI only sends bounded settings/activity data.
- `cargo test --locked discord:: -- --nocapture`: **6 passed**. Four tests use actual Windows named pipes in a unique private test namespace: READY, SET_ACTIVITY content/nonce acknowledgement, ping/PONG, unrelated events, coalesced updates, clear, reconnect after restart, shutdown clear, no-response timeout, ERROR rejection, hung-handshake cancellation, invalid READY and oversized-frame rejection. Two tests verify payload privacy/timing and jitter-vs-seek deduplication. No real Discord endpoint, account, token, or personal profile was used.
- Integrated checks: `cargo clippy --locked -- -D warnings`, `cargo test --locked` (**59 passed, 8 ignored**), formatting and Windows release build pass. The optional `--all-targets` Clippy run also exposed pre-existing test-only lints in player/lyrics/store/UI; that stronger check does not pass. Real Discord profile rendering remains an interactive verification, not a claimed test success.
- The rebuilt portable ZIP passed startup, GUI smoke and extracted-package null-output playback. Packaging regression tests: **2 passed**. GUI report: `dist/ui-test-results/windows-ui-20261031-223614.json`; playback: `dist/playback-test-results/discord-null/local.json`. These smoke tests leave presence disabled and do not demonstrate a live Discord profile. Real WASAPI checks below belong to the earlier checkpoint, not this build.

### Earlier integrated playback/lyrics verification (2026-10-31)

- Formatting, strict Clippy, unit tests and Windows release build pass. Unit checkpoint: **53 passed, 0 failed, 8 ignored** (the added keyless smoke is opt-in). Packaging regressions: **2 passed**.
- Explicit bundled-mpv null-output integration passed: EQ/balance apply, bypass/reset, real invalid-filter rejection, pause, seek, volume, speed, resume and natural EOF.
- Live search/browse passed: **20 search tracks, 43 Explore tracks**. The keyless lyrics smoke passed via YouTube Music; explicit LRCLIB and Musixmatch live tests **failed** with HTTP 503 and HTTP 403 respectively. They remain failures, not ignored successes.
- The rebuilt complete Windows ZIP passed native startup, GUI interaction smoke and extracted-package local null-output playback. Local WAV and YouTube both passed real-output-mode checks using **WASAPI, 48 kHz**, with system-only `PATH` and isolated app data.
- Package reports: `dist/playback-test-results/final-null/local.json`, `dist/playback-test-results/final-wasapi/local.json`, `dist/playback-test-results/final-wasapi/youtube.json`. GUI checkpoint: `dist/ui-test-results/windows-ui-20261031-184756.json`.

Real-account sign-in, brand-channel switching, keyed-provider requests, all-source live availability, a listener hearing sound, exclusive-output hardware behavior and full UI/feature parity are **not verified**. The missing feature rows above remain actionable work, not completed scope.
