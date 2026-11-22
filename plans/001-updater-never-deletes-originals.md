# Plan 001: A failed in-app update never deletes the installed app, and never claims "unchanged" when it isn't

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**:
> `git diff --stat c6cbd32..HEAD -- src/update.rs src/ui/updater.rs verification/update-swap scripts/verify-update-swap.ps1 .claude/skills/verify-update-swap/SKILL.md`
> If any of these changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: LOW (only failure paths change; the success path renames are identical)
- **Depends on**: none
- **Category**: bug (data loss)
- **Planned at**: commit `c6cbd32`, 2026-11-21

## Why this matters

YTDugem (a Rust/egui music player, Windows-first) updates its portable folder in place: it renames each installed top-level item into `.update/old`, renames the new one in, and on failure renames everything back. Two rollback renames ignore their result (`let _ = fs::rename(...)`), and every retry starts with `let _ = fs::remove_dir_all(&old)`. If the rename that puts the installed `YTDugem.exe` back fails (an antivirus scanner holding it), the next retry deletes the only copy of the user's exe, and the final error still says "the app was left unchanged". The project already has a TLA+ model of this swap that reproduces the bug (`real-rollback-fails` model). This plan fixes the code so that (a) no original is ever deleted before an update succeeds and (b) the error never claims "unchanged" when a rollback rename failed. It then updates the model, which has been prototyped and passes TLC.

## Current state

Files:
- `src/update.rs` — release check, download, `apply` + `swap` (the folder swap), `cleanup` at next start. Tests at the bottom (`mod tests`).
- `src/ui/updater.rs` — `apply_update` spawns a thread that calls `update::apply` once plus up to 5 retries.
- `verification/update-swap/UpdateSwap.tla`, `MC.tla`, `*.cfg` — TLA+ model of `apply`/`swap`/retry loop.
- `scripts/verify-update-swap.ps1` — runs TLC on every `.cfg` and compares against expected outcomes; then checks drift since the "Sync commit" in the skill file.
- `.claude/skills/verify-update-swap/SKILL.md` — documents the model, coverage record, the open finding, and the `Sync commit:` line the script reads.

`src/update.rs:245-290` today:

```rust
pub fn apply(dir: &Path, staged: &Path) -> Result<()> {
    check_package(staged)?;
    let old = dir.join(STAGING).join("old");
    let _ = fs::remove_dir_all(&old);
    fs::create_dir_all(&old)?;
    let mut names: Vec<OsString> = fs::read_dir(staged)?
        .map(|entry| entry.map(|e| e.file_name()))
        .collect::<std::io::Result<_>>()?;
    names.sort();
    swap(dir, staged, &old, names)
}

fn swap(dir: &Path, staged: &Path, old: &Path, names: Vec<OsString>) -> Result<()> {
    let mut moved: Vec<(OsString, bool)> = vec![];
    for name in names {
        let current = dir.join(&name);
        let had = current.symlink_metadata().is_ok();
        let step = || -> std::io::Result<()> {
            if had {
                fs::rename(&current, old.join(&name))?;
            }
            if let Err(e) = fs::rename(staged.join(&name), &current) {
                if had {
                    let _ = fs::rename(old.join(&name), &current);
                }
                return Err(e);
            }
            Ok(())
        };
        if let Err(e) = step() {
            for (name, had) in moved.iter().rev() {
                let _ = fs::rename(dir.join(name), staged.join(name));
                if *had {
                    let _ = fs::rename(old.join(name), dir.join(name));
                }
            }
            return Err(e).with_context(|| {
                format!(
                    "Could not replace {}; the app was left unchanged",
                    name.to_string_lossy()
                )
            });
        }
        moved.push((name, had));
    }
    Ok(())
}
```

`src/ui/updater.rs:148-162` today:

```rust
        std::thread::spawn(move || {
            // A stream lookup may still be running a bundled tool; give it a few seconds.
            let mut result = update::apply(&dir, &staged);
            for _ in 0..5 {
                if result.is_ok() {
                    break;
                }
                std::thread::sleep(Duration::from_secs(1));
                result = update::apply(&dir, &staged);
            }
            if result.is_ok() {
                update::record_installed_version(&dir, &version);
            }
            let _ = tx.send(Event::Update(UpdateEvent::Applied(result.map_err(err))));
        });
```

