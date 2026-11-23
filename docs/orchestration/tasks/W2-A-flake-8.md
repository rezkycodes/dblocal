# W2-A flake 8 — one load-only timing flake (small)

New branch from main after I merge flake 7 (`git checkout -b rezkycodes/w2-flaky-instance-4 main`). Rules `docs/orchestration/tasks/_COMMON-RULES.md` incl. 11-14.

You found it yourself: `process::spawn::tests::stdout_and_stderr_are_appended_to_the_log` fails with `log was ""` under four taskset burners (3/25), because a fixed 250 ms sleep is not enough for the child's output to reach the log file at load 22.

Do: replace the fixed sleep with a bounded poll (deadline 10 s, 10 ms steps, a failing message that says what was missing and what the file held at the deadline); keep the assertions on content exactly as strong as they are. Then grep the workspace for the same pattern, a fixed `thread::sleep`/`tokio::time::sleep` of a few hundred ms followed by an assertion on something an OTHER process or thread writes, in `crates/core/src/process/**`, `crates/core/src/engine/**/tests*`, `crates/core/src/ports/**`, `crates/core/src/update/**` tests; list every hit in the report with a verdict (fixed here, or why it is safe), fix the ones that wait for a child or a file. Red first: show the old test failing under `taskset -c 0 ... yes` burners or with a delay injected into the child (paste).

Bar: fmt, clippy workspace, deny, each changed test x100 from a private copy (check with `--list` that your tests are inside the copy), whole lib x25 default and x25 `RUST_TEST_THREADS=16 taskset -c 0-3` with four `taskset -c 0-3 yes` burners (kill them by pid after), `cargo xtask ci` under the heavy lock (`CARGO_BUILD_JOBS=8`, MemAvailable above 8000 MB). `worker_done` with red/green output pasted.
