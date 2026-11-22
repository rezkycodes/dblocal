# Engine adapters (W1-F)

This document records the concrete behavior implemented in `crates/core/src/engine/`
for PostgreSQL, Valkey and MySQL. It follows the W0-B/W3-B1 spike findings and the
coordinator decisions. Spawning detached processes is owned by W1-D/W2-A; adapters
build commands, parse identities, run protocol health checks, stop engines and
diagnose failures.

## Shared conventions

- **Connection values are secrets.** `EngineAdapter::connection_uri` returns
  `SecretString` and takes `(ctx, credential, format: UriFormat)`.
  - `UriFormat::Uri` returns the bare URI with RFC 3986 percent-encoded userinfo
    (`encode_userinfo` in `engine/uri.rs` encodes every byte that is not
    unreserved, so `@ : / ? # [ ] %` space and non-ASCII are safe).
  - `UriFormat::DotEnv` returns a framework-style block whose values are the raw
    credential components (and a percent-encoded `*_URL` line). The block is a
    secret because it contains the password.
- **No-password instances still have a URI (`F-CRD-03`).** An instance created
  with `AuthMode::None` has no credential row, so `EngineAdapter::connection_uri`
  is not used for it. `connection_uri_no_password` (default impl backed by
  `uri::render_no_password`) renders the same formats without a password:
  PostgreSQL `postgresql://USER@127.0.0.1:PORT/postgres`, Valkey
  `redis://127.0.0.1:PORT/0`, and a `.env` block that omits `DB_PASSWORD` /
  `REDIS_PASSWORD`. A password-mode instance with a missing credential stays a
  typed `NotFound { kind: "credential" }`, never an empty password.
- **No password in argv.** Start/stop/init commands never carry a password as an
  argument. PostgreSQL bootstraps via a `0600` `--pwfile` deleted immediately
  after `initdb`; Valkey writes `requirepass` in a `0600` `valkey.conf`;
  PostgreSQL health checks pass the password through the protocol
  (`tokio-postgres`), not `PGPASSWORD`/argv.
- **Binary names.** `uri::exe_name` appends `.exe` on Windows.
- **Diagnosis.** `diagnose_start_failure` does plain substring matching on the
  stable markers from the spike's verbatim failure texts (no `regex` crate).
  Unknown cases return `StartFailure::Unknown` and the raw log tail is kept by
  the caller.
- **Admin ops.** Phase 2; both adapters return `None` from `admin_ops()`.

## PostgreSQL (`engine::postgres`)

Spike refs: README C-06..C-09, `docs/spikes/engines/postgres.md`.

- **Port/config sync (D-2):** `sync_port(data_dir, run_dir, port)` rewrites the
  DBLocal-managed block in `postgresql.conf` atomically (temp file in the same
  directory, fsync, rename). It preserves other settings and comments. `start`
  calls it before spawning so the config can never disagree with the store.

- **Initialize (`initdb`):**
  - Runs `bin/initdb -D <data_dir> -U <superuser> --encoding=UTF8 --locale=<loc>
    --auth-local=<a> --auth-host=<a>` with `--pwfile=<path>` when
    `auth == Password`.
  - Locale fallback chain: `C.UTF-8`, then `C.utf8`, then `C` (selected from
    `locale -a` when available).
  - Auth: `Password` -> `scram-sha-256` for local and host; `None` -> `trust`
    for local and host.
  - Password bootstrap: a `0600` pwfile written next to the data directory,
    deleted immediately after `initdb` (success or failure).
  - On any init failure the partially created data directory is removed and the
    raw `initdb` output is returned in the engine error.
  - Post-init `postgresql.conf` appends: `listen_addresses = '127.0.0.1'`,
    `port = <port>`, `unix_socket_directories = '<run_dir>/sock'`.
  - **Unix socket is optional (C-09).** DBLocal only ever connects over TCP
    `127.0.0.1`, so the unix socket exists only as a convenience. `choose_socket_dir`
    is the single gate: if the full `<run_dir>/sock/.s.PGSQL.<port>` path is within
    the kernel's 107-byte `sun_path` limit it returns `SocketDir::Enabled(dir)` and
    the directory is created `0700`; otherwise it returns `SocketDir::Disabled`,
    logs which path was taken (no secrets), and continues TCP-only. An over-long
    home path therefore never fails instance creation:

    | Path length | `unix_socket_directories` | Behaviour |
    |---|---|---|
    | `<= 107` bytes | `'<run_dir>/sock'` | unix socket created, dir `0700` |
    | `> 107` bytes | `''` (empty) | unix sockets disabled, TCP loopback only |

    Both post-init and start (`prepare_run`) apply the same rule, so a create under
    a short path and a later start under a longer one stay consistent with each
    other. `validate_socket_path_len` remains the pure length check the chooser
    uses.

