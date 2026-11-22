# W2-A flake 5 — one regression in startup reconciliation (the third phase lost its busy guard)

Same branch `rezkycodes/w2-flaky-instance`, same worktree, rules `docs/orchestration/tasks/_COMMON-RULES.md` incl. 11-14. Head under review: `be75903`.

## What the reviewer verified (do not redo)

The create-vanishes fix is right in principle: the per-instance lock plus a fresh read before the Deleting and create-cleanup decisions closes the race you traced; `remove_tree_retrying` is bounded and only retries `ENOTEMPTY`/`EEXIST`; the broadcast-before-registry order in `finish_operation` is consistent with `operation_result`. Your own bar (fmt, clippy, deny, `xtask ci`, 0/60, coverage) is accepted as reported; the reviewer re-runs it on the merged result.

## R1 (HIGH, blocks the merge) — `pending` phase acts on a stale snapshot with no in-flight guard

On main, `reconcile_on_startup` filtered the whole snapshot with `!inner.ops.is_busy(instance.id())`, so none of its three phases touched an instance that has an operation running in this process. Your change replaced that filter with `lock_if_idle` + `fresh_instance` for the Deleting and the create-cleanup phases, and removed the filter. The third phase (`pending`: rows where `status().holds_process()`, i.e. Starting, Running, Stopping) now has NO guard and no fresh read: `adopt` and `mark` act on the startup snapshot row, and `persist` is an unconditional `update_status`.

Scenario (the CLI: `LocalService::new` spawns reconciliation, then `dblocal stop x` runs at once): the stop claims `Stopping` synchronously; reconciliation lists the rows after the claim, sees `Stopping` with a live recorded identity, probes it alive, and `adopt` writes `Running`. If the stop finishes first (row `Stopped`, identity cleared) the late `adopt` leaves a `Running` row with a dead process; if the stop is slower, `Running` overwrites the `Stopping` claim and a second stop is accepted while the first is in flight. The start case: a `Starting` row that already has an identity (the spawn recorded it) is probed; if the probe says alive, `adopt` writes `Running` while the start is still waiting for readiness, and if it says gone, `mark(ReconciledAbsent)` writes `Stopped` and clears the identity under a start that is still in flight. Nothing in `crates/core/src/instance/tests/` pins this (I grepped: `reconciliation_adopts_a_live_stopping_row_as_running` covers only the crash case with no operation in flight).

Do, in this order:

1. Write the failing test FIRST with the hooks you already built (`on_reconcile` before/after list, a `Harness` with a live fake identity): after the rows are listed, an in-process stop (or start) holds the instance lock and the row is `Stopping` (or `Starting` with an identity). Reconciliation must leave the row, the identity and the `Changed` events for that instance alone. Paste the red output on the current head.
2. Fix it the same way as the other two phases: per instance, `lock_if_idle` (skip when busy), re-read the row, re-check `holds_process()` and the `Starting`-without-identity rule on the FRESH row, take the identity from the fresh row. The probe must still run outside the store lock; hold the instance guards across probe and persist (they are try-locks, so no deadlock with an operation that arrives meanwhile: it is refused as busy, which is the right answer during reconciliation).
3. Mutation evidence on a copy or while idle (rule in `_COMMON-RULES.md`): remove the new lock in the third phase -> your new test is red; keep the other two phases' tests green.
4. Update the stale doc comment on `Ops::is_busy` (`registry.rs`): it says reconciliation uses it.

## Not in this round

Everything else. The test-only hook code that lives inside `reconcile.rs` (about 100 lines under `#[cfg(test)]`) is a nit: move it into a `#[cfg(test)] mod` file only if it costs nothing.

## Bar

`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo deny check`, instance lib tests: the new test x100, `cargo test -p dblocal-core --lib` x20 at default threads and x20 with `RUST_TEST_THREADS=16 taskset -c 0-3`, loop a PRIVATE COPY of the test binary (copy it out of the shared `CARGO_TARGET_DIR` right after `cargo test --no-run`: another worktree's cargo rewrites the same-named binary under a running loop; print the distinct passed counts so a vacuous 0-test pass cannot hide), `cargo xtask ci` on the final head under the heavy lock (`CARGO_BUILD_JOBS=8`, `MemAvailable` above 8000 MB). Merge `main` first (W3-B2 lands on main right before you start: `git log main --oneline | head`; if it is not there yet, merge it before the final gates). Report with `worker_done` only with the red and green output pasted.
