# Core contract (W0-A)

This document is the binding contract for Wave 1 workers. Later workers add files inside the
pre-declared modules; they must not edit `crates/core/src/lib.rs`, rename modules, or change the
signatures below without going through the coordinator.

## Module map and ownership

| Module | Owner | Role |
|---|---|---|
| `error`, `consts`, `strings`, `secret`, `platform`, `paths`, `model`, `engine`, `service` | W0-A (done) | Stable contract, value types, traits |
| `store` | W1-A | SQLite schema, migrations, transactions |
| `catalog` | W1-B | Catalog snapshot load, refresh, `CatalogView` |
| `download` | W1-C | Resumable download, SHA-256 verify, safe extraction |
| `process`, `ports` | W1-D | Detached spawn, pidfile, reconciliation, bind probe |
| `secrets` | W1-E | Keychain and `0600` file fallback, `CredentialRef` I/O |
| `engine/{postgres,valkey,mysql,mariadb}` | W1-F | Real `EngineAdapter` implementations |
| `service/mock` | W1-G | In-memory `DblService` for the GUI shell |
| `instance` | W2-A | Real `DblService` implementation, orchestration |

All modules are declared in `crates/core/src/lib.rs`. Stub modules contain only a
`//! Owned by W1-x` doc comment and compile cleanly.

## `DblService` (the only front-end surface)

```rust
pub trait DblService: Send + Sync + 'static {
    fn list_instances(&self) -> Result<Vec<InstanceView>>;
    fn catalog(&self) -> Result<CatalogView>;
    fn refresh_catalog(&self, force: bool) -> Result<OperationId>;
    fn validate_spec(&self, spec: &InstanceSpec) -> Result<()>;
    fn suggest_port(&self, engine: EngineKey) -> Result<Port>;
    fn create_instance(&self, spec: InstanceSpec) -> Result<OperationId>;
    fn start_instance(&self, id: &InstanceId) -> Result<OperationId>;
    fn stop_instance(&self, id: &InstanceId) -> Result<OperationId>;
    fn delete_instance(&self, id: &InstanceId, mode: DeleteMode) -> Result<OperationId>;
    fn change_port(&self, id: &InstanceId, port: Port) -> Result<()>;
    fn data_size(&self, id: &InstanceId) -> Result<u64>;
    fn set_autostart(&self, id: &InstanceId, enabled: bool) -> Result<()>;
    fn cancel_operation(&self, id: &OperationId) -> Result<()>;
    fn connection_uri(&self, id: &InstanceId, format: UriFormat) -> Result<SecretString>;
    fn tail_logs(&self, id: &InstanceId, lines: usize) -> Result<Vec<String>>;
    fn watch_logs(&self, id: &InstanceId, enabled: bool) -> Result<()>;
    fn list_runtimes(&self) -> Result<Vec<Runtime>>;
    fn remove_runtime(&self, runtime: &str) -> Result<()>;
    fn list_archives(&self) -> Result<Vec<ArchivedData>>;
    fn purge_archive(&self, archive: &str) -> Result<()>;
    fn subscribe(&self) -> broadcast::Receiver<ServiceEvent>;
}
```

- Mutating calls return quickly with `OperationId`; progress is reported as events.
- `connection_uri` returns `SecretString` because a URI may embed the password.
- A GUI built on a mock implementing this trait must behave identically against the real impl.

Read models (each is flattened so the UI never re-implements filtering):

- `InstanceView { id, name, engine, version, port, status, auth, autostart, credential_backend, failure }`
  — `auth` drives the permanent "No auth" badge (F-CRD-03 AC2), `credential_backend` drives the
  "Disimpan di file, bukan keychain" note (F-CRD-01 AC2), `failure` is `Some` while `failed` (F-INS-03 AC4).
