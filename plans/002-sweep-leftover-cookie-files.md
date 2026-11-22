# Plan 002: Plaintext session cookie files left by a crash are deleted at startup and at sign-out

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat c6cbd32..HEAD -- src/auth.rs src/main.rs`
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

YTDugem (Rust/egui music player, Windows-first) keeps the YouTube sign-in sealed with AES-GCM under a key in Windows Credential Manager, and the UI tells users so. But to hand the session to yt-dlp and mpv it writes **plaintext** Netscape cookie files into the `auth/` folder beside the library, and browser imports and the sign-in window create `import-*` / `signin-*` folders there (the import folder holds yt-dlp's decrypted `cookies.txt`). These are removed only by `Drop` impls. After a crash, forced kill or power loss they stay forever — nothing sweeps them — and `logout()` doesn't remove them either. Anyone who can read the user's profile can then lift a live YouTube/Google session. Deleting the leftovers at startup (when the single-instance lock guarantees no other YTDugem uses them) and at sign-out closes that gap.

## Current state

- `src/auth.rs` — sign-in, sealed session, cookie files.
- `src/main.rs` — startup: takes the single-instance lock, then calls `update::cleanup()`.

`src/auth.rs:1510-1521` — the folder:

```rust
fn auth_dir() -> Result<PathBuf> {
    let p = crate::store::data_path()
        .parent()
        .ok_or_else(|| anyhow!("Could not locate application data directory"))?
        .join("auth");
    fs::create_dir_all(&p)?;
    ...
}
```

Things created inside `auth/`:
- `session.json` — the sealed session (`src/auth.rs:324`). **Must never be deleted by this plan.**
- `cookies-<pid>-<n>.txt` — plaintext cookie files from `cookie_file()` / `new_cookie_file()` (`src/auth.rs:1537-1568`), removed by `impl Drop for CookieFile`.
- `import-<pid>-<nanos>/` — browser import folder containing `cookies.txt` (`src/auth.rs:980-985`), removed by `impl Drop for AuthProfile`.
- `signin-<pid>-<nanos>/` — throwaway browser profile for the sign-in window (`src/auth.rs:1062-1066`), same `Drop`.

`src/auth.rs:283-291` — sign-out:

```rust
/// Forgets the session and removes the saved copy.
pub fn logout() -> Result<()> {
    let mut state = active()
        .lock()
        .map_err(|_| anyhow!("auth state unavailable"))?;
    secretfile::delete()?;
    *state = None;
    GENERATION.fetch_add(1, Ordering::SeqCst);
    Ok(())
}
```

`src/main.rs:63-66` — startup order:

```rust
    let after_update = args.iter().any(|s| s == update::AFTER_UPDATE);
    let _instance =
        lock_instance(after_update).map_err(|e| eframe::Error::AppCreation(e.into()))?;
    update::cleanup();
