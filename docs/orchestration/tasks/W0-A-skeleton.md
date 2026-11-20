# W0-A — Workspace skeleton and core contract

Branch: `w0-skeleton`. Read first: AGENTS.md, then PRD §5 (feature map), §6 `F-SPK-05` and `F-INS`
(state machine), §7 (data model), §8 (command surface), §9 (architecture rules 1-6, folder structure).

## Goal

Seven other workers build modules in parallel after you. Deliver a compile-clean Cargo workspace and a
**tested** core contract so they never touch each other's files. The contract is the most important
thing you produce: design it carefully, keep it small, and test it.

## You own

`Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`, `.cargo/config.toml`,
`crates/**`, `docs/architecture/**`.
Do **not** create `.github/`, `packaging/`, `scripts/`, `deny.toml`, `COPYING`, `README.md`,
`docs/spikes/`, `spikes/`, `catalog/`. Other workers own them.

## Deliverables

### 1. Workspace

- Members: `crates/core` (`dblocal-core`, lib), `crates/gui` (`dblocal-gui`, lib with a stub
  `pub fn run() -> dblocal_core::Result<()>`; **no gpui dependency yet**), `crates/cli` (`dblocal-cli`,
  `[[bin]] name = "dblocal"`), `crates/xtask` (`xtask`).
- `dblocal` with no args calls `dblocal_gui::run()`. With a subcommand it parses with clap and returns
  `DblError::NotImplemented`. Parse every command in PRD §8 (`list status start stop uri create delete logs`),
  stubs only. Exit codes per `F-CLI`: 0 ok, 1 general error, 2 bad arguments, 3 instance not found, 4 engine failed to start.
- `cargo xtask ci` runs fmt check, clippy `-D warnings`, and tests. `cargo xtask catalog` is a stub that
  prints "not implemented" and exits 1 (W1-B fills it in). Add `[alias] xtask = "run --package xtask --"`.
- `[workspace.package]`: version `0.1.0`, edition 2024, `rust-version = "1.95"`, license `GPL-3.0-only`.
- `[workspace.lints]`: `unsafe_code = "deny"`; clippy `unwrap_used`, `expect_used`, `panic`, `todo`,
  `unimplemented` = deny (tests are exempt through `clippy.toml` allow-in-tests keys).
- `rust-toolchain.toml` pinned to `1.95.0` with `rustfmt` and `clippy`.
- `[workspace.dependencies]` holds every version from the AGENTS.md table. **Also declare them in
  `crates/core/Cargo.toml`** (`tokio`, `reqwest`, `rusqlite`, `sha2`, `tar`, `flate2`, `liblzma`, `zip`,
  `sysinfo`, `nix` for unix, `windows-sys` for windows, `tracing`, `rand`, `url`, `percent-encoding`,
  `semver`, `serde`, `serde_json`, `thiserror`, `uuid`, `tokio-postgres`, `mysql_async`, `redis`).
  Reason: later workers must not all edit the same Cargo.toml. Exceptions, which you leave to later
  workers with a `# W1-E adds keyring` comment: `keyring`. If any other crate fails to build on this
  machine without a missing system library, leave it out the same way and say so in your report.
- Core features: `mock` (empty feature now; W1-G fills `service/mock.rs`).

### 2. Core modules (binding names; internals are yours)

Pre-declare **every** module below in `crates/core/src/lib.rs` so later workers only add files, never edit `lib.rs`.
Modules marked *stub* contain only a `//! Owned by W1-x` doc comment and compile cleanly.

