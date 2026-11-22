# W2-A flake hunt 1 — two instance-service tests fail under CPU load

Branch: a NEW worktree from `main`. Edit only inside your own worktree (`git rev-parse --show-toplevel` must print a path under `~/orca/workspaces/db-local/`); the main repo path `/home/rezkycodes/Development/AI-Agent/db-local` only holds this spec and is READ-ONLY for you. Rules: `docs/orchestration/tasks/_COMMON-RULES.md` (rules 11-13).

## Evidence (reviewer, on `main` plus a trial merge, `cargo test -p dblocal-core --lib`, whole-lib loops while other workers' builds and e2e kept the machine at load 10-19)

- `instance::tests::safety::two_services_on_one_home_spawn_exactly_once` failed once in 25 runs (`906 passed; 1 failed`).
- `instance::tests::coverage::a_failed_start_settles_through_the_credential_and_store_paths` failed once in 40 runs, reported by another worker; it passed 10/10 in isolation.
- The same lib passed 15/15, 20/20 and 25/25 on a quiet machine many times. Both are in code merged from W2-A / W2-F follow-ups, not in the updater that was being reviewed.

A flaky test is a red CI run on GitHub and a lost evening for every worker, so this is worth fixing properly. Do not "fix" it with retries, longer sleeps alone, `#[ignore]`, or by weakening an assertion.

## Task

1. Reproduce each failure on demand: run the single test in a loop (200 times) while the machine is loaded (for example `nproc` busy loops: `for i in $(seq $(nproc)); do (while :; do :; done) & done`, kill them afterwards; or `taskset`/`nice` to starve the test; say what you used), and paste the panic message of a failing run. If you cannot reproduce after a serious attempt, say so with numbers and look for other load-sensitive tests the same way (full lib x30 under load) until you have at least one reproduction.
2. Find the root cause of each, from the failing output and the code: a real race in the product code (then fix it there, with a deterministic regression test that forces the interleaving), or a test that depends on timing, a port that something else may hold, a shared directory, the process table, or event ordering (then make the test deterministic: wait on a condition with a generous deadline rather than a fixed sleep, inject the clock or the barrier, never share ports across tests). Explain each root cause in plain words in `docs/architecture/service.md` or the test's doc comment.
3. Hunt for siblings: grep the instance and process test modules for the same pattern (fixed ports from `test_port()`, `thread::sleep`, polling with short deadlines, event streams subscribed after the operation started) and fix the ones you can prove fragile. List what you checked.
4. Port allocation: check what `test_port()` returns and whether two lib tests (or a lib test and a real engine started by another worker under `20000-29999`, `41000-41999`, `42000-42999`, `43000-43999`) can collide. Pick a range that the e2e and the other workers do not use and document it.

## Bar

Each reproduced flake: red before, green after, shown by a 200-iteration loop under the same load (paste counts). `cargo fmt --all -- --check`, `cargo clippy -p dblocal-core --all-targets -- -D warnings`, full `cargo test -p dblocal-core --lib` x30 on a loaded machine (paste), `cargo deny check`, `cargo xtask ci`. A mutation for each product-code fix (red, restored, clean tree). `worker_done` only with pasted evidence; otherwise `ask` and continue.
