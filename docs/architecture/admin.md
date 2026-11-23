# Database and user administration (W3-A)

This document records the library built in `crates/core/src/admin/**` for PRD
`F-DBU-01..03`: listing, creating and dropping databases, creating users with a
privilege level, and resetting the superuser password with the F-DBU-03 AC2
rollback guarantee. It is a **library over a connection description only**;
wiring it into `DblService`, the instance service and the GUI is a later task.

Code layout:

| File | Layer |
|---|---|
| `admin/sql.rs` (+ `admin/sql/tests.rs`) | Pure SQL builders, identifier validation, quoting, hostile corpus |
| `admin/postgres.rs` (+ `admin/postgres/tests.rs`) | `tokio-postgres` execution, the F-DBU-03 AC2 reset state machine |
| `admin/error.rs` | Module-local `AdminError` with `DBL-8100..=8199` codes |
| `admin/messages.rs` | Indonesian user messages, one static string per variant |

## Invariants honoured

- Loopback only: `AdminTarget::new` rejects any host that is not `127.0.0.1`,
  `::1` or `localhost` (`AdminError::NonLoopbackHost`, `DBL-8102`). `localhost`
  is dialled as `127.0.0.1` so no resolver can redirect the connection.
- No raw string concatenation with unvalidated input. Identifiers are wrapped in
  `Identifier`, whose only constructors enforce `[A-Za-z][A-Za-z0-9_]*` up to 63
  bytes and reject `postgres`, `template0`, `template1`; quoting is then
  mechanical and tested. Values that the wire protocol accepts as parameters
  (a database name in the read queries) are bound, not interpolated.
- No secret in an error, a log or `Debug`/`Display`. `AdminError` carries no
  free-form string; its only text payload is a validated five-character SQLSTATE.
  The canary test formats every variant and scans for a password and a
  connection string.
- Immutability: `AdminTarget` has private fields and a `with_timeouts`/`with_password`
  copy builder; the domain records (`DatabaseInfo`, `UserInfo`) expose getters
  only.

## Identifier validation

`Identifier::new` is for **databases and schema objects**; `Identifier::role` is
for **roles**. The two share the allowlist (empty, > 63 bytes, leading digit,
any byte outside `[A-Za-z0-9_]` are all rejected with a specific
`IdentifierReject`), but only `new` applies the reserved-name list. `postgres` is
the default superuser role and must be addressable by `ALTER ROLE postgres`, so
`role` deliberately skips that list.

Because the allowlist already excludes quotes, backticks, semicolons, comment
markers, unicode and NUL bytes, `quote_identifier` can only ever receive a
harmless value. The quoting layer is nevertheless proven **independently** of
the allowlist: `admin/sql/tests.rs` feeds raw hostile strings straight to the
quoting functions (bypassing `Identifier`) and parses the result with a tiny
tokenizer that understands `--` and `/* */` comments, `"…"`/`` `…` `` quoted
identifiers and `'…'`/`E'…'` literals. Every case must tokenize to exactly one
statement with the original text.

## Quoting

- PostgreSQL identifier: `"name"`, with `"` doubled.
- MySQL identifier: `` `name` ``, with `` ` `` doubled.
- PostgreSQL literal: `E'…'` with `'` doubled and `\` doubled, so the value is
  unambiguous regardless of `standard_conforming_strings`; a NUL byte becomes
  `\x00`. Used only for `CREATE ROLE … PASSWORD` / `ALTER ROLE … PASSWORD`,
  which the grammar does not accept as a bound parameter.
- MySQL literal: single-quoted with the usual backslash escapes.

## The exact SQL per privilege level (PostgreSQL)

`CREATE DATABASE "name" WITH ENCODING 'UTF8' TEMPLATE template0` — `template0` so
the requested encoding is honoured even when `template1` was customised.