- **Start command:** `bin/pg_ctl -D <data_dir> -l <logs_dir>/postgres.log -w
  -t <START_TIMEOUT_SECS> start`. `spawn_command` returns this command; it never
  carries a password.

- **Identity:** `postmaster.pid` is parsed by `parse_postmaster_pid` /
  `read_postmaster_pid` into `PostmasterIdentity { pid, data_dir, start_epoch,
  port, socket_dirs, status }` (lines 1,2,3,4,5,8). Before signalling a PID the
  executor must verify the executable resolves to the expected `postgres` binary
  and the process start time matches line 3 (C-08). Adapters do not signal.

- **Health:** a real protocol query `SELECT 1` over `tokio-postgres`
  (`127.0.0.1:<port>`, db `postgres`, superuser, 5s connect timeout). A backend
  auth challenge (`28xxx`) proves the server is up and is treated as
  `Health::Ready` (C-06, trait contract). `pg_isready` is not used.

- **Stop:** `bin/pg_ctl -D <data_dir> -m fast -w -t <STOP_GRACE_SECS> stop`.
  Escalation to a forced kill is the service/W1-D's job; a non-zero exit returns
  an engine error with the raw output.

- **Connection values:**
  - `UriFormat::Uri`: `postgresql://<user>:<enc_pw>@127.0.0.1:<port>/postgres`
  - `UriFormat::DotEnv`:
    - `DATABASE_URL=<uri>`
    - `DB_HOST=127.0.0.1`
    - `DB_PORT=<port>`
    - `DB_DATABASE=postgres`
    - `DB_USERNAME=<superuser>`
    - `DB_PASSWORD=<password>`

- **Diagnosis mapping (substring markers):** every case without an honest
  variant maps to `StartFailure::Unknown` so the GUI shows the raw log lines
  instead of a misleading fix.
  - `StartFailure::PortInUse`: `could not bind IPv4 address`,
    `Is another postmaster already running on port`, `could not create any TCP/IP sockets`
    with `Address already in use`.
  - `StartFailure::DataDirVersionMismatch`: `database files are incompatible with server`,
    `initialized by PostgreSQL version`.
  - `StartFailure::PermissionDenied`: `Permission denied` only.
  - `StartFailure::BinaryMissing`: exec `Exec format error`, shell exec
    `No such file or directory` that is not a socket lock-file error.
  - `StartFailure::Unknown`: unix socket path too long, a missing
    `.s.PGSQL.*` lock file, `pg_ctl: directory "..." does not exist` (a missing
    data directory is not a missing binary), and anything unrecognized. The raw
    tail is kept by the caller.

## Valkey (`engine::valkey`)

Spike refs: README C-03, C-10..C-12, `docs/spikes/engines/valkey.md`.

- **Initialize (`valkey.conf`):** creates `<data_dir>`, `<instance>/run`,
  `<instance>/logs` (data dir must exist before start). Writes a `0600`
  `run/valkey.conf` containing:
  - `bind 127.0.0.1 -::1`, `protected-mode yes`, `port <port>`
  - `daemonize no`, `supervised no`
  - `pidfile <run>/valkey.pid`, `logfile <logs>/valkey.log`, `dir <data_dir>`
  - `requirepass <password>` only when `auth == Password` (the password never
    appears in argv; it lives only in the `0600` config)
  - persistence: `appendonly yes`, `appendfilename "appendonly.aof"`,
    `appenddirname "appendonlydir"`, periodic RDB saves.

