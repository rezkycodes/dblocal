# W3-A fix round 1 — the generated password lands in the engine log, plus five smaller items

Same branch `w3-admin`. Rules: `docs/orchestration/tasks/_COMMON-RULES.md` (rules 11-13: real-engine runs under `flock`, never end a turn to wait). First `git merge main` into the branch (conflicts expected only in `crates/core/src/lib.rs` `pub mod` lines: keep both). Edit only `crates/core/src/admin/**`, `crates/core/tests/admin_e2e.rs`, `crates/core/tests/admin_support/**`, `docs/architecture/admin.md`, `crates/core/Cargo.toml`/`Cargo.lock` if (and only if) `cargo deny` accepts the dependency.

The reviewer verified round 0 independently on a trial merge with main: `cargo fmt`, `clippy -D warnings`, `cargo deny` green; admin lib tests 37 passed x20; full core lib 650 passed x5; `admin_e2e` 8 passed x3 under the lock; no engine left running. Ten of eleven reviewer mutations are killed by named tests (identifier charset, PG literal backslash and quote doubling, read-only getting write grants, any host accepted, rollback removed, verify skipped, reserved names, `Debug` leaking the password, identifier quoting dropped). Do not rework any of that. Problems:

## F1 (HIGH) — a failing `CREATE ROLE` / `ALTER ROLE` writes the cleartext password into the engine log

Reviewer probe on the real cluster (scratch test, reverted): `create_user("leaky")` twice; the second returns `UserExists` and the engine log (`logs/postgres.log`) contains `STATEMENT:  CREATE ROLE "leaky" LOGIN PASSWORD E'<the generated password>'` (`PROBE_LOG_HAS_PASSWORD_LITERAL=true`). PostgreSQL logs the text of a failing statement by default (`log_min_error_statement = error`). That file is exactly what `dblocal logs` and the Logs tab show and what users paste into bug reports. Requirement 4 ("passwords ... never appear in ... logs") is violated for every failure after the statement was parsed: role exists, statement/lock timeout, `ALTER ROLE` on a missing role, a failed rollback `ALTER`.

Fix, both parts:
1. Never send the cleartext. Compute the SCRAM verifier on the client (`postgres_protocol::password::scram_sha_256(password.as_bytes())`; the crate is already in the tree through `tokio-postgres`, version 0.6.12; add it as a direct dependency only if `cargo deny` accepts it and say so) and send `PASSWORD '<SCRAM-SHA-256$4096:...>'` through `quote_postgres_literal`. The PG server stores a verifier either way, so authentication is unchanged.
2. Belt and braces on every admin connection (the admin user is the superuser, so these are settable in the startup options): `-c log_min_error_statement=panic -c log_statement=none -c log_min_duration_statement=-1`.

Tests, on the real cluster: create a user whose password you pick (add a `#[doc(hidden)]`/`pub(crate)` seam so the test knows it; the production API stays as it is), then (a) `create_user` again (fails `UserExists`), (b) a failing `ALTER` (missing role), (c) the sink-failure rollback path: after all three, read the engine log and assert that none of the known passwords appears, that there is no `PASSWORD E'`, and that the created role can still authenticate with the cleartext password (the verifier is right) and the reset e2e still passes. Mutations (red pasted, restored): put the cleartext literal back; remove only the log options (decide honestly whether the log test stays green because SCRAM alone suffices, and say so). In `admin.md`: record the finding, the fix, and for the later MySQL adapter that the general query log (if the user enables it) records `CREATE USER ... IDENTIFIED BY '<cleartext>'`: propose sending a precomputed hash (`IDENTIFIED WITH ... AS '<hash>'`); do not implement it now.

## F2 (MEDIUM) — a wrong password and a stopped engine are the same error

`with_client` maps every connect failure to `AdminError::Connect` (DBL-8100, "engine unreachable"). The GUI cannot tell the user "the stored password does not match" from "start the instance". tokio-postgres connect errors carry SQLSTATE `28P01` (invalid password) and `28000` (invalid authorization): map those to a new `AdminError::AuthFailed` (DBL-8116, own Indonesian message, in the 8100-8199 range, added to the table test and `admin.md`). Timeouts and refused connections stay `Connect`. Real-cluster tests: wrong password -> `AuthFailed`; closed port -> `Connect`; the messages differ; neither error's `Debug`/`Display` contains the password.

## F3 (MEDIUM) — calling the admin API from inside a tokio runtime panics

`with_client` builds a runtime and calls `block_on`. The wiring will call this from a service that already runs on tokio. Reproduce first and paste the red run: a `#[tokio::test]` that calls `PgAdmin::list_databases` on a closed port (expected today: panic "Cannot start a runtime from within a runtime"). Fix so the API is safe from any context: run each operation's async body on a short-lived dedicated OS thread (`std::thread::scope`) and join it, or an equivalent that you can justify. Tests: closed port from a `#[tokio::test]` (current-thread and multi-thread flavours) returns `Err(Connect)`, and from `tokio::task::spawn_blocking`. Document the contract in `admin.md`.

## F4 (LOW) — `UserNotFound` (DBL-8108) is never produced

The table says "reserved for the drop path" but `drop_user`/`revoke`/`grant` on a missing role return `QueryFailed(42704)`. Map the role statements' `42704` to `UserNotFound` at the call sites (do not map `42704` globally: it also means other missing objects), e2e: drop and revoke on a missing user. Remove the word "reserved" from the table.

## F5 (LOW) — the `force` choice is not pinned by any test

Mutation `let statement = if force {` -> `if true {` survived all lib and e2e tests, because the pre-check hides it. Move the statement construction into a pure builder `drop_database_postgres(db, force)` with exact-SQL unit tests for `false` and `true`, and add to the e2e: with a live session and `force = true` the database is dropped and the session is terminated; with `force = false` the result is `DatabaseInUse` and the session is still alive. Rerun the mutation: red.

## F6 (LOW) — one spec item was only half done

The spec required "hostile names are rejected BEFORE any statement reaches the server (assert with the server's statement log or a counter)". The e2e `hostile_names_are_rejected_before_any_statement_reaches_the_server` only checks that the database count is unchanged, which does not show that nothing reached the server. Enable `log_statement = 'all'` for that test's cluster (or count statements another way), exercise every public method that takes a name with the hostile corpus (the typed constructor rejects first, so no call should reach the server), and assert that the statement log has no statement containing any hostile fragment and no `CREATE/DROP/GRANT/ALTER` at all after the setup. Mutation: relax the identifier check so one hostile name gets through to a statement: the e2e must be red as well (not only the lib test).

## Bar

`cargo fmt --all -- --check`, `cargo clippy -p dblocal-core --all-targets -- -D warnings`, `cargo test -p dblocal-core --lib admin` x40, full `--lib` once, `cargo deny check`, `cargo xtask ci`, `admin_e2e` three consecutive runs under `flock` (summaries pasted), coverage with `CARGO_LLVM_COV_TARGET_DIR=$HOME/.cache/dblocal-target-cov-w3a` (delete afterwards; table pasted: pure layer >= 90%, every file >= 80%). Mutations red then restored, `git status --short` clean. `worker_done` only with pasted evidence; otherwise `ask` and continue with unblocked parts.