`STAGING` is `".update"` (`src/update.rs:27`). `cleanup()` (called at startup in `src/main.rs:66`) deletes `.update` at the next start; do not change it.

Conventions: `anyhow::Result` with `bail!`/`.context()`; doc comments are full sentences explaining *why*; tests in `#[cfg(test)] mod tests` at the bottom of the file, building fake app folders in `std::env::temp_dir()` — copy the style of `a_failed_swap_restores_the_app_folder` (`src/update.rs` near the end) and its helper `package(root, marker)`.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Format check | `cargo fmt --all --check` | exit 0 |
| Lint | `cargo clippy --workspace --locked -- -D warnings` | exit 0 |
| Unit tests (this module) | `cargo test --locked update::` | all pass |
| Full suite | `cargo test --workspace --locked` | all pass (ignored tests stay ignored) |
| Model check (needs Java 11+) | `powershell -NoProfile -File scripts/verify-update-swap.ps1` | exit 0, four `OK` lines |

If `cargo test` fails only in `ui_tests` snapshot comparisons (image diffs) and you touched no drawing code, rerun with the environment variable `CI=1` (it skips image comparison) and mention it in your report.

## Suggested executor toolkit

- The project skill `verify-update-swap` (`.claude/skills/verify-update-swap/SKILL.md`) — read it fully before Step 5; it explains how to read TLC output and the sync-commit procedure.

## Scope

**In scope** (the only files you should modify):
- `src/update.rs`
- `src/ui/updater.rs`
- `verification/update-swap/UpdateSwap.tla`
- `verification/update-swap/real-rollback-ok.cfg`, `real-rollback-fails.cfg`, `broken-no-inline-undo.cfg`, `broken-no-rollback-old.cfg`
- `scripts/verify-update-swap.ps1` (only the `$expected` table)
- `.claude/skills/verify-update-swap/SKILL.md`
- `plans/README.md` (status row)

**Out of scope** (do NOT touch):
- `update::cleanup()` and `download()` — not part of this bug; `cleanup` deleting `.update` at the next start is outside the model by design.
- `verification/update-swap/MC.tla` — the package items do not change.
- The success path of `swap` (the order and arguments of the forward renames) — the model and existing tests depend on it.
- Anything about update signing/size limits — separate findings.

## Git workflow

- Branch: `advisor/001-updater-never-deletes-originals`
- Conventional commits, matching `git log` (e.g. `fix(ui): inset recent covers so hover highlight frames them`). Use e.g. `fix(update): never delete the installed app when a rollback rename fails`.
- Do NOT add `Co-Authored-By` or "Generated with" trailers (owner preference).
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Add the `NotRestored` error type

In `src/update.rs`, after `parse_checksum`, add:

```rust
/// An update attempt that could not put every replaced item back: some are still in
/// `.update/old`. Its text never says the app was left unchanged.
#[derive(Debug)]
pub struct NotRestored(pub String);

impl std::fmt::Display for NotRestored {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NotRestored {}
```

**Verify**: `cargo build --locked` → exit 0 (a dead-code warning is fine until Step 3).

### Step 2: `apply` restores leftovers instead of deleting them

Add a `restore` function and change `apply`. Target shape:

```rust
/// Puts back what an earlier attempt, whose rollback failed, left in `old`. Nothing in `old`
/// is deleted here: while anything cannot go back, the update stops, so an original item is
/// never destroyed before an update succeeds.
fn restore(dir: &Path, old: &Path) -> Result<()> {
    let entries = match fs::read_dir(old) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    for entry in entries {
        let name = entry?.file_name();
        let current = dir.join(&name);
        if current.symlink_metadata().is_ok() || fs::rename(old.join(&name), &current).is_err() {
            return Err(NotRestored(format!(
                "An earlier update attempt left {} in {}. Nothing more was changed; \
                 if YTDugem is missing it, copy it back into {}",
                name.to_string_lossy(),
                old.display(),
                dir.display()
            ))
            .into());
        }
    }
    Ok(())
}

pub fn apply(dir: &Path, staged: &Path) -> Result<()> {
    let old = dir.join(STAGING).join("old");
    restore(dir, &old)?;
    check_package(staged)?;
    fs::create_dir_all(&old)?;
    // ... names / sort / swap exactly as before
}
```

