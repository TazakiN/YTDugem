---
name: verify-update-swap
description: Model-check the in-app updater's folder swap (src/update.rs apply/swap and the retry loop in src/ui/updater.rs apply_update) with TLA+/TLC. Use when a change touches those functions, check_package, the .update staging layout, or the package's top-level items, or when asked whether a failed update can damage or delete the installed app.
---

# Verify the updater's folder swap

## The promise

When an in-app update cannot replace the app folder, the folder is left as it was, and the
user's copy of YTDugem is never deleted before the new one is fully in place.

**The harm if it breaks:** a failed update leaves the user with an app folder that has no
`YTDugem.exe` or mixes old and new files. The user then has to download and reinstall the app by hand.

## Run it

```powershell
powershell -NoProfile -File scripts/verify-update-swap.ps1
```

The script needs Java 11 or newer. It downloads `tla2tools.jar` v1.7.4 into `target/tla/` on first use and pins its
SHA-256. A run takes a few seconds. It runs every model in `verification/update-swap/` and checks
each against its expected outcome. It then compares the covered code with the sync commit below.

| Exit | Meaning |
| --- | --- |
| 0 | Every model matched its expectation, and the covered code is unchanged since the sync commit. |
| 1 | A model did not match its expectation. Read `target/tla/out/<model>.txt`. |
| 2 | Setup failed: Java is missing, the jar hash is wrong, or the sync line is missing. |
| 3 | The models matched, but `src/update.rs` or `src/ui/updater.rs` changed since the sync commit. The result is not trusted until you do the steps in *When the code changes*. |

The script expects these results:

| Model (`.cfg`) | What it is | Expected |
| --- | --- | --- |
| `real-rollback-ok` | The code as written. Any forward rename may fail. The rollback renames succeed. | pass (190 states) |
| `real-rollback-fails` | The code as written, and an undo or rollback rename may fail too. Checks `HonestFailure` in place of `FailureLeavesAppUnchanged`, which cannot hold when a rename fails forever. | pass (1449 states): nothing is lost and the report is honest. It failed before the fix below. |
| `broken-no-inline-undo` | `swap_with` without the inline `rename(&old.join(&name), &current)` undo | `FailureLeavesAppUnchanged` violated: shows the check can fail |
| `broken-no-rollback-old` | `swap_with` rollback without `rename(&old.join(name), &dir.join(name))` | `FailureLeavesAppUnchanged` violated: shows the check can fail |

Read every TLC result with the pstack `/statechart` skill. It tells a violation as its shortest
path in plain steps. On a pass, it draws how the swap moves between states. Retell a
counterexample in the code's words, such as "`YTDugem.exe` moved to `.update/old`; then...", not as raw TLC
states.

## What is checked

The model is `verification/update-swap/UpdateSwap.tla`. `MC.tla` gives it the package's
top-level items in `names.sort()` order: `NOTICE` (a file the new package adds),
`YTDugem.exe`, and the `tools` folder. Each `fs::rename` is one atomic step that may fail
with nothing moved. Renaming onto an existing folder fails. Renaming onto an existing file
replaces it, and the replaced version is destroyed, as `MoveFileEx` with
`MOVEFILE_REPLACE_EXISTING` does.

Invariants (in `UpdateSwap.tla`):

- `FailureLeavesAppUnchanged`: when `apply_update` gives up, every item in the app folder is the
  version it was before. That is what the error text "the app was left unchanged" claims.
- `HonestFailure`: when `apply_update` gives up and no attempt reported `NotRestored`, every
  item in the app folder is the version it was before. The error claims "unchanged" only when
  that is true.
- `SuccessInstallsEveryItem`: when `apply` returns `Ok`, every package item in the app folder
  is the new version.
- `NoOriginalLostBeforeSuccess`: no original item is destroyed before the swap succeeds.
- `TypeOK`.

## Coverage record