- `FailureInfo { code, message, diagnosis, log_tail }` — last 20 log lines inline (F-INS-03 AC4).
- `CatalogView { status, banner, engines: Vec<EngineEntry> }` — already filtered for this platform
  (F-CAT-02 AC1-3). `EngineEntry { engine, available, unavailable_reason, versions }`,
  `VersionEntry { version, eol, installed, size_bytes }`.
- `UserFacingError { code, message, diagnosis }` with `From<&DblError>`; never carries a secret.
- `OperationId`, `OperationStage { Queued, Downloading, Verifying, Extracting, Initializing, Starting, Done, Failed }`,
  `UriFormat { Uri, DotEnv }`.

`InstanceSpec.port` is `Option<Port>`: `None` means "allocate one", as `dblocal create` does
without `--port` (F-PRT-01). A persisted `Instance` always has a concrete port.

## Event flow

```text
UI/CLI -> DblService::create_instance / start / stop / delete
       -> Result<OperationId>
       <- ServiceEvent::OperationProgress { id, stage }
       <- ServiceEvent::DownloadProgress { id, progress }
       <- ServiceEvent::InstanceChanged  { view }             // full InstanceView
       <- ServiceEvent::OperationFinished{ id, result: Result<(), UserFacingError> }
       <- ServiceEvent::CatalogUpdated   { status }
       <- ServiceEvent::LogLines         { id, lines }        // only while watch_logs(id, true)
```

`subscribe()` returns a `tokio::sync::broadcast::Receiver<ServiceEvent>`. Consumers must tolerate
lagged receivers and treat events as hints to refresh state, not as the source of truth.

**Live logs choice.** Live logs are opt-in per instance through `watch_logs(id, enabled)` and arrive as
`ServiceEvent::LogLines`. This is the smallest API that satisfies F-LOG-01 AC1: the UI never polls,
the service does file I/O only for instances someone is looking at, and a lagged receiver simply
contains fewer lines instead of stalling the writer. `tail_logs` remains the non-streaming read.

## `EngineAdapter` (per-engine contract)

```rust
pub trait EngineAdapter: Send + Sync {
    fn key(&self) -> EngineKey;
    fn canonical_port(&self) -> u16 { self.key().canonical_port() }
    fn initialize(&self, ctx: &InitContext) -> Result<()>;
    fn spawn_command(&self, ctx: &RunContext) -> Command;
    fn health_check(&self, ctx: &RunContext) -> Result<Health>;
    fn shutdown(&self, ctx: &RunContext) -> Result<()>;
    fn connection_uri(&self, ctx: &RunContext, credential: &Credential) -> SecretString;
    fn diagnose_start_failure(&self, output: &str, exit_code: Option<i32>) -> StartFailure {
        StartFailure::Unknown
    }
    fn admin_ops(&self) -> Option<&dyn AdminOps> { None }
}
```

Contexts: `InitContext { instance_name, runtime_dir, data_dir, port, superuser, auth, password: Option<SecretString> }`
and `RunContext { instance_name, runtime_dir, data_dir, logs_dir, run_dir, port, superuser, auth, credential: Option<Credential> }`.
`password` is `Some` exactly when `auth == AuthMode::Password`; the same holds for `credential`.

- `shutdown` blocks and is bounded by `consts::STOP_GRACE_SECS`; the caller escalates to a forced kill
  after that and logs it (F-INS-03 AC3).
- `health_check` is a protocol-level probe. An authentication challenge (Valkey `NOAUTH`, a PostgreSQL
  auth request) proves the server is up and must answer `Health::Ready`.
- `AdminOps` reads the credential from `RunContext::credential`; no method takes one, so a caller cannot
  act as one user while reporting another. Methods (Phase 2): `list_databases`, `create_database`,
  `drop_database`, `create_user`, `reset_password`. Engines without admin support return `None`.
- `diagnose_start_failure` classifies raw start output into `StartFailure { PortInUse,
  DataDirVersionMismatch, PermissionDenied, BinaryMissing, Unknown }` (each with a stable `as_str()`),
  which the UI turns into a one-click fix (F-LOG-02). W0-B collects the real error texts; the default
  is `Unknown`, so an adapter that has not been taught degrades to "see the log".