Passwords are **never** sent as cleartext in `CREATE ROLE` / `ALTER ROLE`.
PostgreSQL logs the text of a failing statement by default
(`log_min_error_statement = error`), and `logs/postgres.log` is the file surfaced
by `dblocal logs` and the Logs tab. Instead the client computes a SCRAM-SHA-256
verifier with `postgres_protocol::password::scram_sha_256` and sends
`PASSWORD 'SCRAM-SHA-256$4096:...'` as a plain literal (not `E'...'`). The
server stores a verifier either way, so authentication is unchanged. Every admin
connection also sets `-c log_min_error_statement=panic -c log_statement=none -c
log_min_duration_statement=-1` as belt-and-braces (the admin user is superuser).

MySQL/MariaDB note (adapter not implemented here): enabling the general query log
records `CREATE USER ... IDENTIFIED BY '<cleartext>'`. The later MySQL adapter
must send a precomputed hash (`IDENTIFIED WITH caching_sha2_password AS
'<hash>'` or the engine-equivalent form) rather than cleartext; do not log or
persist the cleartext.

Read-only, run against the target database:
```sql
GRANT CONNECT ON DATABASE "<db>" TO "<user>";
GRANT USAGE ON SCHEMA public TO "<user>";
GRANT SELECT ON ALL TABLES IN SCHEMA public TO "<user>";
GRANT SELECT ON ALL SEQUENCES IN SCHEMA public TO "<user>";
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT ON TABLES TO "<user>";
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT ON SEQUENCES TO "<user>";
```

Read-write adds:
```sql
GRANT INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO "<user>";
GRANT USAGE, UPDATE ON ALL SEQUENCES IN SCHEMA public TO "<user>";
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT INSERT, UPDATE, DELETE ON TABLES TO "<user>";
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT USAGE, UPDATE ON SEQUENCES TO "<user>";
```

`revoke_privileges` is the mirror image (`REVOKE ALL …`) across database, schema,
tables, sequences and the default privileges.

The MySQL/MariaDB builders are implemented and unit-tested (`GRANT SELECT …` /
`GRANT SELECT, INSERT, UPDATE, DELETE … ON `<db>`.*`) but **not executed** here;
the MySQL adapter lands in a later task.

## Listing

`list_databases` returns name, owner and size, excluding templates:
```sql
SELECT d.datname::text, pg_get_userbyid(d.datdba)::text,
       pg_database_size(d.datname)::bigint
FROM pg_database d
WHERE d.datistemplate = false AND d.datallowconn = true
ORDER BY d.datname;
```
`list_users` returns `rolname, rolcanlogin, rolsuper` for `rolcanlogin` roles.

## Dropping a database

`drop_database(db, force)`:
- `force == false` (the default): `database_connection_count`: `SELECT count(*)
  FROM pg_stat_activity WHERE datname = $1` (name **bound**). A non-zero count is
  `AdminError::DatabaseInUse` (`DBL-8106`) and nothing is dropped. This is the
  typed error the task requires, and it means the caller never sees a raw
  `55006`.
- `force == true`: `DROP DATABASE "<db>" WITH (FORCE)`. The switch is explicit so
  terminating other sessions is never accidental.

The SQL statement is built by the pure `drop_database_postgres(db, force)` builder
(unit-tested for both forms), so the `force` decision is pinned by an exact-SQL
test independent of the pre-check.

## Runtime contract (F3)

`PgAdmin::with_client` is safe to call from inside an existing tokio runtime. It
runs the connection work on a dedicated OS thread (`std::thread::scope` for
current-thread runtimes, `tokio::task::block_in_place` for multi-thread
runtimes) and builds a throwaway current-thread runtime there. Callers never
need to worry about "Cannot start a runtime from within a runtime".

## Password reset and the F-DBU-03 AC2 guarantee

`PgAdmin::reset_superuser_password(sink)` runs the pure state machine
`reset_with`, which is unit-tested with injected doubles so every failure point
is covered without a server:

