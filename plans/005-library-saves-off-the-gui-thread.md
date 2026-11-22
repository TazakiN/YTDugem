# Plan 005: Library saves are coalesced and written off the GUI thread, and flushed on exit

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat c6cbd32..HEAD -- src/ui.rs src/store.rs`
> Plan 004 is expected to have changed both (it adds `open_library` and tests). Any other
> change to the excerpts below is a STOP condition.

## Status

- **Priority**: P2
- **Effort**: M
- **Risk**: MED (persistence timing changes; mitigated by the flush on exit and the plan-004 tests)
- **Depends on**: plans/004-tests-for-library-read-only-and-backup.md (must be DONE)
- **Category**: perf / correctness
- **Planned at**: commit `c6cbd32`, 2026-11-21

## Why this matters

YTDugem (Rust/egui music player) saves its whole library synchronously on the GUI thread on every change: `Library::save` pretty-serializes everything, writes a temp file, **fsyncs**, copies the old file to `.bak`, and renames. Sliders call it on every `.changed()` — i.e. every frame of a drag (7 equalizer bands, crossfade, speed, balance), every keystroke in some text fields, every OS volume event — and every track change saves twice. On Windows an fsync often costs 10–100 ms, so dragging an EQ band stutters, and by the end of a drag `.bak` holds the state from a moment ago, so the "last known good" backup protects nothing. The project rule (AGENTS.md) is "Keep slow … file scans … off the GUI thread." The listening-stats log already solves the same problem with a revision counter and a background writer (`src/ui/stats_page.rs`); this plan applies that proven pattern to the library while keeping all 58 `self.save()` call sites unchanged.

## Current state

Files:
- `src/ui.rs` — app struct `YTDugem` (fields at ~215-385), `with_parts` constructor (~524-660), `fn save` (~671), `enum Event` (~100-200), `fn events` (event match, ~3981+), `impl eframe::App` with `logic` and `on_exit` (~5779-5799).
- `src/store.rs` — `Library::save` (~468-482).
- `src/ui/stats_page.rs` — the exemplar pattern (`changed`, `save(wait, tx)`, `stats_save_step`, `stats_on_exit`).

`src/ui.rs` `fn save` (~671-677):

```rust
    fn save(&mut self) {
        if !self.read_only {
            if let Err(e) = self.library.save(&self.path) {
                self.message = format!("Could not save library: {e:#}");
            }
        }
    }
```

`src/store.rs:468-482`:

```rust
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = path.with_extension("json.tmp");
        let mut file = std::fs::File::create(&temp)?;
        use std::io::Write;
        file.write_all(&serde_json::to_vec_pretty(self)?)?;
        file.sync_all()?;
        // Keep a last-known-good copy; rename replaces files atomically on Windows too.
        if path.exists() {
            std::fs::copy(path, path.with_extension("json.bak"))?;
        }
        std::fs::rename(temp, path).context("Cannot commit library")
    }
```

`Library` is **not** `Clone` (`#[derive(Serialize, Deserialize)]` only), so serialize on the GUI thread and write the bytes in the background.

The exemplar, `src/ui/stats_page.rs:72-108`:

```rust
    fn changed(&mut self) {
        self.revision += 1;
        self.unsaved_since.get_or_insert_with(Instant::now);
    }
    /// Write the log: in the background, or now (on exit). Errors are returned, not shown.
    fn save(&mut self, wait: bool, tx: &Sender<Event>) {
        if self.error.is_some() || self.unsaved_since.take().is_none() {
            return;
        }
        let log = StatsLog { events: self.log.events.clone() };
        let (path, written, revision) = (self.path.clone(), self.written.clone(), self.revision);
        let job = move || -> anyhow::Result<()> {
            let mut last = written.lock().map_err(|_| anyhow::anyhow!("Stats saver failed"))?;
            if *last >= revision {
                return Ok(());
            }
            log.save(&path)?;
            *last = revision;
            Ok(())
        };
        if wait {
            if let Err(e) = job() {
                eprintln!("YTDugem: could not save listening stats: {e:#}");
            }
        } else {
            let tx = tx.clone();
            std::thread::spawn(move || {
                if let Err(e) = job() {
                    let _ = tx.send(Event::StatsSaveFailed(err(e)));
                }
            });
        }
    }
```