| Module | Content |
|---|---|
| `error` | `DblError` (`thiserror`, `#[non_exhaustive]`): `InvalidInput`, `NotFound`, `AlreadyExists`, `InvalidTransition`, `PortInUse`, `ChecksumMismatch`, `UnsafeArchive`, `Io`, `Network`, `Storage`, `Engine`, `Secret`, `Unsupported`, `Cancelled`, `NotImplemented`. `fn code(&self) -> &'static str` returns a stable `DBL-NNNN`. `fn user_message(&self) -> String` (friendly, never contains secrets). `pub type Result<T, E = DblError>`. |
| `consts` | Every constant in PRD §6/§10: canonical ports, start timeouts (30 s, Valkey 10 s), stop grace 20 s, status poll 2 s, catalog TTL 24 h, log rotation 10 MB × 3, password length 24, `DBLOCAL_HOME`, `.dblocal`. |
| `platform` | `Os`, `Arch`, `Platform::current()`, helpers to match catalog artifacts. |
| `secret` | `SecretString`: redacted `Debug`/`Display`, explicit `expose()`, no `Serialize`. |
| `paths` | `DblPaths` is the **only** place that knows `~/.dblocal` layout: `runtimes_dir`, `runtime_dir(engine, version, arch)`, `instances_dir`, `instance_dir(name)`, `data_dir`, `logs_dir`, `run_dir`, `archive_dir`, `secrets_dir`, `state_db`, `app_log`. `DblPaths::from_env()` honours `DBLOCAL_HOME`; `DblPaths::with_root(path)` for tests. |
| `model` | `EngineKey {Postgresql, Mysql, Mariadb, Valkey}` (`as_str`, `FromStr`, `canonical_port`, `display_name`, `family`); `InstanceId` (uuid v4 string); `InstanceName` (validated `^[a-z0-9][a-z0-9-]{1,30}$`); `Port` (validated ≥ 1024); `AuthMode {Password, None}`; `InstanceStatus {Draft, Provisioning, Initializing, Stopped, Starting, Running, Stopping, Failed, Deleting}` with `InstanceEvent` and `InstanceStatus::apply(self, event) -> Result<Self>` implementing **exactly** the PRD `F-INS` state machine; `InstanceSpec`; `Instance`; `Artifact`, `ArchiveFormat`, `EngineVersion`, `CatalogSnapshot`, `CatalogStatus {Fresh, Stale, Offline}`; `Runtime`, `RuntimeState`; `PortAllocation`; `CredentialRef`, `CredentialBackend`; `DeleteMode {PurgeData, ArchiveData}`; `ArchivedData`; `DownloadProgress {bytes_done, bytes_total, bytes_per_sec, eta}`. All immutable value types with `with_*` updates. |
| `engine` | `trait EngineAdapter: Send + Sync` with `key`, `canonical_port`, `initialize(&InitContext)`, `spawn_command(&RunContext) -> std::process::Command`, `health_check(&RunContext) -> Result<Health>`, `shutdown(&RunContext)`, `connection_uri(&RunContext, &Credential) -> SecretString`, `admin_ops() -> Option<&dyn AdminOps>`; `InitContext`, `RunContext`, `Health {Ready, NotReady(String)}`, `Credential`; `trait AdminOps` (documented methods: list/create/drop database, create user, reset password; Phase 2); `fn adapter_for(EngineKey) -> Box<dyn EngineAdapter>` returning stubs that answer `NotImplemented`. Files `engine/{postgres,valkey,mysql,mariadb}.rs` exist as stubs. |
| `service` | **`trait DblService: Send + Sync + 'static`**: `list_instances`, `catalog`, `create_instance(InstanceSpec) -> Result<OperationId>`, `start_instance`, `stop_instance`, `delete_instance(id, DeleteMode)`, `cancel_operation`, `connection_uri(id, UriFormat) -> Result<SecretString>`, `tail_logs(id, lines)`, `subscribe() -> tokio::sync::broadcast::Receiver<ServiceEvent>`. Plus `InstanceView`, `CatalogView`, `OperationId`, `OperationStage`, `UriFormat {Uri, DotEnv}`, and `ServiceEvent {InstanceChanged, OperationProgress, DownloadProgress, OperationFinished, CatalogUpdated}`. `service/mock.rs` exists behind `#[cfg(feature = "mock")]` as an empty file. This trait is the **only** surface the GUI and CLI use. Design it so a GUI built on a mock behaves identically against the real implementation. |
| `strings` | Indonesian user-facing strings the PRD quotes for core. Workers append. |
| stubs | `store`, `catalog`, `download`, `process`, `ports`, `secrets`, `instance` (the real `DblService` lands here in Wave 2). |

### 3. Required tests

- State machine: a table test covering **every** valid transition in the PRD diagram and **every** invalid
  pair (`InvalidTransition`).
- `InstanceName` and `Port` boundary cases. `EngineKey` string round trip.
- `DblPaths` layout under `with_root`, and that nothing resolves outside the root.
- `SecretString` never leaks through `Debug`, `Display`, or `format!("{:?}", struct_containing_it)`.
- Error codes are unique and match `^DBL-\d{4}$`; `user_message` is non-empty for every variant.
- CLI: `dblocal --help` succeeds; an unknown subcommand exits 2; a stub subcommand exits 1.

### 4. Contract document

`docs/architecture/CONTRACT.md`, at most 150 lines: module map, which worker owns each stub, the
`DblService` and `EngineAdapter` signatures, event flow, and how to add an engine.

## Evidence required in your report

`cargo build --workspace`, `cargo xtask ci`, `cargo run -p dblocal-cli -- --help`,
`cargo run -p dblocal-cli` (no args exits 0), `cargo doc --no-deps -p dblocal-core` with zero warnings,
`cargo llvm-cov -p dblocal-core --summary-only` at 80% or higher. Paste the output.