```
generate new_password
  → engine.apply(old_password, new_password)   # connect as old, set new
  → engine.verify(new_password)                # fresh connection must authenticate
  → sink.store(new_password)                   # keychain (injected later)
```

- If `apply` fails, nothing changed; the error is returned and no rollback runs.
- If `verify` fails, the engine is rolled back to the old password and the error
  is `PasswordVerify` (`DBL-8112`).
- If the sink fails, the engine is rolled back and the error is
  `CredentialStore` (`DBL-8113`).
- If the rollback itself fails, the error is `RollbackFailed` (`DBL-8114`) and
  says the state is inconsistent. It carries only `RollbackState { engine_changed,
  sink_failed, rollback_failed }` — never a secret.

A bug the real-engine e2e found: the rollback first connected with the **old**
password, but the engine now held the **new** one, so the rollback could not
authenticate and always reported `RollbackFailed`. `PasswordEngine::apply` now
takes the connect credential separately from the target password, so the
rollback connects with the new password to restore the old one. The e2e
`reset_password_rolls_back_when_the_sink_fails` proves the old password still
works afterwards.

The two trait seams are `PasswordEngine` (apply/verify) and `CredentialSink`
(store). Both have real implementations selected by the later wiring: `PgAdmin`
itself for `PasswordEngine`, the keychain for `CredentialSink`.

## Error codes

`DBL-8100..=8199`, all in `admin/error.rs`. A table test asserts every code is
inside the range, unique, and reached by a variant.

| Code | Variant | Meaning |
|---|---|---|
| 8100 | `Connect` | engine unreachable (TCP/timeout, not auth) |
| 8116 | `AuthFailed` | stored credential refused (`28P01`/`28000`) |
| 8101 | `InvalidIdentifier` | name failed the allowlist |
| 8102 | `NonLoopbackHost` | host is not loopback |
| 8103 | `InvalidTarget` | empty host/superuser, zero port |
| 8104 | `DatabaseExists` | `42P04` |
| 8105 | `DatabaseNotFound` | `3D000` |
| 8106 | `DatabaseInUse` | `55006` / live connections |
| 8107 | `UserExists` | `42710` |
| 8108 | `UserNotFound` | role missing on drop/revoke/grant (`42704`) |
| 8109 | `InsufficientPrivilege` | `42501` |
| 8110 | `QueryFailed` | any other SQLSTATE |
| 8111 | `Unsupported` | engine lacks the operation |
| 8112 | `PasswordVerify` | new password did not authenticate |
| 8113 | `CredentialStore` | sink refused the password |
| 8114 | `RollbackFailed` | rollback failed, state inconsistent |
| 8115 | `PasswordGenerate` | OS RNG failed |
| 8117 | `NoPassword` | the instance was created without a password; admin needs one created with a password (W4-A fix 1) |
| 8118 | `SystemSchema` | the database is one MySQL/MariaDB own (`mysql`, `sys`, `information_schema`, `performance_schema`); admin must not manage it (W4-A2) |

`NoPassword` is deliberately distinct from `Unsupported` (8111): the engine is
PostgreSQL (supported), but an instance created with `--no-password` has no
credential to authenticate an admin session with, so the fix is a different
instance, not a later adapter. `map_connect_error` also maps SQLSTATE `3D000`
(unknown database) to `DatabaseNotFound` (8105), so the grant step's connect to a
database dropped mid-operation is a missing-database answer, not "server
unreachable".

## Proposed `DblService` surface (for the wiring task)

Additive methods with default `NotImplemented` bodies so no existing implementor
breaks:

```rust
#[async_trait_or_blocking]  // final shape decided by the wiring task
pub trait DblService {
    // ... existing methods ...

    fn list_databases(&self, id: &InstanceId) -> Result<Vec<DatabaseInfo>, UserFacingError> {
        Err(UserFacingError::new(CODE_NOT_IMPLEMENTED, strings::ERR_NOT_IMPLEMENTED))
    }
    fn create_database(&self, id: &InstanceId, name: &str) -> Result<(), UserFacingError> {
        Err(UserFacingError::new(CODE_NOT_IMPLEMENTED, strings::ERR_NOT_IMPLEMENTED))
    }
    fn drop_database(&self, id: &InstanceId, name: &str, force: bool) -> Result<(), UserFacingError> { /* … */ }
    fn create_user(&self, id: &InstanceId, name: &str, db: &str, level: PrivilegeLevel)
        -> Result<SecretString, UserFacingError> { /* … */ }
    fn reset_superuser_password(&self, id: &InstanceId) -> Result<SecretString, UserFacingError> { /* … */ }
}
```

The real implementation reads the superuser credential from `SecretStore`, builds
an `AdminTarget`, and converts `AdminError` through `UserFacingError::from`
(`impl From<AdminError> for UserFacingError`). The `CredentialSink` it injects is
a thin adapter over `SecretStore::set`.

## Valkey

Valkey has no databases/users in this sense: it has numbered logical databases
(`SELECT n`) and an ACL system, not a named database and a login role with
table privileges. This task proposes **no admin ops for Valkey v1**; the
Databases tab shows nothing for a Valkey instance. This matches the PRD's
`admin_ops()` comment ("None untuk Valkey v1").

## W3-A fix round 1 findings

### F1 (HIGH) — cleartext password in engine log on statement failure

- **Finding.** A failing `CREATE ROLE`/`ALTER ROLE` caused PostgreSQL to log the
  statement text at `log_min_error_statement = error` (default). The reviewer
  probe saw `STATEMENT: CREATE ROLE "leaky" LOGIN PASSWORD E'<cleartext>'` in
  `logs/postgres.log`, which is the file surfaced by `dblocal logs` and the Logs
  tab. This violated "passwords never appear in logs" for any failure after parse
  (duplicate role, timeout, missing role, failed reset rollback).
- **Fix.** The client computes the SCRAM verifier with
  `postgres_protocol::password::scram_sha_256(password.as_bytes())` and sends
  `PASSWORD E'<SCRAM-SHA-256$4096:...>'` via `quote_postgres_literal`. The server
  stores a verifier either way, so authentication is unchanged. Belt and braces:
  every admin connection sets `-c log_min_error_statement=panic -c
  log_statement=none -c log_min_duration_statement=-1`.
- **Tests.** Real-cluster e2e creates a role with a known canary password,
  triggers (a) duplicate create, (b) failing ALTER on a missing role, (c) sink-
  failure reset rollback, then reads `logs/postgres.log` and asserts the canary
  is absent and no `PASSWORD E'` appears; it also proves the role still
  authenticates and reset e2e still passes. Mutation: cleartext literal restored
  -> red; log options removed alone -> the log-absence test stays green because
  SCRAM already prevents the cleartext from being in the statement (reported
  honestly in the worker report).
- **MySQL note.** If the MySQL general query log is enabled it records
  `CREATE USER ... IDENTIFIED BY '<cleartext>'`. The later MySQL adapter must
  send a precomputed hash (`IDENTIFIED WITH ... AS '<hash>'`); do not implement
  it now.

### F2 (MEDIUM) — wrong password vs stopped engine

Connect failures carrying SQLSTATE `28P01`/`28000` map to `AuthFailed`
(DBL-8116) with its own Indonesian message; timeouts/refused connections remain
`Connect` (DBL-8100). Neither error's Debug/Display contains the password.

### F3 (MEDIUM) — admin API panics inside a tokio runtime

`with_client` runs on a dedicated OS thread and builds a throwaway current-thread
runtime there (`block_in_place` on multi-thread runtimes, `std::thread::scope`
on current-thread). Safe from `#[tokio::test]` (current-thread and multi-thread)
and from `spawn_blocking`.