The mutex serializes writers; a writer holding an older revision than what's already on disk skips — so the newest state always wins even if threads run out of order.

`logic` and `on_exit` today (`src/ui.rs` ~5781-5794):

```rust
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.events(ctx);
        ...
        self.tray_step(ctx);
        self.stats_save_step();
        forget_idle_images(ctx, self.glass.as_ref().and_then(|g| g.twin()));
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    fn on_exit(&mut self) {
        self.stats_on_exit();
    }
```

`logic` keeps running while the window is hidden in the tray, so a step placed there fires even when no UI is drawn. The `StatsSaveFailed` event arm (`src/ui.rs` ~3999) shows how background failures are surfaced: `self.toast_error(format!(...))`.

`src/ui.rs` already imports `std::sync::{Arc, Mutex}` and `std::time::{Duration, Instant}` (check the `use std::{…}` block at the top).

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Format check | `cargo fmt --all --check` | exit 0 |
| Lint | `cargo clippy --workspace --locked -- -D warnings` | exit 0 |
| Plan-004 + new tests | `cargo test --locked library_safety_tests` | all pass |
| Store tests | `cargo test --locked store::` | all pass |
| Full suite | `cargo test --workspace --locked` | all pass |
| Manual GUI check (Windows) | `cargo run --release` | see Step 6 |

If `cargo test` fails only in `ui_tests` snapshot image comparisons, rerun with `CI=1` and mention it.

## Scope

**In scope**:
- `src/store.rs` — split `Library::save` into serialize + `save_bytes`
- `src/ui.rs` — fields, `with_parts` init, `fn save`, new `library_save_step` / `write_library`, `Event::LibrarySaveFailed`, its event arm, `logic`, `on_exit`, tests in `mod library_safety_tests`
- `plans/README.md` (status row)

**Out of scope** (do NOT touch):
- The 58 `self.save()` call sites — their behaviour changes through `fn save` only. Do not edit them, do not switch sliders to `drag_stopped()`.
- `src/ui/stats_page.rs` — exemplar only.
- `Library::load`, the read-only decision, backup rotation policy beyond what `save_bytes` already does.
- `request_repaint_after(100ms)` in `logic` — a separate finding; leave it.

## Git workflow

- Branch: `advisor/005-library-saves-off-the-gui-thread`
- Conventional commits, e.g. `perf(store): write the library in the background, coalescing rapid changes`.
- No `Co-Authored-By` or "Generated with" trailers.
- Do NOT push or open a PR unless instructed.

## Steps

### Step 1: `store::save_bytes`

In `src/store.rs`, move the body of `Library::save` (from `create_dir_all` to the final `rename`) into:

```rust
/// Writes serialized library bytes the way `Library::save` always has: a synced temp file, the
/// previous file kept as `.bak`, then an atomic rename. Safe to call from a worker thread.
pub fn save_bytes(path: &Path, bytes: &[u8]) -> Result<()> { /* moved body, writing `bytes` */ }
```

and make `Library::save` become `save_bytes(path, &serde_json::to_vec_pretty(self)?)`.

**Verify**: `cargo test --locked store::` → all pass (behaviour unchanged).

### Step 2: State fields

In `struct YTDugem` (next to `read_only`), add:

```rust
    /// Bumped by every library change; the writer skips a snapshot older than what it wrote.
    library_revision: u64,
    /// Set by the first unsaved change; the library is written `LIBRARY_SAVE_AFTER` later.
    library_unsaved_since: Option<Instant>,
    /// The revision on disk. Shared with background writers, which run one at a time.
    library_written: Arc<Mutex<u64>>,
    /// A background write failed: retry on exit even without another change.
    library_save_failed: bool,
```