- **Start command:** `bin/valkey-server <run>/valkey.conf`. The config sets
  `daemonize no`, so the caller owns the child/process group.

- **Identity:** `run/valkey.pid` is parsed by `parse_pidfile` /
  `read_pidfile` into `ServerIdentity { pid }`. The executor must still verify
  the executable is the expected `valkey-server` and match process start time
  before signalling (C-08 discipline).

- **Health:** `PING` via the `redis` crate over RESP
  (`redis://...@127.0.0.1:<port>/0`, 5s timeout). The reply is parsed: `PONG`
  means ready; an auth challenge (`NOAUTH` / `AuthenticationFailed`) means the
  server is up and is treated as `Health::Ready`. `valkey-cli` exit codes are
  never trusted (C-12).

- **Stop:** `SHUTDOWN NOSAVE` over the protocol (AOF persistence). A socket
  closed by the server after the command is treated as success. SIGTERM is a
  graceful fallback in Valkey (C-11) but escalation is the service's job.

- **Connection values:**
  - `UriFormat::Uri`: `redis://<user>:<enc_pw>@127.0.0.1:<port>/0`
    (the `redis://` scheme is used for client compatibility; `valkey://` also
    works with the bundled client per spike).
  - `UriFormat::DotEnv`:
    - `REDIS_URL=<uri>`
    - `REDIS_HOST=127.0.0.1`
    - `REDIS_PORT=<port>`
    - `REDIS_PASSWORD=<password>`

- **Diagnosis mapping (substring markers):** Valkey writes some fatal causes
  only to `logfile` (C-10), so callers must pass the log tail. Cases with no
  honest variant map to `StartFailure::Unknown` and keep the raw tail.
  - `StartFailure::PortInUse`: `Address already in use` (spike 6.1) only.
  - `StartFailure::PermissionDenied`: `Permission denied` / `permission denied`,
    including `Can't open or create append-only dir ...: Permission denied` (6.7).
  - `StartFailure::Unknown`: a generic bind failure such as
    `Could not create server TCP listening socket ...: Cannot assign requested address`
    plus `Failed listening on port ... aborting` (6.5, not a clash); every
    `FATAL CONFIG FILE ERROR` case (bad directive, wrong value type, port out of
    range, missing `dir`); `Can't open or create append-only dir ...: Not a directory`;
    and anything unrecognized.

## Host glibc detection (`engine::glibc`, C-03)

Valkey Linux binaries are not self-contained. `jammy` requires `GLIBC_2.34`,
`noble` requires `GLIBC_2.38`; a host in `[2.34, 2.38)` can run `jammy` but not
`noble`.

- `parse_ldd_version(&str) -> Option<Version>` parses real `ldd --version`
  output (Fedora `ldd (GNU libc) 2.43`, Ubuntu packaged forms, three-part
  versions) and returns `None` for musl/garbage.
- `valkey_variant(version) -> Option<ValkeyVariant>` picks `Noble` (>= 2.38),
  `Jammy` (>= 2.34), or `None`.
- `host_version()` / `host_valkey_variant()` are thin runners over
  `ldd --version`.

## Deviations from the spike

- **Health/auth challenge:** the spike states an auth challenge proves the server
  is up. The adapters treat PostgreSQL SQLSTATE `28xxx` and Valkey `NOAUTH` /
  `AuthenticationFailed` as `Health::Ready`. A connect-time credential rejection
  during `shutdown` (not health) is reported as an engine error, because the
  command could not be issued.
- **Honest diagnosis (review round 1, F1):** every recognized cause with no
  matching `StartFailure` variant now maps to `Unknown` and the raw tail stays
  with the caller, so the GUI shows the log lines instead of a wrong fix.
  Specifically, PostgreSQL unix-socket-path-too-long, a missing `.s.PGSQL.*`
  lock file, and `pg_ctl: directory "..." does not exist`; and Valkey generic
  bind failures (`Cannot assign requested address`), missing-`dir`
  config errors, and append-only `Not a directory`. `PortInUse` is reserved for
  real address-in-use text; `PermissionDenied` for real `Permission denied`.
