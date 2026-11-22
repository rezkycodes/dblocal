# Engine adapters (W1-F)

This document records the concrete behavior implemented in `crates/core/src/engine/`
for PostgreSQL and Valkey. It follows the W0-B spike findings and the coordinator
decisions for W1-F. Spawning detached processes is owned by W1-D/W2-A; adapters
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

## MariaDB (`engine::mariadb`)

Spike refs: `docs/spikes/engines/mariadb.md` (W3-B2, 2026-11-22), MariaDB REST
API `downloads.mariadb.org/rest-api/mariadb/<version>/`. Verified LTS builds:
11.8.10 and 11.4.14, Linux x86_64 `linux-systemd` tarballs, SHA-256 matched the
REST API `checksum.sha256sum`.

- **Layout.** Top-level `mariadb-<ver>-linux-systemd-x86_64/` with `bin/`,
  `scripts/`, `lib/`, `share/`. Strip the top-level dir. `mariadbd` is the
  canonical server binary; `mysqld` is a compat name. `mariadb-install-db` lives
  in `scripts/` (shell script), not `bin/`.

- **Host deps.** `mariadbd` needs glibc >= 2.28 and links `libcrypt.so.1` on the
  probed build. Fedora 44 provides `libcrypt.so.2` (XCRYPT_2.0) which satisfies
  the required symbol, but the soname differs; a host without `libcrypt.so.1`
  will fail to exec with `error while loading shared libraries: libcrypt.so.1: ...`.
  The adapter diagnoses loader errors as `BinaryMissing` (raw text kept by caller).
  Packaging should preflight and document the host package: `libxcrypt-compat` on
  Fedora, `libcrypt1`/`libxcrypt-compat` as appropriate on Debian/Arch. Do not
  install packages automatically.

- **Initialize (`mariadb-install-db`):** runs
  `scripts/mariadb-install-db --datadir=<dir> --basedir=<runtime>
  --auth-root-authentication-method=normal --skip-test-db`.
  On failure the data dir is removed and raw output is returned. It writes the
  DBLocal-managed `my.cnf` (0600) with `bind-address=127.0.0.1`, `port`,
  `datadir`, `basedir`, a socket path, `pid-file`, `log-error`, `skip-name-resolve`.
  No password in config.

- **Socket path fallback (C-09 analogue).** AF_UNIX `sun_path` is 107 bytes on
  Linux and 103 bytes on macOS/BSD. The natural path is
  `<instance>/run/mariadb-<port>.sock`; when it exceeds the platform limit the
  adapter selects a short private fallback (`$XDG_RUNTIME_DIR/dblocal/` if set
  and short enough, else `/tmp/dblocal-<euid>/`, mode 0700, owned by current
  euid, not a symlink) with a deterministic file name derived from port and a
  short hash of the data directory. The chosen path is written into `my.cnf`, so
  initialize, bootstrap and start agree. macOS behavior is implemented but
  unverified on this Linux host.

- **Password bootstrap (`AuthMode::Password`).** After install-db, the adapter
  writes a 0600 one-shot init file containing `ALTER USER ... IDENTIFIED BY ...;
  FLUSH PRIVILEGES; SHUTDOWN;`, starts `mariadbd --defaults-file=... --init-file=...`,
  waits for it to exit within `START_TIMEOUT_SECS`, then deletes the init file
  on both success and failure. The SQL literal escapes backslashes, single quotes
  and control chars. The password never appears in argv, env, or logs; init-file
  contents are not logged. On failure the data dir is removed.

- **Start command:** `bin/mariadbd --defaults-file=<data_dir>/my.cnf` with
  stdin null. No password in argv/env; `MYSQL_PWD` and `MYSQL_TCP_PORT` are
  removed from the environment.

- **Health:** TCP connect to `127.0.0.1:<port>`, read the MySQL/MariaDB packet
  header + greeting payload, parse protocol version 10. A valid greeting
  (including MariaDB version string like `11.8.10-MariaDB`) means
  `Health::Ready`. A protocol error packet is treated as Ready because the
  server spoke the protocol. Connect/read failures are engine errors. No
  `mysqladmin ping` exit-code trust.

- **Shutdown:** read `data_dir/mariadbd.pid`, verify the PID identity before
  signalling (Linux: `/proc/<pid>/exe` resolved to the runtime `mariadbd` and
  cmdline contains this instance's `--datadir` or `--defaults-file`; macOS/BSD:
  `ps -p <pid> -o command=` best-effort check, documented as unverified), then
  send `SIGTERM` and wait up to `STOP_GRACE_SECS` for the PID to disappear.
  Liveness uses `kill(pid,0)` and treats Linux zombie state (`Z`) as dead. A
  stale/unrelated pid is never signalled; shutdown returns `Ok(())` with a log
  line. The bounded wait returns a clear engine error on timeout so the service
  can escalate.

- **Connection values:**
  - `UriFormat::Uri`: `mysql://<user>:<enc_pw>@127.0.0.1:<port>/`
  - `UriFormat::DotEnv`: `DATABASE_URL`, `DB_HOST`, `DB_PORT`, `DB_DATABASE=`
    (empty, MariaDB/MySQL has no mandatory default db in URI), `DB_USERNAME`,
    `DB_PASSWORD` when present. Uses `uri.rs` percent-encoding.

- **Diagnosis (substring markers, raw tail kept by caller):**
  - `PortInUse`: `Address already in use`, `Can't start server: Bind on TCP/IP
    port`, `Do you already have another server running on port`.
  - `DataDirVersionMismatch`: explicit incompatible-data markers, including
    MySQL 8.4 datadir opened by MariaDB (`ib_logfile0 was not found` +
    `Unknown/unsupported storage engine: InnoDB` / `Plugin 'InnoDB' registration
    as a STORAGE ENGINE failed`). Cross-major MariaDB dirs that InnoDB upgrades
    silently are not misclassified.
  - `PermissionDenied`: real `Permission denied`.
  - `BinaryMissing`: exec `Exec format error` or missing mariadbd/install-db.
  - `Unknown`: socket path too long, bad config/unknown variable, missing data
    dir / `Can't change dir to`, disk full, and anything unrecognized.

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
- **MariaDB cross-major dirs (W3-B2 spike):** opening a MariaDB 11.4 datadir
  with 11.8 and the reverse both reached "ready for connections" on the probed
  builds (InnoDB tolerated the format). The adapter therefore only reports
  `DataDirVersionMismatch` for explicit incompatibility markers (including the
  real MySQL 8.4-on-MariaDB InnoDB failure), never for a silent/ambiguous case.
- **MariaDB host libcrypt (W3-B2 spike):** the 11.8.10/11.4.14 Linux tarballs
  link `libcrypt.so.1`; Fedora 44 ships `libcrypt.so.2` (XCRYPT_2.0) with the
  required symbol but a different soname. The adapter does not bundle or shim the
  library; exec/loader failures surface through the normal start-failure path.