Initialize in `with_parts` (`0`, `None`, `Arc::default()`, `false`). Add near the other consts at the top of `src/ui.rs`:

```rust
/// Rapid changes (a slider drag, a track change's two saves) become one write.
const LIBRARY_SAVE_AFTER: Duration = Duration::from_secs(1);
```

**Verify**: `cargo build --locked` → exit 0 (dead-code warnings OK until Step 3).

### Step 3: `save` schedules; `write_library` writes

Replace `fn save` and add the writer (model on `stats_page::save`):

```rust
    /// Marks the library changed; `library_save_step` writes it within `LIBRARY_SAVE_AFTER`.
    fn save(&mut self) {
        if !self.read_only {
            self.library_revision += 1;
            self.library_unsaved_since.get_or_insert_with(Instant::now);
        }
    }
    fn library_save_step(&mut self) {
        if self.library_unsaved_since.is_some_and(|t| t.elapsed() >= LIBRARY_SAVE_AFTER) {
            self.write_library(false);
        }
    }
    /// Writes the library: in the background, or now (`wait`, on exit).
    fn write_library(&mut self, wait: bool) {
        let pending = self.library_unsaved_since.take().is_some();
        let retry = std::mem::take(&mut self.library_save_failed);
        if self.read_only || !(pending || retry) {
            return;
        }
        let bytes = match serde_json::to_vec_pretty(&self.library) {
            Ok(b) => b,
            Err(e) => { self.message = format!("Could not save library: {e:#}"); return; }
        };
        let (path, written, revision) = (self.path.clone(), self.library_written.clone(), self.library_revision);
        let job = move || -> anyhow::Result<()> {
            let mut last = written.lock().map_err(|_| anyhow::anyhow!("Library saver failed"))?;
            if *last >= revision {
                return Ok(());
            }
            store::save_bytes(&path, &bytes)?;
            *last = revision;
            Ok(())
        };
        if wait {
            if let Err(e) = job() {
                eprintln!("YTDugem: could not save library: {e:#}");
            }
        } else {
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                if let Err(e) = job() {
                    let _ = tx.send(Event::LibrarySaveFailed(err(e)));
                }
            });
        }
    }
```

Note the revision guard: on a fresh app `*last == 0` and `library_revision == 0` only when nothing changed, in which case `pending` is false and we return early — so the guard never blocks a real first save.

Add the event variant next to `StatsSaveFailed` in `enum Event`:

```rust
    /// Writing `library.json` failed in the background.
    LibrarySaveFailed(String),
```

and its arm next to the `StatsSaveFailed` arm in `fn events`:

```rust
                Event::LibrarySaveFailed(e) => {
                    self.library_save_failed = true;
                    self.message = format!("Could not save library: {e}");
                }
```

(Keep `self.message` — the same surface the old synchronous `save` used.)

**Verify**: `cargo clippy --workspace --locked -- -D warnings` → exit 0.

### Step 4: Drive it from `logic` and flush in `on_exit`

In `logic`, add `self.library_save_step();` right after `self.stats_save_step();`.
In `on_exit`, add `self.write_library(true);` (before or after `self.stats_on_exit();`; either is fine).

Then search for any other way the process ends without `on_exit`: `grep -n "process::exit" src/` must print nothing. If it prints anything, STOP.

**Verify**: `cargo clippy --workspace --locked -- -D warnings` → exit 0; `grep -rn "process::exit" src/` → no output.

### Step 5: Tests

In `src/ui.rs` `mod library_safety_tests` (created by plan 004):

