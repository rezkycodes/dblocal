# W3-B1 resume 1 — finish the fix round (you ended your turn while waiting for a lock)

Same branch `rezkycodes/w3-mysql`, same worktree, same rules (`_COMMON-RULES.md`, rules 11-14). The spec of the round is `docs/orchestration/tasks/W3-B1-fix-1.md`; HEAD is `50ceedb`. You stopped with "I'll complete these once locks clear". A turn that ends does not resume by itself (Orca never wakes an idle worker): never end your turn to wait for a lock or a process; run the command with `flock -w 7200` in the foreground or in the background with `until ...; do sleep 10; done` (bounded), and keep going.

Left to do, each with pasted output:

1. `cargo xtask ci` under `flock -w 7200 $HOME/.cache/dblocal-heavy.lock`, wait for `awk '{print $1}' /proc/loadavg` below 8 first, paste the load before and after and the final "xtask ci: all gates passed" line. Your earlier run's log was empty because of a `| tail` pipe; write to a file and `tail` the file when it finishes. If it fails with the GPUI render SIGABRT at load above 10, rerun at lower load; at low load it is a real failure.
2. The real-binary mutation for P3: `bind-address=0.0.0.0` must be killed BOTH by the unit test and by `assert_only_instance_loopback_port` in `mysql_e2e` (run the e2e under `flock -w 3600 $HOME/.cache/dblocal-e2e.lock`; the MariaDB worker also uses that lock, so wait for it in the foreground). Also the `AuthMode::None` branch granting nothing to `127.0.0.1`: the no-password e2e must go red. Restore, `git status --short` clean.
3. The mutation of P2 (password on the `mysqladmin` argv) must be red on a named unit test; paste it.
4. Coverage of `engine/mysql*` with `CARGO_LLVM_COV_TARGET_DIR=$HOME/.cache/dblocal-target-cov-w3b1b` (lib + `mysql_e2e` merged), paste the per-file table, delete the directory afterwards. The reviewer measured round 0 at 80.22% lines for the single file; pure logic must be at 90% or more.
5. Then `worker_done` with the evidence above and everything from your last report that still holds. Re-run `git rev-parse --short HEAD` and name only hashes you pasted.
