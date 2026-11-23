# W2-A flake 8 fix 1 — one untested behaviour, one stale comment, one new whole-lib flake

Same branch `rezkycodes/w2-flaky-instance-4`, same worktree. Rules `docs/orchestration/tasks/_COMMON-RULES.md` incl. 11-15 (rule 15: work only in your own worktree, stage explicit paths). FIRST `git merge main` (main moved: W2-G, W4-A2, W4-C and docs; the reviewer's trial merge of your head onto main is conflict-free). Do not touch `dblocal-gpui-kit-source/`.

## What the reviewer verified and accepts (do not redo)

On the trial merge (main fde8fa7 + your 9c391ca): fmt, clippy workspace (also `--features demo`), deny, core lib x3 (1453 passed), your seven changed or added tests x40 from a private copy (40/40 each, also x40 pinned with `taskset -c 0-3` plus four burners and `RUST_TEST_THREADS=16`), whole lib pinned x12 (12/12). Mutations red: marker destroyed together with the data (`a_live_delete_keeps_its_row_while_another_service_reconciles`: `left: None right: Some(Deleting)`), tick counter never advancing (harness panics), `is_busy` guard removed (`the supervisor must not act on a row an operation holds`). The product fix (marker beside the instance directories, keyed by the instance id, a UUID) is accepted.

## F1 (MEDIUM) — the reconciliation-side marker cleanup is untested (mutation green)

Replacing both `super::delete::clear_in_flight_marker(&inner.paths, &id);` calls in `instance/reconcile.rs::finish_delete` with nothing keeps all 1453 lib tests green (the compiler only warns that the function is never used). That cleanup is new behaviour you added: a crashed delete's dotfile must not stay in the home forever. Do, red first:
1. A test where a delete crashed with data still present (branch (a): row `Deleting`, marker naming a process that is gone, data dir present): after the restarted service's reconciliation settles, the row is back to `Stopped` AND `marker_path` no longer exists.
2. The same for branch (c) (data gone, no archive: reconciliation removes the row): row gone AND the marker gone.
3. Paste the red output on your head with the two calls removed (each branch separately: remove one call, only its test goes red), then green, restored, clean `git status --short`.
Also assert the negative: a marker that names a LIVE owner (the existing "another process owns the delete" test) is still there after the restart (reconciliation must not clear a live delete's marker).

## F2 (LOW) — stale doc comment

`crates/core/src/instance/retry.rs:15` still says the sibling recreates `delete.inflight` inside `instances/<name>/`. The marker is no longer there. Say what the race that `remove_tree_retrying` guards really is today (a sibling writing something else into the directory, or the old reason is gone), and if the old reason is gone, say so in the comment; do not remove the retry.

## F3 (MEDIUM, time-box about 90 minutes) — `process::evidence::a_grandchild_survives_its_parent_exiting` failed once in 18 pinned whole-lib runs

Reviewer evidence: whole lib, `RUST_TEST_THREADS=16 taskset -c 0-3` with four `yes` burners, run 3 of 6 failed (the next 12 runs were green): `thread 'process::evidence::a_grandchild_survives_its_parent_exiting' panicked at crates/core/src/process/evidence.rs:119:5: adopted engine must still be stoppable`. The same test alone x100 pinned is 100/100 and `process::` x30 pinned is 30/30, so it needs the pressure of the whole lib. Your branch does not touch this file; this is the next flake in the hunt. Do: reproduce with the whole lib pinned (loop it, keep the failing output), find out from `terminate`'s outcome whether `signalled()` is false because the process was already gone, because the identity no longer matched, or because the 3 s budget ran out, and report which. Fix the product or the test accordingly (if it is a product defect in `terminate`, say so plainly and fix it with a deterministic test), or, if you cannot reproduce in the time box, say that and add the diagnostics (the `ProcessOutcome` in the assertion message) so the next occurrence names its cause. Whole lib pinned x25 afterwards.

## Bar

fmt, clippy workspace (also `--features demo`), deny, the new tests x100 from a private copy (check with `--list` that they are inside the copy), core lib x3 and pinned whole lib x25, `cargo xtask ci` under the heavy lock on the final head (`CARGO_BUILD_JOBS=8`, MemAvailable above 8000 MB). `worker_done` only with the red and green outputs pasted.
