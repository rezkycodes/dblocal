# W2-A flake 6 — a row can be observed `Stopped` with its old process identity (two writes in the wrong order)

New branch from `main` AFTER the W2-A flake branch (`rezkycodes/w2-flaky-instance`) is merged (the reviewer merges it first; check `git log main --oneline | head` for the merge before you start). Rules `docs/orchestration/tasks/_COMMON-RULES.md` incl. 11-14. Same worktree as before is fine (`git checkout -b rezkycodes/w2-flaky-instance-2 main`).

## Item 0 (URGENT, do this FIRST, its own commit, report it before the rest): `dblocal stop` is refused as busy by the process' own startup reconciliation

Found by the reviewer with the REAL binary on merged main (`e604687`), not by any test. Setup: a valkey instance created with `dblocal create`, then in a loop `dblocal start x; dblocal stop x` (two separate CLI processes), isolated `DBLOCAL_HOME`.

- baseline binary (`700da45`, before your fixes): `stop` 40 of 40 ok, 0 refusals.
- merged main binary (`e604687`): `stop` refused with `[DBL-1004] Tindakan itu tidak bisa dilakukan pada status instance saat ini.` 9 of 20, then 15 of 20 (it gets worse under load). Same machine, same instance, same sequence.

Cause (read it, confirm it): `LocalService::new` spawns `reconcile_on_startup` (auto_start). Since round 5 the third phase holds a `lock_if_idle` guard for every row that holds a process (Starting/Running/Stopping) across the probe AND the persist. A one-shot CLI process issues its command within milliseconds of construction, so `stop_instance` calls `lock_instance`, finds the guard held by reconciliation and returns `InvalidTransition { from: "busy" }`. The refusal I asked for in round 5 ("an operation that arrives meanwhile is refused as busy, which is the right answer during reconciliation") is right for a real in-flight USER operation and wrong for reconciliation: the CLI user did nothing wrong and the instance is not busy in any way they can see. The GUI has the same race (a tray click right after launch). `start` of a stopped row is not affected (phase 3 skips it), `stop`, `restart`, `delete`, `change_port`, `set_autostart` on a running row are.

Required behaviour: an operation that arrives while STARTUP RECONCILIATION holds that instance's guard waits for reconciliation to finish with that instance (bounded: the reconciliation probe is already bounded, so a few seconds at most; then the old refusal), instead of failing. A guard held by a real in-flight operation still refuses as busy exactly as before, and reconciliation still must not touch a row an in-flight operation owns (your round 5 tests stay green). Pick the design (for example: `Ops::lock_instance` distinguishes a reconciliation guard and waits on it; or the service entry points await a `reconciled` signal with a timeout; the CLI-only variant "construct with auto_start false and run reconciliation to completion before the command" fixes only the CLI, so it is not enough on its own because the GUI shares the race) and say why.

1. Red first: a deterministic test with your `on_reconcile` hook pausing reconciliation while it holds the guard of a live-identity Running row; call `stop_instance` from another thread; release the hook; the stop must succeed and the row end `Stopped`. Paste the red output on `main`.
2. Fix. Keep the R1 tests green (reconciliation vs a real in-flight stop/start).
3. Mutation: remove the wait -> the new test red; restored, clean `git status --short`.
4. Real binary, in the same worktree, with `cargo build -p dblocal-cli --bin dblocal` into the GUI target and an isolated `DBLOCAL_HOME` (copy the valkey tarball from `~/.cache/dblocal-dev/` into `$DBLOCAL_HOME/downloads/`): 40 iterations of `dblocal start x; dblocal stop x`, 0 refusals, and again with `taskset -c 0-1` plus 4 `yes` burners (load makes it worse). Paste the counts. Clean up every engine process afterwards (`pgrep -x valkey-server`).
5. `cargo xtask ci` does NOT run `cli_e2e`/`service_e2e`; run them under the e2e lock on your final head (`DBLOCAL_E2E=1 ... --ignored`) and paste the result lines. I did not run them on main after round 5, so tell me if anything else fails.

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

## Item 5: two product changes of your round 4 have no test (reviewer mutations stayed green)

On `fb1aff5` with the whole `instance::` module (103 tests): (a) swapping `finish_operation` back to "record in the registry, then broadcast `OperationFinished`" is not caught by any test; (b) putting the delete marker nonce back to `format!("{}-{}", std::process::id(), id.as_str())` (per process and instance, instead of the per-operation id) is not caught by any test either. Add one deterministic test each (for (a): a subscriber that stops as soon as the registry says finished must already find `OperationFinished` in its channel; for (b): the loser's marker `Drop` must not remove the winner's marker, two deletes of the same id in one process must not share a nonce) and show each red against its mutation.

## Bar

fmt, clippy workspace, deny, the new test x100, `cargo test -p dblocal-core --lib` x25 at default threads and x25 with `RUST_TEST_THREADS=16 taskset -c 0-3` run from a PRIVATE COPY of the test binary (copy it right after `cargo test --no-run`, print the distinct passed counts), `cargo xtask ci` under the heavy lock on the final head (`CARGO_BUILD_JOBS=8`, MemAvailable above 8000 MB: other workers link GUI test binaries at the same time, check `free -m` first). `worker_done` only with the red and green output pasted.
