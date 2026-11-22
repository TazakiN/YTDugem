# Plan 004: Tests pin down that a corrupt library is never overwritten and that `.bak` holds the previous save

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat c6cbd32..HEAD -- src/ui.rs src/store.rs`
> If either file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1 (prerequisite for plan 005)
- **Effort**: S
- **Risk**: LOW (one tiny extraction; otherwise test-only)
- **Depends on**: none
- **Category**: tests
- **Planned at**: commit `c6cbd32`, 2026-11-21

## Why this matters

The project's top data rule (AGENTS.md) is "Never silently overwrite a corrupt music library." Today it is enforced in only two places in the 7,000-line `src/ui.rs` — the load in `YTDugem::new` turns saving off when `library.json` is invalid, and `fn save` checks `read_only` — and **no test covers either**. The store test only checks that a `.bak` file exists, not what it contains. Plan 005 will rewrite how saving works (moving it off the GUI thread); these tests must exist first so that refactor cannot silently break the rule.

## Current state

- `src/ui.rs` — the egui app. `YTDugem` is the app struct; `YTDugem::new(cc)` needs an eframe `CreationContext`, so it can't be called in tests, but `YTDugem::with_parts(...)` can (used by `src/ui/ui_tests.rs:76-95`).
- `src/store.rs` — `Library::load` / `Library::save` and its tests (`mod tests`, `library_roundtrip_and_corruption_is_not_silently_discarded` at ~line 630).

`src/ui.rs:453-462` today:

```rust
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let path = store::data_path();
        let (library, mut message, read_only) = match Library::load(&path) {
            Ok(l) => (l, String::new(), false),
            Err(e) => (
                Library::default(),
                format!("{e:#}. Saving disabled to preserve your library."),
                true,
            ),
        };
```

`src/ui.rs:671-677`:

```rust
    fn save(&mut self) {
        if !self.read_only {
            if let Err(e) = self.library.save(&self.path) {
                self.message = format!("Could not save library: {e:#}");
            }
        }
    }
```

`src/store.rs:468-482` — `Library::save`: writes `library.json.tmp`, `sync_all`, copies the existing `library.json` to `library.json.bak`, then renames tmp over `library.json`.

`YTDugem::with_parts` signature (`src/ui.rs:524-532`):

```rust
    fn with_parts(
        library: Library,
        path: PathBuf,
        read_only: bool,
        message: String,
        account: Option<Account>,
        stats: stats_page::Stats,
        integrations: Integrations,
    ) -> Self
```

How `ui_tests.rs` builds one (copy this):

```rust
    let stats = stats_page::Stats::load(&path);
    let mut app = YTDugem::with_parts(library, path, true, String::new(), None, stats, Integrations::new());
```

Test-module convention in `src/ui.rs`: several `#[cfg(test)] mod <name> { use super::*; ... }` blocks (e.g. `mod playlist_tests` at ~line 5555). Temp dirs: `std::env::temp_dir().join(format!("<prefix>-{}", std::process::id()))`, removed at the end.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Format check | `cargo fmt --all --check` | exit 0 |
| Lint | `cargo clippy --workspace --locked -- -D warnings` | exit 0 |
| New ui tests | `cargo test --locked library_safety_tests` | all pass |
| Store tests | `cargo test --locked store::` | all pass |
| Full suite | `cargo test --workspace --locked` | all pass |

If `cargo test` fails only in `ui_tests` snapshot image comparisons, rerun with `CI=1` and mention it.

## Scope

**In scope**:
- `src/ui.rs` — extract `open_library`, add `mod library_safety_tests`
- `src/store.rs` — one new test in `mod tests`
- `plans/README.md` (status row)

**Out of scope**:
- Changing `Library::save`/`Library::load` behaviour or `fn save` — that is plan 005 / a separate finding.
- `src/ui/ui_tests.rs` (snapshot tests).

## Git workflow

