# YTDugem project guidelines

## Purpose

YTDugem rewrites [BitChord Desktop](https://github.com/kushagrasinghx/BitChord) in Rust, with native egui rendering and mpv playback. **Windows is the supported delivery target for this phase.** Existing Linux code is compatibility, not a parallel product roadmap. No JVM, web frontend, Android target, or new backend is required.

The goal is the closest practical match in desktop features, playback, account behavior, lyrics, navigation, layout, and interaction. This is a goal, not a claim of completed parity. Known gaps are listed in `docs/USAGE.md`.

## Source of truth

- Desktop baseline: upstream `desktop-build-30`, commit `7c673dcdbb5a6f8cc36538eb8813c559e37c26b2`.
- The ignored `references/BitChord/` checkout is read-only. If absent, clone the original repository there. Inspect desktop files using `git show origin/desktop-build-30:<path>` from that checkout; do not switch or edit it.
- Follow desktop call sites into shared code when identifying providers, settings, and behavior. A mobile-only feature is not automatically a desktop requirement.
- Preserve GPL-3.0-only and upstream attribution. Document any copied/adapted implementation and third-party runtime licenses.

## Implementation rules

1. Trace the complete flow and every caller before changing a shared function. Reuse existing helpers and dependencies.
2. Keep network calls, audio startup, downloads, and file scans on background workers. Discard stale results after track/account/request changes.
3. Persist new settings backward compatibly. Preserve corrupt libraries and backups. Never replace invalid data with an empty successful save.
4. Validate remote URLs, IDs, timestamps, sizes, and external-process arguments. Never interpolate user values into shell commands.
5. Treat cookies and provider keys as passwords. Use OS-protected storage, masked input, and explicit logout/removal. Never include secrets in logs, snapshots, reports, or chat.
6. Implement real integrations before enabling their controls. Do not present an empty handler, hard-coded success, or untested endpoint as a completed feature.
7. Preserve keyboard navigation, clear control labels, visible errors, loading/cancel/retry states, and non-blocking UI behavior.
8. Prefer small vertical slices: settings, persistence, service behavior, UI integration, and a runnable regression check together.

## Worker coordination

The orchestrator assigns disjoint files, reviews diffs, resolves integration, and runs final checks. Workers report exact changes, upstream evidence, commands/results, and limitations. They do not create further workers or edit another worker's files without coordination. Never discard unrelated changes.

## Definition of done

- Upstream behavior identified and mapped to a reachable Rust UI path.
- Logic covered by runnable offline tests, including malformed input/failure handling where relevant.
- `cargo fmt --check`, `cargo clippy --locked -- -D warnings`, `cargo test --locked`, and `cargo build --release --locked` pass.
- Runtime/IPC/playback/package changes checked against the actual complete Windows ZIP with a system-only `PATH`.
- Null-output decoding, non-null Windows output, and user-confirmed audible playback are distinct claims. A non-null backend alone does not prove the user heard sound.
- Live provider tests and real-account sign-in reported separately. Missing credentials, network restrictions, and ignored tests are limitations, not passes.
- README and `docs/USAGE.md` describe what was delivered, not what was planned.