- **Long home path (review round 1, F2):** PostgreSQL no longer fails creation
  when `<run>/sock/.s.PGSQL.<port>` exceeds 107 bytes. `choose_socket_dir`
  disables the unix socket instead and the server runs TCP-only. The
  `postgres_long_path_disables_unix_socket_and_stays_tcp_only` e2e test proves it
  with the real 18.7 binary: `pg_ctl start -w` returns, `select 1` works over
  TCP, no socket file exists, and stop/restart succeed.
- **PostgreSQL `initdb` locale:** `select_locale()` consults `locale -a` on the
  host; if unavailable it falls back to the preference chain which resolves to
  `C`, matching the spike's "fallback `C.utf8`, then `C`" intent. The runtime's
  bundled environment is not probed directly.

## MySQL (`engine::mysql`)

Spike refs: `docs/spikes/engines/mysql.md` (W3-B1, real 8.4.11 minimal tarball, Linux x86_64).

- **Runtime:** MySQL 8.4 LTS minimal tarball, `strip_prefix` `mysql-8.4.11-linux-glibc2.28-x86_64-minimal/`. Binaries used: `bin/mysqld`, `bin/mysqladmin`. `mysql` client is present in the tarball and used by e2e tests. glibc floor GLIBC_2.28, `RUNPATH=$ORIGIN/../lib/private`, 0 missing ldd deps on the spike host.

- **Initialize (`mysqld --initialize-insecure`):** runs `mysqld --no-defaults --initialize-insecure --datadir=<dir> --basedir=<runtime> --log-error=<logs>/init.err`. The initialize step creates `root@localhost` with an empty password; it does **not** create `root@127.0.0.1`. The adapter then writes `run/my.cnf` (0600), writes a one-shot `run/dblocal-init.sql` (0600), starts `mysqld --defaults-file=<my.cnf> --init-file=<init.sql>` foreground, waits for TCP readiness, issues `mysqladmin ... shutdown` (password via `MYSQL_PWD` env when `AuthMode::Password`, never argv), waits for exit bounded by `STOP_GRACE_SECS`, and deletes the init SQL file. On any failure the data directory and init SQL file are removed and raw output/log tail is returned.

- **Auth bootstrap (no password in argv/file left behind):** password mode init SQL runs `ALTER USER 'root'@'localhost' IDENTIFIED BY '...'`, creates/grants `root@127.0.0.1`, and `FLUSH PRIVILEGES`. No-password mode creates/grants `root@127.0.0.1` with empty password and keeps localhost as created by initialize-insecure. Passwords are SQL single-quote escaped before embedding in the one-shot init SQL; the init SQL is a `SecretString`-backed render and is never logged.

- **`my.cnf` (0600, atomic write):** `bind-address=127.0.0.1`, `port=<port>`, `datadir`, `socket=<resolved socket>`, `pid-file=<run>/mysqld.pid`, `log-error=<logs>/error.log`, `mysqlx=OFF`, `skip-name-resolve`, `basedir=<runtime>`. No `0.0.0.0`. `mysqlx=OFF` is required: without it MySQL 8.4 binds X Plugin on `*:33060` and a unix `mysqlx.sock`, which violates the single-loopback-port invariant (spike proved with ss).