- Branch: `advisor/004-tests-for-library-read-only-and-backup`
- Conventional commit, e.g. `test(store): pin down read-only mode and backup contents`.
- No `Co-Authored-By` or "Generated with" trailers.
- Do NOT push or open a PR unless instructed.

## Steps

### Step 1: Extract the load decision into a testable function

In `src/ui.rs`, add a free function (near `merge_local`, outside `impl`):

```rust
/// The library at `path`, the message to show, and whether saving must stay off. A library
/// that exists but cannot be read is kept as it is on disk: the app starts empty and read-only.
fn open_library(path: &Path) -> (Library, String, bool) {
    match Library::load(path) {
        Ok(l) => (l, String::new(), false),
        Err(e) => (
            Library::default(),
            format!("{e:#}. Saving disabled to preserve your library."),
            true,
        ),
    }
}
```

and in `YTDugem::new` replace the `match` with `let (library, mut message, read_only) = open_library(&path);`. No behaviour change.

**Verify**: `cargo clippy --workspace --locked -- -D warnings` → exit 0.

### Step 2: UI-level tests

Add at the end of the `#[cfg(test)]` blocks in `src/ui.rs`:

```rust
#[cfg(test)]
mod library_safety_tests {
    use super::*;
    // tests below
}
```

Tests:
1. `a_corrupt_library_opens_read_only_and_untouched` — temp dir, write `library.json` = `b"{ not json"`. `let (lib, message, read_only) = open_library(&path);` Assert `read_only`, `message.contains("Saving disabled")`, `lib.history.is_empty()`, and file bytes still `b"{ not json"`.
2. `a_read_only_app_never_writes_the_library` — temp dir, write `library.json` = `b"{ not json"`. Build `YTDugem::with_parts(Library::default(), path.clone(), true, String::new(), None, stats_page::Stats::load(&path), Integrations::new())`. Push a track into `app.library.liked` (any `Track { id: "x".into(), ..Default::default() }`), call `app.save()`. Assert `library.json` bytes unchanged, and that `library.json.bak` and `library.json.tmp` do **not** exist.
3. `a_missing_library_opens_writable` — temp dir with no file: `open_library` returns `read_only == false` and empty message.

Clean up each temp dir at the end of each test (`let _ = std::fs::remove_dir_all(&dir);`). Use a distinct dir name per test (append the test name).

**Verify**: `cargo test --locked library_safety_tests` → 3 passed.

### Step 3: Store-level backup content test

In `src/store.rs` `mod tests`, add `backup_holds_the_previous_save` — temp dir; save library A (e.g. `volume = 10.0`), read bytes `a`; save library B (`volume = 20.0`), read bytes `b`. Assert `library.json` == `b` and `library.json.bak` == `a`, and `Library::load` returns volume 20.0.

**Verify**: `cargo test --locked store::` → all pass, including the new test.

## Test plan

- 3 new tests in `src/ui.rs` (`mod library_safety_tests`), 1 in `src/store.rs`. No network, no mpv, no real data dir.

## Done criteria

- [ ] `cargo fmt --all --check` exits 0
- [ ] `cargo clippy --workspace --locked -- -D warnings` exits 0
- [ ] `cargo test --workspace --locked` exits 0, including the 4 new tests
- [ ] `grep -n "fn open_library" src/ui.rs` prints one line, and `YTDugem::new` calls it
- [ ] `git status` shows only in-scope files modified

## STOP conditions

- `YTDugem::with_parts` panics or needs a window/GPU in a plain unit test (it is used by `ui_tests.rs`, so it should not).
- Test 2 fails on current code — that would be a real bug in read-only mode; report it with the observed file state instead of changing product code.
- The load/`save` excerpts differ from the live code.

## Maintenance notes

- Plan 005 changes `fn save` to schedule a background write. It must keep test 2 passing **after** its flush function runs (plan 005 tells the executor to extend test 2 to call the flush too).
- If backup rotation changes (e.g. keeping several backups), update `backup_holds_the_previous_save` deliberately; it encodes the current one-deep contract.
