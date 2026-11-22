# W2-A flake 6 — a row can be observed `Stopped` with its old process identity (two writes in the wrong order)

New branch from `main` AFTER the W2-A flake branch (`rezkycodes/w2-flaky-instance`) is merged (the reviewer merges it first; check `git log main --oneline | head` for the merge before you start). Rules `docs/orchestration/tasks/_COMMON-RULES.md` incl. 11-14. Same worktree as before is fine (`git checkout -b rezkycodes/w2-flaky-instance-2 main`).

## Evidence (reviewer, 2026-11-22/23, private copy of the lib test binary, `RUST_TEST_THREADS=16 taskset -c 0-3`, whole lib x25)

- branch head `fb1aff5` (your round 5): 1 failure in 25: `instance::tests::safety::reconciliation_adopts_a_live_process_and_clears_a_dead_one`, `safety.rs:315` "a dead identity is cleared on reconciliation". Default threads: 20/20.
- `main` (same condition, 25 runs): 12 runs failed (11 distinct instance tests, `a_delete_refuses_to_remove_a_row_whose_engine_survives` 5 times) and 1 run HUNG: the test binary sat at 2% CPU for 45 minutes until I killed it (`eu-stack` shows the test thread in `instance::tests::coverage::a_watched_log_streams_appended_lines`); 12 runs passed. Your fixes removed almost all of that; the one failure left is not a test artefact.

## Cause (read from the code, confirm it)

`reconcile.rs::mark` writes the new status first (`persist`) and clears the process identity afterwards (`clear_identity`), two separate store writes. A reader (the test polls `list_instances`, the GUI and the CLI read the same store) that lands between them sees `Stopped` with the old pid/start-time/exe still recorded. That is a product inconsistency, not only a test race: a row that says `Stopped` must never still carry the identity of a process we just declared gone (the next start, a stop, the supervisor and `identity_of` all read it).

## Do

1. Reproduce deterministically first: a test that pauses between the two writes (a hook like your `on_reconcile` ones, or a store wrapper) and asserts that no observer can see `Stopped` together with a recorded identity. Paste the red output on `main`.
2. Fix: clear the identity BEFORE the status write, or do both in one store transaction (prefer one transaction if a store method for it is small; otherwise reorder). Then audit every other place in `crates/core/src/instance/` that writes a terminal status (`Stopped`, `Failed`) and the identity separately (stop, the supervisor, failed start, `rollback_create`, `adopt`) and give each the same guarantee, or say why it already has it.
3. Mutation: put the old order back -> your new test is red; restored, clean `git status --short`.

## Item 4 (found with the hang above): two tests can block forever

`crates/core/src/instance/tests/coverage.rs:187` (`a_watched_log_streams_appended_lines`) and `crates/core/src/instance/tests/safety.rs:410` call `receiver.blocking_recv()` in a loop whose `deadline` is only checked AFTER an event arrives, so when no event ever arrives the test hangs forever instead of failing (your own `harness_assert.rs:88` comment describes exactly this trap for the harness helpers; these two were missed). In the coverage test there is also a real race: the follower can start tailing after the line was appended, so nothing is ever delivered. Fix both: a bounded wait (`try_recv` plus a short sleep until the deadline, then a failing assertion that says what was missing) and, for the log test, make the append happen only after the follower is known to be registered, or append in a loop until a line arrives or the deadline passes. Grep the whole workspace for `blocking_recv()` and `recv()` without a timeout in tests and bound every one. Red-first for the log test: show the hang turned into a failure (run it with the follower disabled or the append moved before the watch).

## Bar

fmt, clippy workspace, deny, the new test x100, `cargo test -p dblocal-core --lib` x25 at default threads and x25 with `RUST_TEST_THREADS=16 taskset -c 0-3` run from a PRIVATE COPY of the test binary (copy it right after `cargo test --no-run`, print the distinct passed counts), `cargo xtask ci` under the heavy lock on the final head (`CARGO_BUILD_JOBS=8`, MemAvailable above 8000 MB: other workers link GUI test binaries at the same time, check `free -m` first). `worker_done` only with the red and green output pasted.