### F4 (LOW) — `UserNotFound`

`42704` from role DDL/DCL call sites maps to `UserNotFound` (DBL-8108); the
mapping is scoped to role statements so other `42704`s stay `QueryFailed`.

### F5 (LOW) — force pinned by pure builder

`drop_database_postgres(db, force)` builds the exact SQL; unit tests pin `false`
and `true`. The e2e proves a live session blocks `force=false` with
`DatabaseInUse` and is terminated by `force=true`.

### F6 (LOW) — hostile names proved at the server boundary

The hostile e2e enables `log_statement = 'all'` and asserts the log contains no
hostile fragment and no `CREATE/DROP/GRANT/ALTER` after setup, across every
public method that takes a name.

## Tests and coverage

- The pure layer (`admin/sql.rs`, `admin/error.rs`) is **≥ 90 %** covered by unit
  tests including the hostile corpus and the tokenizer proof.
- `admin/postgres.rs` is **≥ 80 %** covered by unit tests (state machine with
  injected failures, error mapping, target validation) plus the ignored e2e.
- `crates/core/tests/admin_e2e.rs` (8 `#[ignore]`d tests) starts a real
  PostgreSQL from the cached catalog archive in a temp home on a port in
  20000-29999, with a `/proc` guard that kills any `postgres` whose executable
  lives in that home — even on panic. Because it starts a real engine, every run
  holds the machine-wide lock (Common Rules 12):
  `DBLOCAL_E2E=1 flock -w 3600 "$HOME/.cache/dblocal-e2e.lock" cargo test -p
  dblocal-core --test admin_e2e -- --ignored --test-threads=1`. Cases: create is
  UTF8 with a size; read-only cannot write; read-write can; drop refuses an open
  connection and succeeds after; hostile names change nothing; reset succeeds and
  the old password stops working; a failing sink rolls back and the old password
  still works; list/exists/revoke/drop plus the duplicate-role and missing-database
  mappings and the closed-port `Connect` path.
- Mutation checks (each reverted, the red output pasted in the worker report):
  weaken the allowlist, drop PostgreSQL identifier quoting, remove the rollback,
  grant write on a read-only level.

## What this does not prove

- **MySQL / MariaDB execution.** The builders exist and are unit-tested; no
  engine runs them yet (the adapter is a later task). The e2e proves PostgreSQL
  only.
- **macOS and Windows.** `tests/admin_e2e.rs` is Linux/x86_64 (`/proc`, `tar`,
  the catalog archive name); it compiles elsewhere but is not run there.
- **A live OS keychain.** `CredentialSink` is injected; the e2e uses recording
  and failing sinks, and the real keychain adapter belongs to the wiring task.
  `reset_password_succeeds_and_the_new_password_works` proves the engine half
  with a recording sink, not the keychain.
- **MySQL privilege semantics** (`GRANT … ON db.*`) are asserted as text, not
  against a running MySQL.

## W4-A2 — MySQL and MariaDB admin (F-DBU-01..03 for the MySQL family)

The same `DblService` calls now work for MySQL and MariaDB through
`admin/mysql.rs` (`MysqlTarget` + `MysqlAdmin`), which implements the
`admin/executor.rs::AdminExecutor` trait. The F-DBU-03 AC2 reset state machine
was extracted into `admin/reset.rs` and is shared by `PgAdmin` and `MysqlAdmin`.

### Client library

`mysql_async` 0.37 (MIT OR Apache-2.0). It was already a workspace dependency
(health checks and the e2e client), resolves to rustls + `flate2` with no
`openssl` / `native-tls` in the lock (`cargo tree` shows none), and its default
feature set needs no system C library. Kept out of scope: any TLS feature.

### Connection: the unix socket on unix, loopback TCP on Windows (W4-A3)

One pure decision, `admin/mysql.rs::Transport::for_platform(unix, socket, port)`,
picks the transport exactly once at construction and never re-decides at runtime:

| Platform | Arm | `Opts` the driver gets |
|---|---|---|
| unix (Linux, macOS) | `Transport::Socket(path)` | `socket=<path>`, `prefer_socket(true)` — unchanged from W4-A2 |
| non-unix (Windows) | `Transport::Loopback { port }` | `ip_or_hostname("127.0.0.1")`, `tcp_port=<instance port>`, **no** socket, `prefer_socket(false)` |

Both arms route the **same** superuser and password; only the transport differs.
There is no runtime fallback between them: a unix host is never moved onto TCP by
a missing socket, and a Windows host is never moved onto a named pipe (see the
driver fact below).

#### unix arm (the original guarantee, unchanged)

Every admin connection dials the instance's **unix socket** (`my.cnf`'s
`socket=`; the adapters' `configured_socket_path` returns the same path the
start command wrote, including the long-path fallback). Reasons:

- The socket is a secure (never-transmitted) channel, so no byte — the password
  or a statement's text — crosses the loopback TCP stream. (`mysql_async` would
  itself do the `caching_sha2_password` RSA public-key exchange over TCP, so the
  socket is not *required* to complete the handshake — but it is the only path
  that keeps the traffic off TCP.)
- MariaDB's bootstrap (`--auth-root-authentication-method=normal`) configures
  `root@localhost`, reachable only through the socket.
- Tests pin it: `transport_on_unix_is_the_socket_and_never_loopback_even_with_a_port`
  feeds a port and an empty path and still gets `Socket`.

#### Windows arm (W4-A3)

Windows has no unix socket: `mysqld`/`mariadbd` do not create one, and
`mysql_async` 0.37.1's `Conn::new` (`src/conn/mod.rs:1029-1034`) returns
`DriverError::NamedPipesDisabled` for any `Opts` that carries a socket path on a
non-unix target. Before W4-A3 that made every `list_databases`, `create_database`,
`drop_database`, `create_user` and `reset_superuser_password` fail with
`DBL-8100` on Windows. Admin now dials `127.0.0.1:<instance port>`:

- The host is the literal `127.0.0.1` (`MANAGED_ACCOUNT_HOST`), equal to the host
  the created accounts live at (`user@'127.0.0.1'`), and never `localhost` — so no
  resolver is consulted and the account host and the transport host cannot drift
  (a test asserts `opts.ip_or_hostname() == MANAGED_ACCOUNT_HOST`).
- `prefer_socket(false)`: with it on, `mysql_async`'s `read_settings` loads
  `@@socket` and `reconnect_via_socket_if_needed` would retry over a path it
  learned from the server. Off, the connection stays on the TCP stream it opened.
- The engine listens only on loopback: both adapters render
  `bind-address=127.0.0.1` into their config (`engine/mysql` `BIND`,
  `engine/mariadb::render_config`), never `0.0.0.0`, and both have a test that
  fails if that line changes.
- Password transport: `mysql_async` completes `caching_sha2_password` with its own
  RSA public-key exchange over the plain TCP stream (it requests the server key,
  XORs the password with the nonce and encrypts it). MariaDB's bootstrap sets the
  password with a plain `ALTER USER … IDENTIFIED BY`, which leaves the default
  plugin for that account; `mysql_async` also accepts a `mysql_native_password`
  switch. Either way the cleartext never crosses the wire.

#### Residual risk of loopback TCP (Windows only)

Any loopback TCP admin path has one hazard the unix socket does not: between the
port check and the engine's bind, a **rogue process** could be listening on
`127.0.0.1:<port>`, and admin would hand it the superuser password. Mitigations
and limits:

- The adapters' recovery path opens no listening window of their own; the engine
  binds loopback and the port is checked free before a start, but the check and
  the bind are not atomic.
- Admin only behaves this way after the caller has loaded a `Running` instance
  row; a stopped or failed instance's port is not dialled by an admin call in
  normal use, and a failed start ends `failed` with the port released.
