# Plan 003: Importing an M3U file never touches network or device paths it lists

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat c6cbd32..HEAD -- src/playlist_io.rs docs/USAGE.md`
> If either file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: security
- **Planned at**: commit `c6cbd32`, 2026-11-21

## Why this matters

YTDugem imports `.m3u`/`.m3u8` playlists, a feature meant for files people share. Every non-URL line is treated as a filesystem path, and during import each one is checked with `Path::is_file()`. On Windows, checking a UNC path such as `\\host\share\x.mp3` makes the system connect to that host over SMB/WebDAV and automatically offer the user's NTLM credentials. So simply importing a playlist downloaded from the internet can send the user's password hash to a stranger's server, where it can be cracked or relayed. Skipping network and device paths **before** any filesystem call closes this. Relative entries (resolved against the playlist's own folder) and local drive paths keep working.

## Current state

- `src/playlist_io.rs` — M3U/CSV export and M3U import. `parse_m3u` is pure (it takes a `usable` callback); `import` wires `usable` to the real filesystem.

`src/playlist_io.rs:220-275` (abridged) — the per-line logic in `parse_m3u`:

```rust
        let track = if line.contains("://") {
            youtube_id(line).map(|id| Track { /* online track */ })
        } else {
            let path = PathBuf::from(line);
            let path = match base {
                Some(base) if path.is_relative() => base.join(path),
                _ => path,
            };
            usable(&path).then(|| {
                /* local Track built from `path` */
            })
        };
        match track {
            Some(t) if !parsed.tracks.iter().any(|o| o.key() == t.key()) => parsed.tracks.push(t),
            _ => parsed.skipped += 1,
        }
```

`src/playlist_io.rs:309-312` — `import` passes the real check:

```rust
    let parsed = parse_m3u(&text, path.parent(), |p| {
        p.is_file() && crate::services::is_audio(p)
    });
```

Important compatibility fact: older YTDugem library entries store Windows paths in verbatim form `\\?\C:\Music\a.opus` (see `merge_local_refreshes_stale_imports` in `src/ui.rs`, `mod playlist_tests`), and exported M3U files may contain them. `\\?\<drive letter>:\…` is a local drive and **must stay accepted**.

Conventions: small private helpers with a doc comment explaining *why*; tests in `mod tests` at the bottom of `src/playlist_io.rs`, pattern: `parses_m3u_entries_and_counts_skips` (uses a fake `usable` closure, no filesystem). Tests must pass on Linux CI too, so the helper must work on plain strings, not on `std::path::Component::Prefix` (which only exists on Windows).

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Format check | `cargo fmt --all --check` | exit 0 |
| Lint | `cargo clippy --workspace --locked -- -D warnings` | exit 0 |
| This module's tests | `cargo test --locked playlist_io::` | all pass |
| Full suite | `cargo test --workspace --locked` | all pass |

If `cargo test` fails only in `ui_tests` snapshot image comparisons, rerun with `CI=1` and mention it.

## Scope

**In scope**:
- `src/playlist_io.rs`
- `docs/USAGE.md` (the "Playlist files" bullet, one sentence)
- `plans/README.md` (status row)

**Out of scope**:
- `src/player.rs`, `src/services.rs` — already-imported tracks; separate concern.
- The import path the user types (the `.m3u` file itself may live on a share — the user chose it).
- Restricting entries to the playlist's folder — a stricter policy the owner hasn't asked for.

## Git workflow

- Branch: `advisor/003-m3u-import-skips-network-paths`
- Conventional commit, e.g. `fix(playlists): skip network and device paths in imported M3U files`.
- No `Co-Authored-By` or "Generated with" trailers.
- Do NOT push or open a PR unless instructed.

## Steps

### Step 1: Add the classifier

In `src/playlist_io.rs`, above `parse_m3u`, add:

```rust
/// A path that names another computer or a device: `\\host\share`, `//host/share`,
/// `\\?\UNC\…`, `\\.\…`. Even checking whether one exists makes Windows connect to that host
/// and offer the user's sign-in (NTLM), so a shared playlist must not cause it. A local drive
/// in verbatim form (`\\?\C:\…`, as older imports stored paths) is not one.
fn is_network_or_device(entry: &str) -> bool {
    let entry = entry.replace('/', "\\");
    let Some(rest) = entry.strip_prefix("\\\\") else {
        return false;
    };
    match rest.strip_prefix("?\\") {
        Some(verbatim) => {
            let b = verbatim.as_bytes();
            !(b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\')
        }
        None => true,
    }
}
```

**Verify**: `cargo build --locked` → exit 0.

### Step 2: Skip such entries before `usable` runs

In `parse_m3u`, in the `else` branch (local path), before building `PathBuf::from(line)`, return no track when `is_network_or_device(line)`; it then falls into the existing `skipped += 1` arm. Target shape:

```rust
        } else if is_network_or_device(line) {
            None
        } else {
            let path = PathBuf::from(line);
            /* unchanged */
        };