| Model action | Code |
| --- | --- |
| `Apply` | `src/update.rs` `apply`: one call from the retry loop |
| `Restore` | `apply`: `restore(dir, &old)?` renames each `.update/old` entry back, and returns `NotRestored` if the app folder already has that name or the rename fails. It never deletes. Then `check_package(staged)?`, `read_dir(staged)` + `names.sort()` |
| `Loop` | `swap_with`: `for name in names`, `had = current.symlink_metadata().is_ok()` |
| `FwdOut` | `swap_with` `step`: `if had { rename(&current, &old.join(&name))?; }` |
| `FwdIn`, `UndoInline` | `swap_with` `step`: `rename(&staged.join(&name), &current)`, and on error `rename(&old.join(&name), &current)`; its failure sets `incomplete` (`restored = false`) |
| `RbLoop`, `RbStaged`, `RbOld` | `swap_with`: `for (name, had) in moved.iter().rev()`, both renames; a failure sets `incomplete`, and `swap_with` returns `NotRestored` |
| `Err`, `MaxAttempts = 6` | `src/ui/updater.rs` `apply_update`: one `update::apply` plus `for _ in 0..5` retries; if any attempt returned `NotRestored`, the final error reports it |
| `Required` | `src/update.rs` `check_package`: `YTDugem.exe`, `tools/mpv.exe`, `tools/yt-dlp.exe` (modeled as the `YTDugem.exe` and `tools` items) |
| `Dirs`, `Existing`, `Order` | Top level of the portable ZIP (`scripts/package-windows.py`), and an installed folder that holds the exe and `tools` |

Sync commit: `bcb06d15d4de53726ce04ef2cffc01d9a3d3525c`

The script reads the line above. Move it forward only after you confirm that the model still describes
`src/update.rs` and `src/ui/updater.rs` at the new commit.

## Scope and assumptions

- Covered: every order of success and failure for each rename, across the first attempt and all
  five retries, for a package with one new file, the exe, and the `tools` folder. Two items
  that are already installed and one new item cover every kind of step `swap` takes (`had` true, `had` false,
  folder, file). More names add no new kind of step.
- Assumed: `create_dir_all` and `read_dir` succeed. No other process renames items in the app folder
  during the swap. Other processes only make renames fail.
- Not covered: a crash or power loss in the middle of `swap`; `download`, `extract`, and checksum
  checks; `cleanup()` at the next start, which deletes `.update` and the backup in it; `restart`;
  the contents of the `tools` folder. A pass says nothing about these.

## Fixed finding

Before commit `bcb06d1`, `real-rollback-fails` violated `NoOriginalLostBeforeSuccess`:

1. `swap` moved the installed `YTDugem.exe` to `.update/old/YTDugem.exe`.
2. Moving the new `.update/new/YTDugem/YTDugem.exe` into place failed, for example because a
   scanner held it.
3. The inline undo `let _ = fs::rename(old.join(&name), &current)` also failed, and the error
   was ignored. The app folder had no `YTDugem.exe`.
4. `apply_update` retried, and `apply` ran `let _ = fs::remove_dir_all(&old)`, deleting the only
   copy of the user's exe, while the error still said "the app was left unchanged".

Since that commit `apply` never deletes `.update/old`: `restore` puts its entries back or stops
with `NotRestored`. A failed undo or rollback rename makes `swap_with` return `NotRestored`,
and `apply_update` reports it even when a later retry fails differently.

## Real bug or modeling error?

Before you report a violation, open the code at each step of the counterexample. Find the line
that performs that step, as the coverage record maps it.

- **Real bug:** every step matches a line that can do that, in that order. Report it with the steps
  in the code's words. Do not weaken an invariant or remove a failure choice so that it passes.
- **Modeling error:** a step cannot happen in the code. For example, a rename that the code guards
  is allowed, two renames are merged that the code does separately, or the item list differs from the
  package. Fix the model to match the code. Then confirm that `broken-*` still fails.

## When the code changes

1. Run the script. Exit 3 lists the covered files that changed.
2. Read the diff from the sync commit in those files. For each change to
   `apply`, `swap`, `check_package`, `STAGING`, or the retry loop in `apply_update`,
   change the matching action in `UpdateSwap.tla` so the model does the same thing. Use the
   coverage record. Keep each action to one `fs` call.
3. If the package's top level or the required files change, update `MC.tla`.
4. If you change rollback steps, keep the `Variant` hooks. Each `broken-*` model must still
   remove one guard and still fail.
5. Rerun the script until every model matches its expectation. Report any new violation with the
   steps above, and do not change product code unless you were asked to. Then set the sync commit to the commit
   whose code you checked, and update the coverage record.