The line `let _ = fs::remove_dir_all(&old);` must be gone. Update `apply`'s doc comment to say leftovers are put back, never deleted.

**Verify**: `grep -n "remove_dir_all(&old)" src/update.rs` → no output. `cargo test --locked update::` → existing 5 tests pass.

### Step 3: `swap` reports a failed undo/rollback as `NotRestored`

Make the rename injectable so tests can make specific renames fail:

```rust
fn swap(dir: &Path, staged: &Path, old: &Path, names: Vec<OsString>) -> Result<()> {
    swap_with(dir, staged, old, names, &|from, to| fs::rename(from, to))
}

/// `swap` with its renames supplied, so tests can make chosen ones fail.
fn swap_with(
    dir: &Path,
    staged: &Path,
    old: &Path,
    names: Vec<OsString>,
    rename: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<()> {
```

Inside `swap_with`, keep the exact same rename sequence, but every call goes through `rename(...)` and a `restored` flag (initially `true` per failure) becomes `false` when the inline undo (`old/name -> current`) or either rollback rename (`dir/name -> staged/name`, `old/name -> dir/name`) returns `Err`. Then:
- if `restored` → return the existing error with context `"Could not replace {name}; the app was left unchanged"` (unchanged text);
- else → `return Err(NotRestored(format!("Could not replace {name} ({e}), and some replaced files could not be put back; they are in {}", old.display())).into());`

Hint for the borrow checker: replace the `step` closure with a small block or pass `&mut restored` into it; do not reorder renames.

**Verify**: `cargo clippy --workspace --locked -- -D warnings` → exit 0; `cargo test --locked update::` → existing tests pass.

### Step 4: The retry loop reports `NotRestored` honestly

In `src/ui/updater.rs` (`apply_update`, the spawned thread), remember whether any attempt returned `NotRestored`, and if the final result is still an error, report that instead of the last attempt's text:

```rust
            let mut result = update::apply(&dir, &staged);
            let mut not_restored = None;
            for _ in 0..5 {
                if let Some(e) = result.as_ref().err().and_then(|e| e.downcast_ref::<update::NotRestored>()) {
                    not_restored = Some(e.0.clone());
                }
                if result.is_ok() { break; }
                std::thread::sleep(Duration::from_secs(1));
                result = update::apply(&dir, &staged);
            }
            if let Some(e) = result.as_ref().err().and_then(|e| e.downcast_ref::<update::NotRestored>()) {
                not_restored = Some(e.0.clone());
            }
            if let (Err(_), Some(text)) = (&result, not_restored) {
                result = Err(anyhow::anyhow!("The update failed, and the app folder may not be as it was. {text}"));
            }
```

(Keep `record_installed_version` and the `tx.send` lines unchanged.) Retrying after `NotRestored` is intended: the next `apply` call's `restore` tries to put the leftovers back.

**Verify**: `cargo clippy --workspace --locked -- -D warnings` → exit 0.

### Step 5: Unit tests for the fixed paths

Add to `mod tests` in `src/update.rs` (use unique temp dir names with `std::process::id()` like the existing tests, and clean up at the end):

