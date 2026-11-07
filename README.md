# YTDugem

A native desktop YouTube Music player written in Rust, rewritten from [BitChord](https://github.com/kushagrasinghx/BitChord). It renders with egui and plays audio through mpv. It needs no JVM, embedded browser, or web frontend.

**Status:** Windows first; Linux is kept working where practical. This is a working rewrite, not yet feature parity with upstream. The [user guide](docs/USAGE.md) lists what has not been ported.

## Download

Download `YTDugem-windows-x86_64.zip` from the artifacts of a [CI run](https://github.com/TazakiN/YTDugem/actions), or build it yourself with `cargo xtask package`. Extract the **whole** archive and run `YTDugem/YTDugem.exe`. Keep the `tools` folder beside the executable, because it contains mpv, yt-dlp, FFmpeg and Deno.

## Build

```sh
cargo run --release
```

Building from source requires mpv, yt-dlp and FFmpeg on `PATH`. See [docs/USAGE.md](docs/USAGE.md) for platform notes, features, and configuration.

## Contributing

Run `cargo ci` before opening a pull request. See [CONTRIBUTING.md](CONTRIBUTING.md) for commands and tests, and [SECURITY.md](SECURITY.md) for reporting vulnerabilities.

## License

[GPL-3.0-only](LICENSE). Based on BitChord by kushagrasinghx and contributors. See [NOTICE](NOTICE) for attribution and bundled third-party software. YTDugem is an independent client and is not affiliated with YouTube or Google.