- The existing identity gate (`process::identity::verify_pid_identity`, used by
  stop/delete) validates a PID's executable and start time **before signalling**
  a process; there is **no** equivalent pre-dial identity check for the admin
  connection today, and this task did not invent one. The core invariant it
  protects — never signal an unidentified process — still holds, because admin
  never signals anything.
- If a hostile local process can race the bind, it already shares the user's
  session and can read the same `~/.dblocal` data; loopback TCP does not widen
  that boundary beyond the port window itself. This is recorded so a future
  hardening task can add a post-connect server identity check (e.g. a
  `SELECT @@version_comment` / handshake fingerprint) if the owner wants it.

Managed accounts are created at `user@'127.0.0.1'` (the TCP account the service
URI uses), not `localhost`, so a client opened with the URI can log in.


### Exact SQL per call

| Call | Statement |
|---|---|
| list databases | `SELECT s.SCHEMA_NAME, COALESCE(SUM(t.DATA_LENGTH + t.INDEX_LENGTH),0) FROM information_schema.SCHEMATA s LEFT JOIN information_schema.TABLES t ON t.TABLE_SCHEMA = s.SCHEMA_NAME WHERE s.SCHEMA_NAME NOT IN ('mysql','sys','information_schema','performance_schema') GROUP BY s.SCHEMA_NAME ORDER BY s.SCHEMA_NAME` |
| create database | ``CREATE DATABASE `name` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci`` |
| drop database (`force != true`, default) | `SELECT COUNT(*) FROM information_schema.PROCESSLIST WHERE DB = ?`; if > 0, `DBL-8106`, else ``DROP DATABASE `name``` |
| drop database (`force`) | ``DROP DATABASE `name``` (no pre-check) |
| create user | ``CREATE USER `u`@'127.0.0.1' IDENTIFIED BY '<escaped>'`` |
| grant `ro` | ``GRANT SELECT ON `db`.* TO `u`@'127.0.0.1'`` |
| grant `rw` | ``GRANT SELECT, INSERT, UPDATE, DELETE ON `db`.* TO `u`@'127.0.0.1'`` |
| drop user | ``DROP USER `u`@'127.0.0.1'`` |
| reset password | ``ALTER USER `root`@'<host>' IDENTIFIED BY '<escaped>'`` for `localhost`, `127.0.0.1`, `::1` |

Constraints:

- The privilege list is **explicit and `ON `db`.*` only** — never `*.*`, never
  `ALL`, never `WITH GRANT OPTION`, so a managed user cannot touch another schema
  or grant rights. Pinned by an exact-SQL test.
- System schemas (`mysql`, `sys`, `information_schema`, `performance_schema`,
  case-insensitive) are refused by the executor with `SystemSchema` (`DBL-8118`)
  before a statement is sent.
- Identifiers go through the existing `Identifier` allowlist and the backtick
  quoter; the password is a [`quote_mysql_literal`] single literal, so a hostile
  value cannot become a second statement (hostile-corpus tests).

### Not leaking the password to the query log