- Never put a password in argv: use an engine environment variable (`PGPASSWORD`, `MYSQL_PWD`).

`fn adapter_for(EngineKey) -> Box<dyn EngineAdapter>` currently returns stub adapters that answer
`NotImplemented` for fallible operations (and a placeholder command for `spawn_command`). Wave 1
replaces this dispatch once the per-engine modules are filled.

## Instance state machine (`F-INS`)

`InstanceStatus::apply(self, InstanceEvent) -> Result<Self>` is the only legal transition path.

| From | Event | To |
|---|---|---|
| Draft | CreateWithDownload | Provisioning |
| Draft | CreateWithRuntime | Initializing |
| Provisioning | DownloadVerified | Initializing |
| Provisioning | DownloadFailed | Failed |
| Initializing | InitSucceeded | Stopped |
| Initializing | InitFailed | Failed |
| Stopped | StartRequested | Starting |
| Starting | HealthCheckPassed | Running |
| Starting | StartFailed | Failed |
| Running | StopRequested | Stopping |
| Running | ProcessExited | Failed |
| Stopping | ShutdownClean | Stopped |
| Stopping | ShutdownTimedOut | Failed |
| Failed | StartRequested | Starting (retry) |
| Stopped | DeleteRequested | Deleting |
| Failed | DeleteRequested | Deleting |
| Starting / Running / Stopping | ReconciledAbsent | Stopped |
| Provisioning / Initializing | Interrupted | Failed |

Every other pair returns `DblError::InvalidTransition`. `Deleting` is terminal.

### PRD clarifications of F-INS-04 (added in review round 1)

The diagram in §6 has no way out of `Running` except a crash, and no way out of `Provisioning` /
`Initializing` at all. Two events close that gap without changing any diagram arrow:

- **`ReconciledAbsent`** (`Starting | Running | Stopping -> Stopped`). Engines do not survive a logout or
  reboot (D-09), so on every launch the pidfile either names a dead PID or a PID reused by a foreign
  process. F-INS-04 AC2 says such an instance is "ditandai stopped": that is this transition, and it is
  recovery, not a crash.
- **`Interrupted`** (`Provisioning | Initializing -> Failed`). The app can die mid-download or
  mid-`initdb`; without this the instance would sit in a non-terminal status forever. F-INS-04 requires
  the next launch to surface it so the user can retry or delete.

`ProcessExited` remains `Running -> Failed`: a death *observed while the app is watching* is still a
failure (F-INS-03 AC4), so the two events are deliberately distinct.

## How to add an engine

1. Implement `EngineAdapter` in `crates/core/src/engine/<name>.rs` (module file is stubbed).
2. Keep the engine self-contained: init, spawn args, health probe, shutdown, URI, diagnosis, optional admin ops.
3. Update `adapter_for` in `engine/mod.rs` to return the real adapter for the new `EngineKey`.
4. Add the `EngineKey` variant, canonical port, display name, family and superuser only if the
   engine is truly new (this requires coordinator approval because `model` is part of this contract).
5. Bind only to `127.0.0.1`/`::1`, never pass passwords in argv, and emit protocol-level health checks.
6. No `match engine_key` outside `engine/` or the `model` scalar mapping tables.

## Conventions

- All domain types are immutable: private fields, `with_*` updates returning a new value.
- `SecretString` redacts `Debug`/`Display`, has no `Serialize`, zeroes its buffer on drop, and exposes
  raw bytes only via `expose()`.
- `DblPaths` is the only source of `~/.dblocal` layout; tests use `DblPaths::with_root` and `DBLOCAL_HOME`.
- Errors are `DblError` with stable `DBL-NNNN` codes and Indonesian `user_message()` strings from `strings`.
- Validation lives at boundaries (`InstanceName`, `Port`, version tags, artifact SHA-256, HTTPS URLs,
  non-zero artifact size, path segments).