1. `a_failed_rollback_is_not_reported_as_unchanged_and_the_retry_restores` — build `app` (via `package(&app, "old")`) and `staged` (`package(&staged, "new")`), `old = app.join(STAGING).join("old")`, `fs::create_dir_all(&old)`. Call `swap_with(&app, &staged, &old, names, &rename)` with `names = ["YTDugem.exe", "tools"]` and a closure that returns `Err(std::io::Error::other("held"))` when `from == staged.join("YTDugem.exe")` **or** `from == old.join("YTDugem.exe")`, and calls `fs::rename` otherwise. Assert: the error `downcast_ref::<NotRestored>()` is `Some`; `app/YTDugem.exe` does not exist; `old/YTDugem.exe` reads `"old"`. Then call the real `apply(&app, &staged)` → `Ok`; assert `app/YTDugem.exe` reads `"new"` and `old/YTDugem.exe` reads `"old"`.
2. `apply_refuses_and_deletes_nothing_when_a_leftover_cannot_go_back` — `app` with `package(&app, "current")`, `old = app.join(STAGING).join("old")` holding a file `YTDugem.exe` with `"original"`, `staged` with `package(&staged, "new")`. `apply(&app, &staged)` → `Err` that downcasts to `NotRestored`; `old/YTDugem.exe` still reads `"original"`; `app/YTDugem.exe` still reads `"current"`; `staged/YTDugem.exe` still reads `"new"`.
3. Keep `a_failed_swap_restores_the_app_folder` passing unchanged; additionally assert there that its error does **not** downcast to `NotRestored` and its text contains `"left unchanged"`.

**Verify**: `cargo test --locked update::` → 7 tests pass (5 old + 2 new).

### Step 6: Update the TLA+ model to the fixed code

Replace `verification/update-swap/UpdateSwap.tla` with the text in the Appendix (already checked with TLC v1.7.4 against these configs). Then edit the `INVARIANTS` lines:

- `real-rollback-ok.cfg`, `broken-no-inline-undo.cfg`, `broken-no-rollback-old.cfg`: append ` HonestFailure` → `INVARIANTS TypeOK FailureLeavesAppUnchanged SuccessInstallsEveryItem NoOriginalLostBeforeSuccess HonestFailure`
- `real-rollback-fails.cfg`: `INVARIANTS TypeOK HonestFailure SuccessInstallsEveryItem NoOriginalLostBeforeSuccess` (the strict `FailureLeavesAppUnchanged` cannot hold when a rename fails forever; the honest-report property replaces it).

In `scripts/verify-update-swap.ps1`, set `$expected` to:

```powershell
$expected = [ordered]@{
    'real-rollback-ok'       = 'pass'                      # the code as written
    'real-rollback-fails'    = 'pass'                      # rollback renames may fail: nothing lost, report honest
    'broken-no-inline-undo'  = 'FailureLeavesAppUnchanged' # proves the check can fail
    'broken-no-rollback-old' = 'FailureLeavesAppUnchanged' # proves the check can fail
}
```

**Verify**: `powershell -NoProfile -File scripts/verify-update-swap.ps1` → four `OK` lines (`real-rollback-ok pass ~190 states`, `real-rollback-fails pass ~1449 states`, both `broken-*` `FailureLeavesAppUnchanged`), then exit **3** (DRIFT) — expected until Step 7.

### Step 7: Update the skill doc and the sync commit

Commit Steps 1–6 first. Then edit `.claude/skills/verify-update-swap/SKILL.md`:
- "The script expects these results" table: match the new `$expected` values and say `real-rollback-fails` now passes.
- Invariants list: add `HonestFailure`: when `apply_update` gives up and no attempt reported `NotRestored`, the folder is as it was.
- Coverage record: change the `Apply` row to `apply`: `restore(dir, &old)?` (new `Restore` action: each `.update/old` entry renamed back, bail with `NotRestored` if the app folder has the name or the rename fails; never deletes), then `check_package(staged)?`, `read_dir` + sort. Add that `UndoInline`/`RbStaged`/`RbOld` failures set `incomplete` (= `swap_with` returns `NotRestored`), and the `Err` row: `apply_update` reports `NotRestored` if any attempt returned it.
- Replace "## Known finding (open)" with a short "## Fixed finding" paragraph: what it was (one sentence each for the 4 steps), and that since this commit `apply` never deletes `.update/old` and failed undo/rollback renames are reported as `NotRestored`.
- Set `Sync commit:` to the full SHA of your Steps 1–6 commit (`git rev-parse HEAD` right after that commit).

Commit this as a second commit (e.g. `docs(update): record the fixed rollback finding in the swap model`).

**Verify**: `powershell -NoProfile -File scripts/verify-update-swap.ps1` → four `OK` lines and `Covered code matches sync commit ...`, exit 0.

## Test plan

