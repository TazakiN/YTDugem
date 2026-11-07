# Contributing to YTDugem

Thank you for helping improve YTDugem. It is a native Rust (egui) desktop music player and a rewrite of [BitChord](https://github.com/kushagrasinghx/BitChord). The project targets Windows first; Linux is kept working where practical.

Before contributing, read [AGENTS.md](AGENTS.md) and [docs/PROJECT_GUIDELINES.md](docs/PROJECT_GUIDELINES.md).

## Prerequisites

- Rust stable, installed with [rustup](https://rustup.rs).
- Windows: the MSVC toolchain, which requires the Visual Studio Build Tools (C++ workload).
- Linux (Debian/Ubuntu):

  ```sh
  sudo apt install mpv yt-dlp ffmpeg libxkbcommon0 libgl1
  ```

  The GUI needs a graphical X11 or Wayland session.
- Python 3 and 7-Zip are needed only to package the Windows ZIP.

## Build and run

```sh
cargo run
```

Runtime discovery checks a `tools` folder beside the executable. Without it, install mpv, yt-dlp and FFmpeg yourself, or point the app at them with `BITCHORD_MPV` and `BITCHORD_YTDLP`.

## Commands

| Command | Purpose |
|---|---|
| `cargo ci` or `cargo xtask ci` | Same checks as CI: format check, clippy with `-D warnings`, tests, release build |
| `cargo lint` | Clippy with `-D warnings` |
| `cargo xtask package` | Builds the Windows portable ZIP through `scripts/package-windows.py`. Needs Python and 7-Zip; set `SEVENZIP` to override the 7-Zip path |
| `cargo xtask smoke [--ui] [--playback] [--online] [--real-audio]` | Windows smoke tests against the packaged ZIP |

Run `cargo ci` before opening a pull request.

## Integration tests

These tests need runtime tools or network access. Run the ones relevant to your change:

```sh
BITCHORD_AUDIO_OUTPUT=null cargo test plays_local_audio -- --ignored --nocapture
cargo test live_music_search_and_browse -- --ignored --nocapture
cargo test live_lyrics -- --ignored --nocapture
cargo test plays_youtube_audio -- --ignored --nocapture
dbus-run-session -- cargo test live_mpris -- --ignored --nocapture
```

Linux GUI smoke test (requires Xvfb, xauth, xdotool and ffmpeg):

```sh
cargo build --locked && python3 scripts/gui-smoke.py
```

Optional variables: `BITCHORD_TEST_OUTPUT` (screenshot directory), `BITCHORD_TEST_BINARY` (binary to test), `BITCHORD_TEST_DISPLAY=:0` (existing display).

Windows playback diagnostic for a local file or HTTPS stream:

```powershell
.\YTDugem.exe --check-playback "C:\Music\track.flac" "C:\Temp\playback-report.json"
```

Notes on test results:

- Null audio output (`BITCHORD_AUDIO_OUTPUT=null`) verifies decoding and playback state only. It does not prove that sound was audible.
- `-RealAudio` / `--real-audio` requires a non-null audio backend.
- An `ignored` result is not a pass. Read the output to confirm the test ran.
- Live provider tests depend on external services and may fail for reasons unrelated to your change. Report them separately.

## Guidelines

- Keep changes small and focused. Do not reformat unrelated code.
- Keep slow work (network requests, downloads, file scans, audio startup) off the GUI thread.
- Add tests for logic changes, including malformed input and failure cases.
- Match upstream BitChord desktop behavior, and list any remaining gaps in [docs/USAGE.md](docs/USAGE.md).
- Never commit credentials, cookies or API keys. Never use a personal browser profile or account in tests.
- AI-assisted contributions must follow [AGENTS.md](AGENTS.md).

## Pull request flow

1. Create a branch from `main`.
2. Make your change and run `cargo ci`.
3. Open a pull request using the template. Describe the change, how you tested it, and any limitations.

## License

YTDugem is licensed under GPL-3.0-only (see [LICENSE](LICENSE)). Contributions are accepted under the same license. Keep attribution to the original BitChord project when you adapt its code, and document any copied or adapted implementation.
