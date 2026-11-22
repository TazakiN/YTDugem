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
