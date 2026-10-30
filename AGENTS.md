# Agent Guidelines

## Goals and Scope

- This project is a **Rust rewrite of BitChord desktop**, targeting Windows and Linux.
- Do not expand the scope to Android, TV, Wear OS, web, or backend development unless the user requests it.
- Do not claim feature parity with upstream until implementation and testing demonstrate it.
- Preserve the GPL-3.0 license and attribution to the original BitChord project.

## Upstream Reference

- Original repository: `references/BitChord/`, cloned from https://github.com/kushagrasinghx/BitChord.
- Treat this directory as a read-only reference. Do not modify it, clean it, or switch its branch unless requested.
- The reference checkout uses `main`. For desktop code missing from that checkout, inspect the remote branch `origin/desktop-build-30` using `git show` or `git ls-tree`.
- Application implementation belongs in `src/`, not in the reference directory.

## Code Map

- `src/main.rs`: startup, WSL graphics configuration, and playback-check mode.
- `src/ui.rs`: native egui/eframe interface and background work.
- `src/player.rs`: mpv process management, IPC, controls, and audio status.
- `src/runtime.rs`: runtime discovery through the `tools` folder beside the executable.
- `src/services.rs`: YouTube Music search, local imports, lyrics, and downloads.
- `src/store.rs`: music library persistence, backups, and inter-instance locking.
- `src/model.rs`: shared data models.
- `src/integrations.rs`: system media controls.
- `src/playback_check.rs`: audio checks using the same engine as the GUI.
- `scripts/`: packaging, installation, and smoke tests. `dist/` and `target/` contain build outputs.

## Working Rules

- Make the smallest change that meets the requirement; prefer the standard library or existing dependencies.
- Avoid unnecessary abstractions, dependencies, configuration, and changes outside the request.
- Do not overwrite user work or discard changes you did not make.
- Keep slow network operations, file scans, downloads, and audio startup off the GUI thread.
- Preserve input validation, error handling, and data protection. Never silently overwrite a corrupt music library.
- Run external programs with separate arguments, not shell-interpolated user input.
- Include runnable tests for logic changes. Distinguish compilation checks, decoder tests, and real audio-output tests.
- Use EGM only when the user requests it or design decisions require it. While a design workflow is active, follow its approval gates before writing implementation code.

## Basic Checks

Run from the project root:

```sh
cargo fmt --check
cargo clippy --locked -- -D warnings
cargo test --locked
cargo build --release --locked
```

Integration tests require runtime tools or network access; an `ignored` result does not prove that a test passed:

```sh
BITCHORD_AUDIO_OUTPUT=null cargo test plays_local_audio -- --ignored --nocapture
cargo test plays_youtube_audio -- --ignored --nocapture
cargo test live_music_search_and_browse -- --ignored --nocapture
cargo test live_lyrics -- --ignored --nocapture
```

Linux GUI smoke test: `python3 scripts/gui-smoke.py`; requires Xvfb, xauth, xdotool, and FFmpeg.

## Windows and Distribution

- Distribute the complete portable ZIP, **not just the `.exe`**. Keep the `tools` folder beside `BitChord.exe`.
- Bundled runtime tools: mpv, yt-dlp, FFmpeg, ffprobe, and Deno. Do not rely on tools installed on the developer's machine or its `PATH`.
- Package with `python scripts/package-windows.py` after building for Windows; requires 7zip. Preserve SHA-256 verification of runtime downloads.
- After changing runtime discovery, IPC, playback, or packaging, test the actual ZIP that will be delivered to users:

```powershell
powershell -NoProfile -File scripts/windows-smoke.ps1
powershell -NoProfile -File scripts/windows-playback-smoke.ps1
powershell -NoProfile -File scripts/windows-playback-smoke.ps1 -Online -RealAudio
```

- Playback tests extract the ZIP into a fresh folder, restrict `PATH` to system directories, and check decoding, audio output, pause, seek, volume, speed, resume, and stop.
- Null mode verifies playback without speakers; do not present it as proof of real audio output. The `-RealAudio` mode must use a non-null backend.
- Report commands, results, and verification limits honestly. Successfully building for Windows from Linux does not prove that Windows playback works.
