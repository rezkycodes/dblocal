# W2-A flake hunt, round 2 — reproduce in the WHOLE-lib run, not in a single-test loop

Same branch `rezkycodes/w2-flaky-instance` (HEAD `e79d91b`: two commits making two tests tolerate broadcast lag). Rules: `docs/orchestration/tasks/_COMMON-RULES.md` including the new rules 11-14. Edit only inside your worktree.

## What round 1 did not achieve

You verified single tests in 200-iteration loops pinned to one core. Every failure the reviewer and other workers saw happened in a WHOLE-lib run (`cargo test -p dblocal-core --lib`, ~950 tests in parallel threads in one process), at machine load 4 to 20, and never in isolation (the same tests passed 10/10 and 200/200 alone). So the loops you ran cannot reproduce the bug; "PASS 200/200" proves nothing here. Failures seen so far, whole-lib runs of `main`-derived trees:

- `instance::tests::safety::two_services_on_one_home_spawn_exactly_once` (906 passed; 1 failed, run 21 of 25)
- `instance::tests::coverage::a_failed_start_settles_through_the_credential_and_store_paths` (1 in 40, reported by another worker)
- `instance::tests::races::a_delete_refuses_to_remove_a_row_whose_engine_survives`
- `engine::postgres::tests::shutdown_reports_a_nonzero_pg_ctl` (948 passed; 1 failed, run 4 of 8 at load 4.6-8: this is NOT a heavy-load effect)
- `archive_recovery` (named by another worker; failed once inside `cargo xtask ci`)

## Hypotheses to test first (cross-test interference inside one process)

1. **ETXTBSY (`Text file busy`)**: several of these tests write a fake script (a fake `pg_ctl`, a fake engine) and then execute it. If another test thread forks at that moment, the child inherits the open write descriptor and `exec` of the script fails with ETXTBSY. The updater worker found exactly this class in W3-U and added a bounded retry in its runner. Grep the instance, engine and process test helpers and harnesses for "write a script then spawn it" and for `chmod`/`set_permissions` followed by `Command::new`.
2. **Process-wide state**: tests that change `PATH`, `HOME`, the current directory, the umask or other environment variables, or that rely on the process table / pid reuse / `/proc` scans, while other threads run.
3. **Shared ports or directories** between tests (`test_port()` allocation, fixed names under a shared temp dir).
4. **Broadcast/event timing** (what you fixed in round 1): keep those two commits only if you can show the failure they remove.

## Method

1. Reproduce: run the WHOLE lib in a loop (`cargo test -p dblocal-core --lib 2>&1 | tee run-N.log` for N in 1..60, keep every failing log) on a machine at ordinary load, and add `RUST_TEST_THREADS=16` or `32` to raise the interleaving. Paste the panic text of each distinct failure (not just the test names). A reproduction is a failing whole-lib run with its message. Use `taskset -c 0-1` (two cores) for added contention if needed, never all cores (rule 14).
2. For each root cause: fix the real defect (a race in product code, or a test helper that makes tests interfere), with a regression test that forces the interleaving deterministically where possible (for example a helper that executes a just-written script while another thread forks in a loop; it must fail before the fix and pass after). Prefer ONE shared helper over seven local retries: for fake executables write the file, `sync_all`, close it, and run it through a single helper that retries ETXTBSY a bounded number of times, or create the script atomically (write to a temp name, `rename`) in a directory no other thread holds open for writing.
3. Prove it: after the fixes, the whole lib 60 times in a row with zero failures (paste the loop output: count, load average before and after), with the same `RUST_TEST_THREADS` that reproduced it before.
4. Siblings: list every test helper that writes then executes a file, and every place that mutates process-wide state in a test, and say what you did about each.

## Bar

`cargo fmt --all -- --check`, `cargo clippy -p dblocal-core --all-targets -- -D warnings`, `cargo deny check`, `cargo xtask ci` (under `flock -w 7200 $HOME/.cache/dblocal-heavy.lock`, load < 8, load pasted), the 60-run whole-lib loop pasted, and the before/after evidence for each root cause. `worker_done` only if the 60-run loop is clean; otherwise `ask` and continue.
