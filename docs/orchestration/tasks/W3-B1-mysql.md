# W3-B1 — MySQL engine adapter (F-ENG-01), Linux first, real binaries

Branch `w3-mysql` from `main`. Read, in this order: `docs/orchestration/tasks/_COMMON-RULES.md` (rules 11-13 matter: every claim is pasted output; real-engine runs go under `flock -w 3600 $HOME/.cache/dblocal-e2e.lock`; never end a turn to wait, `ask` and continue), `AGENTS.md`, `docs/architecture/CONTRACT.md`, `docs/architecture/engines.md` (how the PostgreSQL and Valkey adapters are built; copy their shape), PRD `F-ENG-01`, `F-INS-02..03`, `F-LOG-02`, `F-CRD-02/03`, D-16, D-17, D-24, and `docs/spikes/engines/README.md` (C-01, C-02, C-05) plus `upstream-sources.md` section 2. The adapter today is a stub (`StubAdapter` in `engine/mod.rs`, empty `engine/mysql.rs`).

## Goal

A real `EngineAdapter` for MySQL 8.4 LTS: initialise, start command, protocol health check, graceful shutdown, connection URI, start-failure diagnosis. It must work through the existing `LocalService` unchanged. MariaDB is a SEPARATE worker's task (W3-B2): do not touch `engine/mariadb.rs`.

## You own

`crates/core/src/engine/mysql.rs`, `engine/mysql/**`, the `EngineKey::Mysql` arm of `adapter_for` in `engine/mod.rs` (one line plus its test; the `Mariadb` arm stays a stub), `docs/spikes/engines/mysql.md` (new, real findings), `docs/architecture/engines.md` (a MySQL section only). Not the catalog (`catalog/**`, `xtask/**`): a follow-up wave enables real MySQL artifacts there. If you need a trait change, use `ask`.

## Step 0: a short spike on the real binary (commit its notes first)

Download the Linux x86_64 **minimal** MySQL 8.4 tarball that D-24 names (about 80 MB; find the exact file under `https://dev.mysql.com/get/Downloads/MySQL-8.4/` or `cdn.mysql.com`; if you cannot find a minimal tarball, say so with the URLs you tried and use the full one only if the reviewer agrees via `ask`). Cache it under `$HOME/.cache/dblocal-dev/` (outside the repo), compute SHA-256 yourself, verify the GPG `.asc` signature if `gpg` and the MySQL release key are available, and say what you checked. On the real binary record in `docs/spikes/engines/mysql.md`: layout and `strip_prefix`, glibc floor (`readelf -V`), required host libraries (`ldd`), which binaries the minimal tarball contains (`mysqld`, `mysqladmin`, `mysql`?), `mysqld --initialize-insecure` output and time, every listener the server opens (the X Protocol on 33060 and `mysqlx_socket` must not be left on: an instance must listen ONLY on its own `port` at `127.0.0.1`; prove with `ss -ltnp`), the socket path length limit, shutdown by SIGTERM vs by SQL, how a wrong data directory, a busy port, a bad `my.cnf` and a permission error look in the log (verbatim texts, they feed the diagnosis table).

## Deliverables (follow the shape of `engine/postgres.rs` and `engine/valkey.rs`)

- `initialize`: `mysqld --initialize-insecure --datadir=<dir>` (the root account starts without a password), then set the superuser password (or keep none for `AuthMode::None`) WITHOUT putting it in argv, a file other than 0600, or a log: use a one-shot init file (`--init-file`, 0600, deleted at once) or a protocol connection from the adapter, whichever the spike shows works; `my.cnf` written 0600 with `bind-address=127.0.0.1`, `port`, `datadir`, a SHORT `socket` path (check the 107-byte limit at create and start time), `mysqlx=OFF` (or the equivalent that removes the second listener), `pid-file`, `log-error`, `skip-name-resolve`. Remove a half-built data directory on failure and keep the raw output.
- `spawn_command`: `mysqld --defaults-file=<my.cnf>`; never a password in argv or env of the server. `.exe` on Windows through the existing helper (Windows and macOS are written but UNVERIFIED here; say so).
- `health_check`: a real MySQL protocol handshake (parse the server greeting packet, protocol version 10, and treat an authentication challenge as `Ready`, like the other adapters), with a connect timeout; never trust a process check or an exit code. A crate (`mysql_async`, `mysql`) is allowed only if `cargo deny check` accepts it (licence GPL-3.0-only compatible); otherwise a small hand-written reader with tests. Justify the choice in `engines.md`.
- `shutdown`: graceful, bounded by `consts::STOP_GRACE_SECS`, returns a precise outcome; SIGTERM-and-wait is acceptable if the spike shows `mysqld` exits cleanly on it (verify the log says "Shutdown complete"); otherwise SQL `SHUTDOWN`.
- `connection_uri`: `mysql://user:pw@127.0.0.1:port/` for `Uri` and the `.env` block for `DotEnv`, RFC 3986 userinfo encoding (`uri.rs` already has the MySQL shapes: extend only if the real client disagrees); `connection_uri_no_password` already exists in the trait (check the MySQL shape: `mysql://root@127.0.0.1:3306/`).
- `diagnose_start_failure(log_tail, exit_code)`: port in use, wrong data directory version, permission denied, bad configuration, socket path too long, disk full, using the REAL texts from the spike as table-driven tests; unknown keeps the raw tail.
- Never log or format a password; a canary test scans every error and the spawned command.

## Tests

Unit: config rendering (exact text), argv/env construction (no password anywhere), URI encoding with `@:/?#[]%`, space and non-ASCII, handshake parser with captured real greeting bytes (store them as test fixtures) plus truncated, oversized and hostile packets, every diagnosis text. Integration against the REAL tarball (extract from the cache into a temp dir, gate with an env var and `#[ignore]` like W1-F's, ports **42000-42999 only**): init, start, health, a login with the `mysql` client or your own connection using the superuser password, wrong password rejected, stop, restart, data survives, two instances side by side, and the `ss -ltnp` check that only the instance's own port listens. Run them yourself under the machine lock and paste the summaries three times; leave no process, directory or listener behind (a leak scan like `crates/core/tests/support` has).

## Mutation checks (red output pasted, restored, `git status --short` clean)

password into argv; `bind-address` made `0.0.0.0`; `mysqlx` left on; socket path length check removed; init-file left on disk; handshake accepting any bytes; diagnosis table row removed. Each must fail a NAMED test.

## Bar

`cargo fmt --all -- --check`, `cargo clippy -p dblocal-core --all-targets -- -D warnings`, `cargo test -p dblocal-core --lib engine` x20, `cargo deny check`, `cargo xtask ci`, coverage with `CARGO_LLVM_COV_TARGET_DIR=$HOME/.cache/dblocal-target-cov-w3b1` (delete afterwards; every file >= 80% lines, pure logic >= 90%), all pasted. The report lists exactly what is unverified (macOS, Windows, aarch64, other glibc, MySQL 9). `worker_done` only with pasted evidence; otherwise `ask` and continue.
