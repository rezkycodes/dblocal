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
    fn create_instance(&self, spec: InstanceSpec) -> Result<OperationId>;
    fn start_instance(&self, id: &InstanceId) -> Result<OperationId>;
    fn stop_instance(&self, id: &InstanceId) -> Result<OperationId>;
    fn delete_instance(&self, id: &InstanceId, mode: DeleteMode) -> Result<OperationId>;
    fn cancel_operation(&self, id: &OperationId) -> Result<()>;
    fn connection_uri(&self, id: &InstanceId, format: UriFormat) -> Result<SecretString>;
    fn tail_logs(&self, id: &InstanceId, lines: usize) -> Result<Vec<String>>;
    fn subscribe(&self) -> broadcast::Receiver<ServiceEvent>;
}
```

- Mutating calls return quickly with `OperationId`; progress is reported as events.
- `connection_uri` returns `SecretString` because a URI may embed the password.
- A GUI built on a mock implementing this trait must behave identically against the real impl.

Supporting types: `InstanceView { id, name, engine, version, port, status }`, `CatalogView { snapshot }`,
`OperationId`, `OperationStage { Queued, Downloading, Verifying, Extracting, Initializing, Starting, Done, Failed }`,
`UriFormat { Uri, DotEnv }`.

## Event flow

```text
UI/CLI -> DblService::create_instance / start / stop / delete
       -> Result<OperationId>
       <- ServiceEvent::OperationProgress { id, stage }
       <- ServiceEvent::DownloadProgress { id, progress }
       <- ServiceEvent::InstanceChanged  { id, status }
       <- ServiceEvent::OperationFinished{ id, success, message }
       <- ServiceEvent::CatalogUpdated   { status }
```

`subscribe()` returns a `tokio::sync::broadcast::Receiver<ServiceEvent>`. Consumers must tolerate
lagged receivers and treat events as hints to refresh state, not as the source of truth.

## `EngineAdapter` (per-engine contract)

```rust
pub trait EngineAdapter: Send + Sync {
    fn key(&self) -> EngineKey;
    fn canonical_port(&self) -> u16 { self.key().canonical_port() }
    fn initialize(&self, ctx: &InitContext) -> Result<()>;
    fn spawn_command(&self, ctx: &RunContext) -> Result<Command>;
    fn health_check(&self, ctx: &RunContext) -> Result<Health>;
    fn shutdown(&self, ctx: &RunContext) -> Result<()>;
    fn connection_uri(&self, ctx: &RunContext, credential: &Credential) -> SecretString;
    fn admin_ops(&self) -> Option<&dyn AdminOps> { None }
}
```

Contexts and types: `InitContext { instance_name, runtime_dir, data_dir, port, superuser, with_password }`,
`RunContext { instance_name, runtime_dir, data_dir, logs_dir, run_dir, port, superuser }`,
`Health { Ready, NotReady(String) }`, `Credential { username, password: SecretString }`.

`AdminOps` (Phase 2, behind `admin_ops()`): `list_databases`, `create_database`,
`drop_database`, `create_user`, `reset_password`. Engines without admin support return `None`.

`fn adapter_for(EngineKey) -> Box<dyn EngineAdapter>` currently returns stub adapters that answer
`NotImplemented`. Wave 1 replaces this dispatch once the per-engine modules are filled.

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

Every other pair returns `DblError::InvalidTransition`. `Deleting` is terminal.

## How to add an engine

1. Implement `EngineAdapter` in `crates/core/src/engine/<name>.rs` (module file is stubbed).
2. Keep the engine self-contained: init, spawn args, health probe, shutdown, URI, and optional admin ops.
3. Update `adapter_for` in `engine/mod.rs` to return the real adapter for the new `EngineKey`.
4. Add the `EngineKey` variant, canonical port, display name, family and superuser only if the
   engine is truly new (this requires coordinator approval because `model` is part of this contract).
5. Bind only to `127.0.0.1`/`::1`, never pass passwords in argv, and emit protocol-level health checks.
6. No `match engine_key` outside `engine/` or the `model` scalar mapping tables.

## Conventions

- All domain types are immutable: private fields, `with_*` updates returning a new value.
- `SecretString` redacts `Debug`/`Display`, has no `Serialize`, and exposes raw bytes only via `expose()`.
- `DblPaths` is the only source of `~/.dblocal` layout; tests use `DblPaths::with_root` and `DBLOCAL_HOME`.
- Errors are `DblError` with stable `DBL-NNNN` codes and Indonesian `user_message()` strings from `strings`.
- Validation lives at boundaries (`InstanceName`, `Port`, version tags, artifact checksums, path segments).

