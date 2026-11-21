# W3-A resume 1 — your question was answered, continue the task

You asked (msg_be4cc705a11a) whether you may edit `crates/core/src/error.rs` and `strings.rs` for admin errors, then went idle waiting. The answer was sent but Orca never woke you (a coordinator-side delivery problem, not your fault; about four hours were lost). Here it is again:

- Do NOT edit `crates/core/src/error.rs` or `crates/core/src/strings.rs`.
- Keep a MODULE-LOCAL error enum under `crates/core/src/admin/` (for example `admin/error.rs`) with `code()` and `user_message()` (Indonesian, no secrets, no raw SQL, no passwords) and convert with `UserFacingError::new(code, message)`.
- Your range is `DBL-8100..8199`. The rule, the other modules' ranges and the test pattern are in `docs/architecture/error-codes.md` (on main: `git merge main` first, it only adds docs and other modules' files).
- Required tests: every code inside the range and unique; a canary-secret test (a password and a connection string placed in every variant never appear in Debug, Display, `code()` or `user_message()`); no variant carries an unscrubbed driver error string.

Then do the rest of `docs/orchestration/tasks/W3-A-admin-postgres.md` in order: identifier validation and quoting, create/drop database, users and privilege levels against real PostgreSQL, e2e x3 on the real engine, mutation checks, coverage bar. You already have a skeleton commit (`1cde538`); commit after every step (at the latest every 30 minutes). Do not end your turn to wait for an answer again: if blocked, send `ask` and CONTINUE with the unblocked parts. Report with `worker_done` only with pasted evidence (rule 11).