- New: the two tests in Step 5 (rollback-failure reporting + retry restoring; refusal without deletion).
- Strengthened: `a_failed_swap_restores_the_app_folder` asserts the plain "left unchanged" error.
- Model: all four configs as in Step 6.
- Full: `cargo test --workspace --locked` → all pass.

## Done criteria

- [ ] `cargo fmt --all --check` exits 0
- [ ] `cargo clippy --workspace --locked -- -D warnings` exits 0
- [ ] `cargo test --workspace --locked` exits 0; `cargo test --locked update::` shows 7 passing tests
- [ ] `grep -n "remove_dir_all(&old)" src/update.rs` prints nothing
- [ ] `grep -c "let _ = fs::rename" src/update.rs` prints `0`
- [ ] `powershell -NoProfile -File scripts/verify-update-swap.ps1` exits 0
- [ ] `git status` shows no modified files outside the in-scope list
- [ ] `plans/README.md` row 001 updated

## STOP conditions

Stop and report back (do not improvise) if:
- The excerpts in "Current state" don't match the live code.
- Java is not available, so Step 6/7 can't be verified — finish Steps 1–5, commit, and report that the model step is pending (do not edit the model unverified).
- TLC reports a result other than the expected ones after Step 6 — copy the first 40 lines of `target/tla/out/<model>.txt` into your report. Do not weaken invariants or remove failure choices to make it pass.
- Making `swap_with` compile seems to require changing the order or arguments of the forward renames.
- Any existing `update::` test needs its assertions changed (other than the additions in Step 5).

## Maintenance notes

- Any future change to `apply`, `swap_with`, `restore`, `check_package`, `STAGING` or the retry loop must re-run `scripts/verify-update-swap.ps1` and follow the skill's "When the code changes" section.
- Known, accepted behaviour: if a previous successful update's `.update/old` could not be removed by `cleanup()` at start, the next update stops with `NotRestored` text until a restart's `cleanup()` succeeds. This is safe (nothing deleted) but could confuse; revisit if users report it.
- Still not modeled: power loss mid-swap, and `cleanup()` deleting `.update` at the next start.
- Reviewer focus: that no `let _ =` remains on a rename, and that the success path is byte-for-byte the same sequence of renames.

## Appendix: new `verification/update-swap/UpdateSwap.tla`