1. Extend `a_read_only_app_never_writes_the_library`: after `app.save()`, also call `app.write_library(true)`; the existing assertions (bytes unchanged, no `.bak`/`.tmp`) must still hold.
2. Add `rapid_changes_are_written_once_by_the_flush` — temp dir, no file. `with_parts(Library::default(), path.clone(), false, …)`. Loop 50 times: `app.library.volume = i as f32; app.save();` (check the field type of `volume` in `store.rs`; it is a float). Assert `!path.exists()` (nothing written synchronously). Call `app.write_library(true)`. Assert `Library::load(&path).unwrap().volume == 49.0`. Call `app.write_library(true)` again and assert it is a no-op (file mtime or bytes unchanged).
3. Add `an_older_snapshot_never_replaces_a_newer_one` — in the same module, write a revision-2 snapshot through the job path, then a revision-1 snapshot, and assert the file still holds revision 2's content. Simplest way: set `app.library.volume = 20.0; app.save(); app.save();` (revision 2), `app.write_library(true)`; then set `*app.library_written.lock().unwrap() = 2` is already true — now set `app.library_revision = 1; app.library.volume = 10.0; app.library_unsaved_since = Some(Instant::now()); app.write_library(true);` and assert loaded volume is still `20.0`.

In `src/store.rs` tests, `backup_holds_the_previous_save` (from plan 004) must still pass unchanged.

**Verify**: `cargo test --locked library_safety_tests` → all pass (5 tests: 3 from plan 004 + 2 new). `cargo test --workspace --locked` → all pass.

### Step 6: Manual check on Windows (report results; do not skip silently)

`cargo run --release`, open Settings → the equalizer, drag a band continuously for ~3 seconds. Expected: no visible stutter, and `library.json` (in `%APPDATA%\BitChord\BitChord\data\`) changes at most about once per second (watch its modified time in Explorer). Close the app with the window's close button and restart: the last EQ value is kept. If you cannot run a GUI, say so in the report — this step is then unverified, not passed.

## Test plan

- New: `rapid_changes_are_written_once_by_the_flush`, `an_older_snapshot_never_replaces_a_newer_one`.
- Extended: `a_read_only_app_never_writes_the_library` (after flush).
- Unchanged and passing: plan 004's tests, all `store::` tests.

## Done criteria

- [ ] `cargo fmt --all --check` exits 0
- [ ] `cargo clippy --workspace --locked -- -D warnings` exits 0
- [ ] `cargo test --workspace --locked` exits 0
- [ ] `grep -n "self.library.save(" src/ui.rs` prints nothing (the GUI thread no longer calls it)
- [ ] `grep -c "self.save()" src/ui.rs` still prints `45` (call sites untouched)
- [ ] `grep -n "write_library(true)" src/ui.rs` includes the line in `on_exit`
- [ ] `git status` shows only in-scope files modified
- [ ] Step 6 result reported (passed, or "not run: no GUI")

## STOP conditions

- Plan 004 is not DONE (no `mod library_safety_tests` / `open_library` in `src/ui.rs`).
- Any code path ends the process without `eframe` calling `on_exit` (`process::exit`, `std::process::abort`, or a tray "Quit" that bypasses `ViewportCommand::Close`) — check `src/tray.rs` and `src/ui.rs` for how Quit closes; report instead of adding ad-hoc flushes.
- Something reads `library.json` back from disk right after a `self.save()` (grep `Library::load` and `data_path()` in `src/ui*`); with delayed writes it would read stale data.
- `cargo test` shows a plan-004 test failing after your change.

## Maintenance notes

- New settings keep calling `self.save()`; nothing else is needed.
- Anything that must be on disk *now* (e.g. before spawning another process that reads `library.json`) must call `self.write_library(true)` first.
- Loss window: if Windows ends the session without eframe running `on_exit`, up to ~1 s of changes may be lost (previously 0). Revisit with a `WM_ENDSESSION` flush if users report it.
- Reviewer focus: the revision guard (newest wins), `library_save_failed` retry on exit, and that read-only mode short-circuits before serializing.
- Deferred: sliders could also commit on `drag_stopped()`; with coalescing it's no longer needed for performance.