- **Socket path limit and fallback:** MySQL has no "no unix socket" switch, so the adapter cannot disable the unix socket the way PostgreSQL does. It applies a platform `sun_path` limit: 107 bytes on Linux, 103 bytes on macOS/BSD. If `<run_dir>/mysqld-<port>.sock` fits, it is used unchanged. If it is too long, the adapter resolves a short fallback socket under a verified private directory candidate: `$XDG_RUNTIME_DIR/dblocal/` when set and usable, else `/tmp/dblocal-<euid>/`. Candidates are tried in order; a candidate is used only when it is a real 0700 directory owned by the current euid and the resulting `<candidate>/m<port>-<12hex>.sock` path fits the platform limit. The fallback socket file name is derived from the port and a short SHA-256 hash of the instance directory so two instances never collide. `initialize`, bootstrap and `spawn_command` read the same `my.cnf`, so they agree on the socket. `shutdown` and protocol health use TCP and are unaffected. `shutdown` removes a fallback socket file only when its parent is an allowed fallback directory, its name matches `m<port>-<12hex>.sock` for that port, and the path is a real unix socket; otherwise it warns and leaves it alone. The adapter returns `DblError::InvalidInput { field: "socket", ... }` only when no verified candidate can produce a fitting socket path. macOS behavior is implemented by platform limit/cfg but **unverified on macOS** in this workspace.

- **Start command:** `mysqld --defaults-file=<run>/my.cnf`. No password in argv or env.

- **Prepare on every start:** the fallback socket directory (`$XDG_RUNTIME_DIR/dblocal/` or `/tmp/dblocal-<euid>/`) and the per-instance `tmpdir` (`<run_dir>/tmp`) are volatile — `/run/user/<uid>` and `/tmp` are wiped at logout/reboot (macOS clears `/private/tmp` at boot) and a restored backup can lose `<run_dir>/tmp`. `MysqlAdapter::prepare_run` runs on every start from `instance/start.rs::prepare`: it re-reads `socket=` from `my.cnf`, recreates and re-verifies the fallback directory (0700, our euid, not a symlink) only when the socket lives under an allowed fallback candidate, deletes a stale socket file left by a crash (and refuses a non-socket regular file), recreates the private `tmpdir` 0700, and fails with a clear `Engine` error when it cannot. Without this an instance whose socket path needed the fallback starts once and then fails after the first reboot. macOS is **unverified on macOS** in this workspace.

- **Health check:** real TCP MySQL protocol handshake. The adapter reads the 4-byte packet header and payload; protocol v10 greeting => `Ready`; ERR packet 1045/1130/1251/1524 (or Access denied / Host not allowed messages) => `Ready` (server is up, auth challenge). Other ERR/empty/unknown => `NotReady`. `mysqladmin ping` exit code is not trusted (spike showed rc 0 with a host-not-allowed error).

- **Shutdown:** `mysqladmin --no-defaults -h 127.0.0.1 -P <port> -u root --protocol=TCP shutdown`, password via `MYSQL_PWD` env only when password auth is configured. Non-zero exit returns `DblError::Engine` with raw output. Spike confirmed SQL shutdown and SIGTERM both produce `Shutdown complete`; the adapter's normal path is SQL shutdown.

- **Connection URI:** `UriFormat::Uri` => `mysql://root:<percent-encoded>@127.0.0.1:<port>/`; `UriFormat::DotEnv` includes `DATABASE_URL`, `DB_HOST`, `DB_PORT`, `DB_DATABASE=` (empty), `DB_USERNAME`, `DB_PASSWORD`. No-password (`F-CRD-03`): `mysql://root@127.0.0.1:<port>/` and `.env` omits `DB_PASSWORD`.

- **Diagnosis mapping:** `StartFailure::PortInUse` for `Bind on TCP/IP port: Address already in use` / `Do you already have another mysqld server running on port`. `StartFailure::PermissionDenied` for `Permission denied` / `OS errno 13`. `StartFailure::BinaryMissing` for exec/ENOENT of mysqld/mysqladmin. Socket-path-too-long, bad config/value, missing/garbled ibdata1/datadir, DD init failures, and unrecognized cases map to `StartFailure::Unknown` with raw tail kept by caller.

- **Deviations / notes:** No `mysql_async` runtime dependency was added for health/shutdown; a small handshake reader keeps behavior explicit and auditable. `mysqladmin` is used for shutdown because it is present in the minimal tarball and gives a clean SQL shutdown path; password is passed through `MYSQL_PWD` env, not argv. Windows/macOS/aarch64/MySQL 9.x are unverified.