```

`usable` must never be called for such a line. Update the doc comment on `parse_m3u` to add: "Network and device paths are skipped without being opened."

**Verify**: `cargo test --locked playlist_io::` → existing 6 tests pass.

### Step 3: Tests

Add to `mod tests` in `src/playlist_io.rs`:

1. `network_and_device_paths_are_recognized` — assert `true` for `\\host\share\a.mp3`, `//host/share/a.mp3`, `\\?\UNC\host\share\a.mp3`, `\\.\pipe\x`, `\\?\Volume{abc}\a.mp3`; `false` for `C:\Music\a.mp3`, `\\?\C:\Music\a.mp3`, `\\?\d:\a.mp3`, `sub/song.mp3`, `/home/u/a.mp3`, `..\a.mp3`.
2. `network_entries_are_skipped_without_being_opened` — text with three lines `\\host\share\a.mp3`, `//host/share/b.mp3`, `\\?\C:\Music\c.mp3`; call `parse_m3u(text, None, |p| { assert!(!p.to_string_lossy().contains("host"), "opened {p:?}"); true })`. Assert `skipped == 2` and `tracks.len() == 1` with `source == r"\\?\C:\Music\c.mp3"`.

**Verify**: `cargo test --locked playlist_io::` → 8 passed.

### Step 4: Document the behaviour

In `docs/USAGE.md`, in the **Playlist files** bullet, after "…and missing files, other links and duplicates are skipped and counted.", add: "Network paths (`\\server\share\…`) are skipped without being opened, so a downloaded playlist cannot make Windows contact another computer."

**Verify**: `grep -n "Network paths" docs/USAGE.md` → one line.

## Test plan

- Two new pure tests (Step 3), cross-platform.
- Existing `parses_m3u_entries_and_counts_skips` and `written_m3u_reads_back` unchanged and passing.

## Done criteria

- [ ] `cargo fmt --all --check` exits 0
- [ ] `cargo clippy --workspace --locked -- -D warnings` exits 0
- [ ] `cargo test --workspace --locked` exits 0; `cargo test --locked playlist_io::` shows 8 passed
- [ ] `git status` shows only `src/playlist_io.rs`, `docs/USAGE.md`, `plans/README.md` modified

## STOP conditions

- `parse_m3u` no longer branches on `line.contains("://")` / builds `PathBuf::from(line)` as excerpted.
- An existing test fails after Step 2 (it would mean a real playlist form is now rejected — report which).
- You find exported M3U output (`write_m3u`) that writes paths in a form your classifier rejects other than UNC (check `write_m3u` in the same file).

## Maintenance notes

- If a "play from network share" use case is requested later, make it an explicit opt-in rather than relaxing this check.
- Already-imported library entries are not re-validated; `player.rs` calls `is_file()` on local sources at play time. Out of scope here; a future hardening could apply `is_network_or_device` there too.
- Reviewer focus: `usable` (the filesystem check) is unreachable for rejected lines.