```tla
---------------------------- MODULE UpdateSwap ----------------------------
(***************************************************************************)
(* The in-app updater's folder swap, as the code runs it:                  *)
(*   src/ui/updater.rs  apply_update  - calls update::apply, then retries  *)
(*                                      up to 5 more times, 1 s apart.     *)
(*                                      If any attempt failed with         *)
(*                                      NotRestored, the final error says  *)
(*                                      so.                                *)
(*   src/update.rs      apply         - restore: puts back what an earlier *)
(*                                      attempt left in .update/old, never *)
(*                                      deleting it; check_package; lists  *)
(*                                      .update/new/YTDugem, sorts.        *)
(*   src/update.rs      swap          - per name: dir->old, staged->dir;   *)
(*                                      on error, inline undo, then roll   *)
(*                                      back `moved` in reverse. A failed  *)
(*                                      undo/rollback rename makes the     *)
(*                                      error NotRestored.                 *)
(* Every fs::rename is one atomic step. Any rename may fail (a tool still  *)
(* running from `tools`, an antivirus scan). RollbackCanFail says whether  *)
(* the undo and rollback renames may fail too.                             *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS
    Order,           \* package top-level names, in `names.sort()` order
    Dirs,            \* names that are folders (Windows rename never replaces one)
    Existing,        \* names the app folder holds before the update
    Required,        \* what check_package demands in the staged package
    MaxAttempts,     \* apply_update: 1 call + 5 retries
    RollbackCanFail, \* may the undo and rollback renames fail?
    Variant          \* "real", or a deliberately broken copy of swap (see below)

Names  == {Order[k] : k \in 1..Len(Order)}
Places == {"dir", "staged", "bak"}   \* app folder, .update/new/YTDugem, .update/old
Vers   == {"old", "new", "none"}

\* incomplete: some attempt reported NotRestored (sticky for the whole apply_update).
VARIABLES fs, lost, pc, attempts, names, i, had, moved, j, incomplete
vars == <<fs, lost, pc, attempts, names, i, had, moved, j, incomplete>>

Init ==
    /\ fs = [p \in Places |-> [n \in Names |->
              IF p = "dir"    THEN (IF n \in Existing THEN "old" ELSE "none")
              ELSE IF p = "staged" THEN "new" ELSE "none"]]
    /\ lost = {}
    /\ pc = "apply"
    /\ attempts = 0
    /\ names = <<>>
    /\ i = 1
    /\ had = FALSE
    /\ moved = <<>>
    /\ j = 0
    /\ incomplete = FALSE

(* fs::rename(src/n, dst/n). Missing source fails. An existing folder at   *)
(* dst fails; an existing file at dst is replaced (MoveFileEx with         *)
(* REPLACE_EXISTING), and the replaced version is destroyed.               *)
CanMove(src, dst, n) ==
    fs[src][n] # "none" /\ (fs[dst][n] = "none" \/ n \notin Dirs)

Move(src, dst, n) ==
    /\ lost' = IF fs[dst][n] # "none" THEN lost \cup {<<n, fs[dst][n]>>} ELSE lost
    /\ fs' = [fs EXCEPT ![dst][n] = fs[src][n], ![src][n] = "none"]

\* A rename that the code checks: it succeeds, or it fails and nothing moves.
TryMove(src, dst, n, onOk, onErr) ==
    \/ /\ CanMove(src, dst, n) /\ Move(src, dst, n) /\ onOk
    \/ /\ UNCHANGED <<fs, lost>> /\ onErr

\* An undo or rollback rename. When it fails, swap's error becomes NotRestored.
RollbackMove(src, dst, n) ==
    \/ CanMove(src, dst, n) /\ Move(src, dst, n) /\ UNCHANGED incomplete
    \/ (RollbackCanFail \/ ~CanMove(src, dst, n)) /\ UNCHANGED <<fs, lost>>
       /\ incomplete' = TRUE

StartRollback == /\ pc' = "rbLoop" /\ j' = Len(moved)

\* apply: one call from apply_update's loop.
Apply ==
    /\ pc = "apply"
    /\ attempts' = attempts + 1
    /\ pc' = "restore"
    /\ UNCHANGED <<fs, lost, names, i, had, moved, j, incomplete>>

(* restore(dir, old): for each entry of .update/old, in any order: if the  *)
(* app folder already has that name, or the rename back fails, return      *)
(* NotRestored and delete nothing. When old is empty: check_package(staged)?*)
(* and names = read_dir(staged) sorted.                                    *)
Restore ==
    /\ pc = "restore"
    /\ IF \E n \in Names : fs["bak"][n] # "none"
         THEN /\ \E n \in {m \in Names : fs["bak"][m] # "none"} :
                   IF fs["dir"][n] # "none"
                     THEN /\ pc' = "err" /\ incomplete' = TRUE
                          /\ UNCHANGED <<fs, lost>>
                     ELSE TryMove("bak", "dir", n,
                              pc' = "restore" /\ UNCHANGED incomplete,
                              pc' = "err" /\ incomplete' = TRUE)
              /\ UNCHANGED names
         ELSE /\ UNCHANGED <<fs, lost, incomplete>>
              /\ IF \A n \in Required : fs["staged"][n] # "none"
                   THEN /\ names' = SelectSeq(Order, LAMBDA n : fs["staged"][n] # "none")
                        /\ pc' = "loop"
                   ELSE /\ names' = names
                        /\ pc' = "err"
    /\ i' = 1 /\ moved' = <<>> /\ j' = 0
    /\ UNCHANGED <<attempts, had>>

Loop ==
    /\ pc = "loop"
    /\ IF i > Len(names)
         THEN pc' = "ok" /\ UNCHANGED had
         ELSE /\ had' = (fs["dir"][names[i]] # "none")     \* symlink_metadata().is_ok()
              /\ pc' = IF had' THEN "fwdOut" ELSE "fwdIn"
    /\ UNCHANGED <<fs, lost, attempts, names, i, moved, j, incomplete>>

\* if had { rename(&current, old.join(&name))?; }
FwdOut ==
    /\ pc = "fwdOut"
    /\ TryMove("dir", "bak", names[i], pc' = "fwdIn" /\ UNCHANGED j, StartRollback)
    /\ UNCHANGED <<attempts, names, i, had, moved, incomplete>>

\* rename(staged.join(&name), &current), undoing the first rename inline on error.
FwdIn ==
    /\ pc = "fwdIn"
    /\ TryMove("staged", "dir", names[i],
           /\ moved' = Append(moved, <<names[i], had>>)
           /\ i' = i + 1
           /\ pc' = "loop"
           /\ UNCHANGED j,
           /\ UNCHANGED <<moved, i>>
           /\ IF had /\ Variant # "noInlineUndo"
                THEN pc' = "undoInline" /\ UNCHANGED j
                ELSE StartRollback)
    /\ UNCHANGED <<attempts, names, had, incomplete>>

UndoInline ==
    /\ pc = "undoInline"
    /\ RollbackMove("bak", "dir", names[i])
    /\ StartRollback
    /\ UNCHANGED <<attempts, names, i, had, moved>>

\* for (name, had) in moved.iter().rev()
RbLoop ==
    /\ pc = "rbLoop"
    /\ pc' = IF j = 0 THEN "err" ELSE "rbStaged"
    /\ UNCHANGED <<fs, lost, attempts, names, i, had, moved, j, incomplete>>

RbStaged ==
    /\ pc = "rbStaged"
    /\ RollbackMove("dir", "staged", moved[j][1])
    /\ IF moved[j][2] /\ Variant # "noRollbackOld"
         THEN pc' = "rbOld" /\ UNCHANGED j
         ELSE pc' = "rbLoop" /\ j' = j - 1
    /\ UNCHANGED <<attempts, names, i, had, moved>>

RbOld ==
    /\ pc = "rbOld"
    /\ RollbackMove("bak", "dir", moved[j][1])
    /\ pc' = "rbLoop" /\ j' = j - 1
    /\ UNCHANGED <<attempts, names, i, had, moved>>

\* apply_update: retry while attempts remain, else report the error.
Err ==
    /\ pc = "err"
    /\ pc' = IF attempts < MaxAttempts THEN "apply" ELSE "failed"
    /\ UNCHANGED <<fs, lost, attempts, names, i, had, moved, j, incomplete>>

Done == pc \in {"ok", "failed"} /\ UNCHANGED vars

Next == Apply \/ Restore \/ Loop \/ FwdOut \/ FwdIn \/ UndoInline \/ RbLoop
        \/ RbStaged \/ RbOld \/ Err \/ Done

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
(* Properties.                                                             *)

\* When the update gives up, the app folder holds exactly what it held before.
\* Holds only while undo/rollback renames cannot fail (real-rollback-ok).
FailureLeavesAppUnchanged ==
    pc = "failed" =>
        \A n \in Names : fs["dir"][n] = (IF n \in Existing THEN "old" ELSE "none")

\* When the update gives up without reporting NotRestored, its error's claim
\* "the app was left unchanged" is true.
HonestFailure ==
    pc = "failed" /\ ~incomplete =>
        \A n \in Names : fs["dir"][n] = (IF n \in Existing THEN "old" ELSE "none")

\* When the update succeeds, every package item in the app folder is the new one.
SuccessInstallsEveryItem ==
    pc = "ok" => \A n \in Names : fs["dir"][n] = "new"

\* No original item of the app folder is ever destroyed before the swap succeeds.
NoOriginalLostBeforeSuccess ==
    pc # "ok" => \A n \in Existing : <<n, "old">> \notin lost

TypeOK ==
    /\ fs \in [Places -> [Names -> Vers]]
    /\ attempts \in 0..MaxAttempts
    /\ incomplete \in BOOLEAN
=============================================================================
```

Advisor's verification of this model (TLC v1.7.4, workers auto): `real-rollback-ok` pass (190 distinct states); `real-rollback-fails` pass (1449); `broken-no-inline-undo` and `broken-no-rollback-old` violate `FailureLeavesAppUnchanged`. A mutation that stops failed rollback renames from setting `incomplete` makes `real-rollback-fails` violate `HonestFailure`, so the honesty check has teeth.
