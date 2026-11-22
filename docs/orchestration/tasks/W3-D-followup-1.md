# W3-D follow-up 1 — `dblocal create --no-password` and `dblocal uri` for such an instance, on the real binary

Branch `w3-cli` (already merged into main at `f9f1f39`; first `git merge main` into it). Rules: `docs/orchestration/tasks/_COMMON-RULES.md` (rules 11-13: real-engine runs under the machine lock, never end a turn to wait). Edit only `crates/cli/**` and `docs/architecture/cli.md`. Do NOT touch `crates/core/**`.

## Why

In W3-D round 1 the reviewer found that on the real binary `dblocal create --no-password` and `dblocal uri` for the resulting instance exited 3 (`[DBL-1002]`) although the instance exists and runs. The cause was in the core (`instance/read_ops.rs::connection_uri` required a stored credential). The core fix is now merged on main (`e36e58c`, merge of W2-F follow-up): for `AuthMode::None` it renders `redis://127.0.0.1:PORT/0` for Valkey and `postgresql://USER@127.0.0.1:PORT/DB` (no colon, no password) for PostgreSQL, and `.env` omits `REDIS_PASSWORD`/`DB_PASSWORD` instead of printing an empty one. The reviewer confirmed it on the merged tree with the real binary: Valkey `uri` printed `redis://127.0.0.1:6392/0`, `valkey-cli PING` answered `PONG` without a password; PostgreSQL `uri` printed `postgresql://postgres@127.0.0.1:15433/postgres` and `psql` connected with it. The CLI has no test that pins this, and the CLI mock service (`MockService`) must not hide it again.

## Tasks

1. `cli_e2e` (real binary, real Valkey, same cached runtime as the existing case): add one case `no_password_instance_end_to_end`: `create --no-password` (stdout is exactly the URI plus `\n`, no userinfo, no colon before `@` because there is no `@`), `uri` (exact stdout, exit 0), `uri --env` (exact stdout built from the expected block, contains no `PASSWORD` line), `start`, a `redis` client connecting WITHOUT a password and answering `PING`, `status` exit 0, `stop`, `delete`. Run it with the other cases under `flock -w 3600 $HOME/.cache/dblocal-e2e.lock cargo xtask e2e` three times.
2. Unit/integration tests in `crates/cli` for the same behaviour through the mock service: `uri` for a no-password instance prints the no-password shape, exit 0; check the mock follows the core rule (`crates/core/src/service/mock/ops.rs`, read only); if the CLI has its own test double, it must honour the same rule, and a test must fail if it does not.
3. `cli.md`: one paragraph with the contract (what `create --no-password`, `uri` and `uri --env` print for each engine) and the reminder that a password-less instance is only appropriate for loopback.
4. Mutation (red output pasted, restored, `git status --short` clean): make `uri` print `redis://:@127.0.0.1:PORT/0` or add `default:` (any change to the shape): the e2e and the exact-stdout unit test must both fail.

## Bar

`cargo fmt --all -- --check`, `cargo clippy -p dblocal-cli --all-targets -- -D warnings`, `cargo test -p dblocal-cli` x40, `cargo xtask ci`, `cargo deny check`, three locked `cargo xtask e2e` runs (summaries pasted). `worker_done` only with pasted evidence; otherwise `ask` and continue.