```

The lock is per data folder (`store::lock(&store::data_path())`), and `auth/` sits in that same folder, so once the lock is held, no other YTDugem GUI uses `auth/`.

Conventions: best-effort cleanup ignores errors with `let _ =` (see `update::cleanup`, `AuthProfile::drop`); doc comments explain *why*. Tests in `src/auth.rs`'s `mod tests` create unique temp dirs — pattern: `concurrent_cookie_files_get_their_own_names_and_replace_stale_ones` (`src/auth.rs:1782-1806`). Never use the real data dir or `BITCHORD_DATA_DIR` in a new test.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Format check | `cargo fmt --all --check` | exit 0 |
| Lint | `cargo clippy --workspace --locked -- -D warnings` | exit 0 |
| This module's tests | `cargo test --locked auth::` | all pass |
| Full suite | `cargo test --workspace --locked` | all pass |

If `cargo test` fails only in `ui_tests` snapshot image comparisons, rerun with `CI=1` and mention it.

## Scope

**In scope**:
- `src/auth.rs`
- `src/main.rs` (one added call)
- `plans/README.md` (status row)

**Out of scope** (do NOT touch):
- `session.json` handling and `secretfile` — the sealed store is correct.
- Moving `auth/` out of Roaming AppData, or changing what yt-dlp writes during browser import — separate, larger findings.
- The `--check-playback` path in `main.rs` (runs before the lock; leave it alone).

## Git workflow

- Branch: `advisor/002-sweep-leftover-cookie-files`
- Conventional commit, e.g. `fix(auth): delete session cookie files a crash left behind`.
- No `Co-Authored-By` or "Generated with" trailers.
- Do NOT push or open a PR unless instructed.

## Steps

### Step 1: Add a pure sweep over a given folder

In `src/auth.rs`, near `auth_dir()`, add:

```rust
/// Deletes plaintext copies of the session that only exist while something uses them:
/// `cookies-*.txt` files and `import-*` / `signin-*` folders. A crash leaves them behind, and
/// their contents are not sealed like `session.json`, which is kept.
fn sweep_plaintext(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let path = entry.path();
        if name.starts_with("cookies-") && name.ends_with(".txt") {
            let _ = fs::remove_file(&path);
        } else if name.starts_with("import-") || name.starts_with("signin-") {
            let _ = fs::remove_dir_all(&path);
        }
    }
}
```

Match only these exact prefixes/suffix. Do not touch anything else.

**Verify**: `cargo build --locked` → exit 0 (dead-code warning OK until Step 2).

### Step 2: Public entry points — startup and sign-out

Add:

```rust
/// At startup, with the single-instance lock held: nothing else can be using these files.
pub fn sweep_leftovers() {
    if let Some(dir) = crate::store::data_path().parent() {
        sweep_plaintext(&dir.join("auth"));
    }
}
```

(Do not call `auth_dir()` here: it creates the folder.)

In `src/main.rs`, immediately after `update::cleanup();`, add `auth::sweep_leftovers();`.

In `logout()`, after `secretfile::delete()?;` and before `*state = None;`, add:

```rust
    // Copies still handed to a running yt-dlp or mpv go too: the session is being revoked.
    if let Ok(dir) = auth_dir() {
        sweep_plaintext(&dir);
    }
```

Note: deleting a cookie file that a running mpv/yt-dlp has open is acceptable on sign-out (the user is revoking the session); `CookieFile::drop` ignores the now-missing file.

**Verify**: `cargo clippy --workspace --locked -- -D warnings` → exit 0.

### Step 3: Test

Add to `mod tests` in `src/auth.rs`:

`sweep_removes_plaintext_copies_and_keeps_the_sealed_session` — make a unique temp dir (copy the naming from `concurrent_cookie_files_get_their_own_names_and_replace_stale_ones`). Create:
- `session.json` (content `"sealed"`), `cookies-123-0.txt`, `cookies-999-7.txt`, `notes.txt`, `cookies-backup.json`
- folders `import-123-456/` containing `cookies.txt`, `signin-1-2/` containing `Default/x`, and `other/`

Call `sweep_plaintext(&dir)`. Assert: `session.json`, `notes.txt`, `cookies-backup.json`, `other/` still exist; both `cookies-*.txt`, `import-123-456/`, `signin-1-2/` are gone. Also call `sweep_plaintext` on a non-existent path and assert it does not panic. Remove the temp dir at the end.

**Verify**: `cargo test --locked auth::tests::sweep_removes` → 1 passed.

## Test plan

- New unit test above (pure function, temp dir, no network, no real data dir).
- Full: `cargo test --workspace --locked` → all pass.

## Done criteria

- [ ] `cargo fmt --all --check` exits 0
- [ ] `cargo clippy --workspace --locked -- -D warnings` exits 0
- [ ] `cargo test --workspace --locked` exits 0, including the new test
- [ ] `grep -n "auth::sweep_leftovers()" src/main.rs` prints exactly one line, located after `update::cleanup();`
- [ ] `grep -n "sweep_plaintext" src/auth.rs` shows the definition, the call in `sweep_leftovers`, the call in `logout`, and the test
- [ ] `git status` shows only in-scope files modified

## STOP conditions

- `auth_dir()` or the file name patterns (`cookies-{pid}-{n}.txt`, `import-…`, `signin-…`) differ from the excerpts.
- You find any other file type in `auth/` written by the code (grep `auth_dir()` callers) that a prefix above would match by accident.
- `main.rs` no longer takes the instance lock before `update::cleanup()` — then the startup sweep could delete files of a running instance.

## Maintenance notes

- Any new plaintext artifact under `auth/` must either use one of these prefixes or be added to `sweep_plaintext`.
- `--check-playback` mode runs without the lock; if it ever runs concurrently with a GUI start, its cookie file could be swept. It is a test mode; acceptable.
- Deferred: put the per-run cookie files in non-roaming local app data, and stop yt-dlp writing the whole browser cookie jar during browser import (`--cookies-from-browser … --cookies <file>`), both larger changes.
