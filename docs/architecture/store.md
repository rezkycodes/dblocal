# W1-A SQLite store

Owned by W1-A. Synchronous persistence for instances, installed runtimes,
archived data, credential pointers and settings. The service layer (W2-A) is
the only intended caller; it must run store calls on `spawn_blocking`.

## Entry points

- `Store::open(&DblPaths)` creates the DBLocal root with mode `0700` on Unix,
  opens `state.db`, applies `journal_mode=WAL`, `foreign_keys=ON`,
  `synchronous=NORMAL`, and `busy_timeout=5000 ms`, then runs migrations.
- `Store::open_in_memory()` builds a fully migrated, empty database for tests.

## Schema (v1)

`PRAGMA user_version` is the schema version (latest: 1). Migrations run in order,
each step in one transaction together with its version bump. A database newer
than the binary is refused with `DblError::Storage` (code `DBL-3003`); the file
is never modified or deleted.

| Table | Columns | Notes |
|---|---|---|
| `instances` | `id` TEXT PK, `name` TEXT NOT NULL UNIQUE COLLATE NOCASE, `engine` TEXT NOT NULL, `version` TEXT NOT NULL, `port` INTEGER NOT NULL UNIQUE, `status` TEXT NOT NULL, `auth` TEXT NOT NULL, `superuser` TEXT NOT NULL, `pid` INTEGER, `process_started_at_secs` INTEGER, `process_started_at_nanos` INTEGER, `process_exe` TEXT, `autostart` INTEGER NOT NULL (0/1), `created_at_secs` INTEGER NOT NULL, `created_at_nanos` INTEGER NOT NULL | `port` in 1024..=65535. Process identity columns are all NULL or all NOT NULL (PID-reuse verification, invariant 7). |
| `runtimes` | `id` TEXT PK, `engine` TEXT NOT NULL, `version` TEXT NOT NULL, `arch` TEXT NOT NULL, `path` TEXT NOT NULL, `size_bytes` INTEGER NOT NULL, `state` TEXT NOT NULL, `installed_at_secs` INTEGER NOT NULL, `installed_at_nanos` INTEGER NOT NULL | UNIQUE `(engine, version, arch)`. |
| `archives` | `id` TEXT PK, `instance_name` TEXT NOT NULL, `engine` TEXT NOT NULL, `version` TEXT NOT NULL, `path` TEXT NOT NULL, `size_bytes` INTEGER NOT NULL, `archived_at_secs` INTEGER NOT NULL, `archived_at_nanos` INTEGER NOT NULL | Data kept by "archive" delete (D-18). |
| `credentials` | `instance_id` TEXT PK REFERENCES `instances(id)` ON DELETE CASCADE, `backend` TEXT NOT NULL, `key_ref` TEXT NOT NULL, `username` TEXT NOT NULL | Pointer only (CredentialRef). No password, token or URI column anywhere. |
| `settings` | `key` TEXT PK, `value` TEXT NOT NULL | Key/value settings. |

## Public API

| Function | Purpose |
|---|---|
| `Store::open(&DblPaths)` | Open/create/migrate the on-disk database. |
| `Store::open_in_memory()` | Migrated in-memory database for tests. |
| `insert_instance(&Instance)` | Insert one instance; duplicate name/port -> `AlreadyExists`. |
| `insert_instance_with_credential(&Instance, &CredentialRef)` | Atomic create (instance + credential pointer) in one transaction. |
| `get_instance(&InstanceId) -> Option<Instance>` | Fetch by id. |
| `get_instance_by_name(&str) -> Option<Instance>` | Fetch by name (case-insensitive). |
| `list_instances() -> Vec<Instance>` | All instances, ordered by name. |
| `update_status(&InstanceId, InstanceStatus)` | Persist the given status as-is (no transition rules). |
| `update_process_identity(&InstanceId, Option<u32>, Option<SystemTime>, Option<&Path>)` | Persist or clear `(pid, started_at, exe)`; all-or-none. |
| `update_port(&InstanceId, Port)` | Persist a new port; collision -> `AlreadyExists`. |
| `set_autostart(&InstanceId, bool)` | Persist the autostart flag. |
| `delete_instance(&InstanceId)` | Delete instance; cascades credential pointer, keeps runtimes/archives. |
| `add_runtime(&Runtime)` | Record an installed runtime; duplicate key -> `AlreadyExists`. |
| `list_runtimes() -> Vec<Runtime>` | List runtimes ordered by engine/version/arch. |
| `remove_runtime(&str)` | Remove a runtime by id. |
| `total_runtime_size() -> u64` | Sum of runtime `size_bytes`. |
| `add_archive(&ArchivedData)` | Record an archive. |
| `list_archives() -> Vec<ArchivedData>` | List archives, newest first. |
| `remove_archive(&str)` | Remove an archive record by id. |
| `get_setting(&str) -> Option<String>` | Read a setting. |
| `set_setting(&str, &str)` | Insert or replace a setting. |
| `get_credential(&InstanceId) -> Option<CredentialRef>` | Read the credential pointer (not the secret). |
| `set_credential(&CredentialRef)` | Insert or replace the credential pointer. |

## Invariants and notes

- No password/secret/token/URI column; only `CredentialRef` pointers. Enforced
  by a test that scans `sqlite_master`/`PRAGMA table_info`.
- Status is persisted as given; `InstanceStatus::apply` owns transition rules.
- Corrupt/unreadable database -> `DblError::Storage` (`DBL-3003`), file left
  untouched.
- `busy_timeout` lives in `store::schema::STORE_BUSY_TIMEOUT_MS`; moving it to
  `crate::consts` needs a shared-file change (not owned by W1-A).
- WAL is used, but a single `Store` owns one connection; concurrent writers
  must be serialized by the service. Readers on a separate connection do not
  block on an uncommitted writer (tested).
