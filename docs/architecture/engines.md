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
  - The unix socket directory `<run_dir>/sock` is created `0700` and the full
    path `<dir>/.s.PGSQL.<port>` is validated to be <= 107 bytes at init time
    (C-09). `validate_socket_path_len` is also available to callers at start time.

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

- **Diagnosis mapping (substring markers):**
  - `StartFailure::PortInUse`: `could not bind IPv4 address`,
    `Is another postmaster already running on port`,
    `could not create any TCP/IP sockets` with `Address already in use`.
  - `StartFailure::DataDirVersionMismatch`: `database files are incompatible with server`,
    `initialized by PostgreSQL version`.
  - `StartFailure::PermissionDenied`: `Permission denied`, unix socket path too
    long / missing `.s.PGSQL.*` lock file (`No such file or directory`).
  - `StartFailure::BinaryMissing`: `pg_ctl: directory "..." does not exist`,
    exec `No such file or directory`, `Exec format error`.
  - `StartFailure::Unknown`: anything else (raw tail kept by caller).

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
  only to `logfile` (C-10), so callers must pass the log tail.
  - `StartFailure::PortInUse`: `Address already in use`,
    `Failed listening on port`, `Could not create server TCP listening socket`.
  - `StartFailure::PermissionDenied`: `Can't open or create append-only dir`,
    `Permission denied`, `FATAL CONFIG FILE ERROR` with `No such file or directory`
    (e.g. missing `dir`), other `No such file or directory`.
  - `StartFailure::Unknown`: other `FATAL CONFIG FILE ERROR` cases (bad directive,
    wrong value type, port out of range) and anything unrecognized. The raw tail
    is preserved by the caller.

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
- **Valkey diagnosis (unavailable listen address):** a bind failure caused by
  `Cannot assign requested address` shares the `Failed listening on port` /
  socket-creation markers and is classified as `StartFailure::PortInUse`
  (cannot listen on the requested endpoint). The raw tail distinguishes the
  reason.
- **Socket-dir / long-path diagnosis:** the spike records these as
  "socket-dir/configuration issue" (README 3.1/3.3). `StartFailure` has no
  dedicated socket-dir variant, so recognized socket-dir problems (too-long unix
  socket path, missing `.s.PGSQL.*` lock/dir) are classified as
  `PermissionDenied` rather than `Unknown`, keeping them actionable. The raw
  log tail carries the precise cause.
- **PostgreSQL `initdb` locale:** `select_locale()` consults `locale -a` on the
  host; if unavailable it falls back to the preference chain which resolves to
  `C`, matching the spike's "fallback `C.utf8`, then `C`" intent. The runtime's
  bundled environment is not probed directly.
