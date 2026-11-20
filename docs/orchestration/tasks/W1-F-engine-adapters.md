# W1-F — Engine adapters: PostgreSQL and Valkey

Branch: `w1-engines`. Read first: AGENTS.md, `docs/architecture/CONTRACT.md`, PRD §6 `F-INS-02..03`, `F-LOG-02`, `F-CRD-02`, decisions D-13, D-16, D-17, **D-24**, and above all
`docs/spikes/engines/README.md` sections 1 and 3 plus `postgres.md` and `valkey.md`. Those documents were produced and re-verified on real binaries; **follow them, do not
re-derive them**. Where you disagree with a finding, prove it with a command and tell me.

## Goal

Real `EngineAdapter` implementations that initialise, describe how to start, health-check, stop and diagnose PostgreSQL and Valkey.

## You own

`crates/core/src/engine/postgres.rs`, `engine/postgres/**`, `engine/valkey.rs`, `engine/valkey/**`, `engine/uri.rs`, `engine/glibc.rs`, `docs/architecture/engines.md`.
Do not touch `engine/mod.rs`, `mysql.rs` or `mariadb.rs`; if the trait needs a change, use `ask`. Spawning detached processes is **not** your job (W1-D and W2-A): your
`spawn_command` only builds the `Command`; you parse pidfiles and report identities.

## Deliverables

### PostgreSQL

- `initdb` with `--encoding=UTF8`, the locale fallback chain `C.UTF-8` then `C.utf8` then `C`, `--auth-local` and `--auth-host` per auth mode, password through a
  `0600` `--pwfile` that is deleted immediately afterwards. Remove a partially created data directory on failure and keep the raw output.
- Post-init config: `listen_addresses = '127.0.0.1'`, `port`, `unix_socket_directories` pointing at a **short** directory, with the full `<dir>/.s.PGSQL.<port>` length checked
  against 107 bytes at create and start time (finding C-09).
- Start with `pg_ctl -w -t <consts::START_TIMEOUT> -l <log> start`. Identity from `postmaster.pid`: line 1 PID, line 3 start epoch, plus the resolved `postgres` executable (C-08).
- Health: a real protocol query (`select 1`) with `tokio-postgres` so the password never leaves the process. `pg_isready` alone is not enough (C-06).
- Stop: `pg_ctl stop -m fast -w -t <consts::STOP_GRACE>`; escalation is the service's job through W1-D, so return a precise outcome.
- Admin operations (`AdminOps`) stay unimplemented in this task (Phase 2); say so through the trait's own "unsupported" path.

### Valkey

- Render `valkey.conf`: `bind 127.0.0.1 -::1`, `protected-mode yes`, `port`, `daemonize no`, `supervised no`, `dir`, `pidfile`, `logfile`, `requirepass` when set, persistence per spec.
  Create `dir` first (it must exist). The password lives in the config file written `0600`, never in argv.
- Start command: `valkey-server <conf>`. Health: `PING` through the `redis` crate; parse the reply, never trust a CLI exit code (C-12).
- Stop: `SHUTDOWN NOSAVE` or `SAVE` over the protocol; SIGTERM is graceful in Valkey (C-11) and left to the service as the fallback.
- Failure diagnosis must read the **log tail**, not only stderr (C-10).
- `engine/glibc.rs`: parse the host glibc version (for example from `ldd --version` output, as a pure function plus a thin runner) so the catalog can choose `jammy` or `noble` (C-03).

### Shared

- `connection_uri` for every `UriFormat`: `postgresql://user:pw@127.0.0.1:port/db` and `redis://default:pw@127.0.0.1:port/0`, with RFC 3986 userinfo percent-encoding (W0-B proved
  unencoded hostile characters break parsing). `DotEnv` renders framework-style variables. Return `SecretString`.
- `diagnose_start_failure(log_tail, exit_code) -> StartFailure` for port in use, wrong-major data directory, permission denied, socket directory problems, bad configuration,
  and append-only directory failures. Use plain substring matching on the stable markers in the spike's verbatim failure texts (no new `regex` dependency). Unknown failures keep the raw tail.
- Executable names get `.exe` on Windows through one helper.

## Tests (RED first)

Unit: config rendering, argument construction (assert no password in any argument), URI encoding with `@:/?#[]%` space and non-ASCII, glibc parsing of real `ldd --version` samples,
and **every verbatim failure text from the spike as a table-driven diagnosis test**. Integration against real binaries: extract the cached archives from `$HOME/.cache/dblocal-dev/`
(PostgreSQL 18.7.0 and 17.12.0, Valkey 9.2.0 jammy and noble) into a temp directory, then run full init, start, health, stop and restart **with ports 41000-41999 only**, including PostgreSQL
18 and 17 side by side. Gate these with an environment variable and `#[ignore]` so `cargo test` stays offline, and **run them yourself and paste the results**. Leave no process, directory or listener behind.

## Done when

`cargo xtask ci` green, >= 80% lines on the unit-testable files, the integration run pasted, and a list of anything that differs from the spike.
