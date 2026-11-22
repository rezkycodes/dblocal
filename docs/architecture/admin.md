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