Enabling MySQL's general query log records `CREATE USER ... IDENTIFIED BY
'<cleartext>'`. Every admin statement that carries a password literal runs with
`SET sql_log_off = 1` on the session first, so its text never reaches the general
log. That variable needs `SYSTEM_VARIABLES_ADMIN`/super, which the admin
connection is; the set is best-effort (the socket already avoids a network
observer) and its failure does not abort the statement.

### Reset (F-DBU-03 AC2) for MySQL/MariaDB

The shared `reset_with` runs `apply → verify → store`, rolling back to the old
password on a verify or sink failure. `apply` sets the new password on every
loopback host account of the superuser (`localhost`, `127.0.0.1`, `::1`);
`verify` opens a fresh connection with the new password — over the socket on
unix, over loopback TCP on Windows — so the reset is only reported as successful
after the transport admin actually uses has authenticated with it. A failed reset
leaves the old password working and the keychain untouched (service-level test
with a failing sink).

### Error codes

MySQL server errors map without carrying the driver text: `1045` →
`AuthFailed` (8116), `1049` → `DatabaseNotFound` (8105), `1007` →
`DatabaseExists` (8104), `1008` → `DatabaseNotFound`, `1044`/`1142` →
`InsufficientPrivilege` (8109), `1396` → `UserExists` (8107) or `UserNotFound`
(8108) by the "doesn't exist" text; anything else is `QueryFailed` (8110) with no
payload. System schemas are `DBL-8118`.

### Valkey and no-password

`executor_for` accepts PostgreSQL, MySQL and MariaDB (a typed `NoPassword`
(8117) for any of them created without a password) and rejects only Valkey with
`Unsupported` (8111), which has no named databases or login roles.

### Tests and what is not proven

- Unit: exact-SQL and hostile-corpus builder tests, `MysqlAdmin` target/opts/
  system-schema/error-mapping, the shared reset state machine.
- Real-engine e2e (`tests/support/cases/admin_mysql.rs`, run under the e2e lock,
  MariaDB via the `libcrypt.so.1` shim): create with the `utf8mb4_unicode_ci`
  charset asserted via `information_schema`, list, refuse every system schema, ro
  SELECT vs INSERT, rw INSERT vs CREATE DATABASE, reset (old fails, new works,
  URI carries it), drop, clean PurgeData delete with no surviving engine.
- Not proven here: macOS/Windows, aarch64, MariaDB 11.4 (needs `libcrypt.so.1`,
  absent on this host), MySQL 8.0/9.x, a live OS keychain, and a reset with a
  live keychain failure against a real server (the sink-failure rollback is
  proven at the service level with the shared state machine).

## W4-A3 — the Windows transport

W4-A3 added the loopback TCP arm described under "Connection" above. What changed
in code:

- `admin/mysql.rs::Transport` (pure): `Socket(PathBuf)` or `Loopback { port }`,
  chosen by `Transport::for_platform(unix, socket, port)`. `MysqlTarget::new`
  takes the instance port again and derives the arm from `cfg!(unix)`;
  `MysqlTarget::opts` maps the arm to `Opts` and nothing else chooses a
  transport. There is no runtime fallback.
- `instance/admin_ops.rs::mysql_admin` passes `instance.port()` and still
  resolves `configured_socket_path` first — the socket is the unix arm and the
  long-path fallback it already applies is unaffected.

Proven on Linux (the unix host is where every commit and gate runs):

- `Transport::for_platform` both arms, including "unix + a port available is
  still the socket" and "unix + an empty socket path is still not loopback".
- The loopback `Opts`: socket absent, `prefer_socket` off, host the literal
  `127.0.0.1` equal to `MANAGED_ACCOUNT_HOST`, port the instance port.
- A real connection attempt to a closed `127.0.0.1:<port>` maps to `Connect`
  (`DBL-8100`) without a socket configured.
- The Linux e2e (`admin_mysql`) still passes on real MySQL 8.4.11 and MariaDB
  11.8.10 under the e2e lock, which is the unix arm end to end.
- Type-check for a non-unix target: `docs/spikes/admin/mysql-admin-tcp/probe`
  compiles the real `admin/mysql.rs` against real `mysql_async` 0.37.1 for
  `x86_64-pc-windows-gnu` (the workspace cross-check stops earlier in
  `aws-lc-sys`/`libz-sys`/`libsqlite3-sys`/`liblzma-sys`, which need a mingw C
  compiler this host does not have).

**Not proven:** any Windows run at all. No `mysqld`/`mariadbd` on Windows was
started, the `caching_sha2_password` RSA exchange was not observed against a real
Windows server, and the engine's own `my.ini` on Windows was not inspected.
`docs/dev/windows-verification.md` carries the manual rows.
