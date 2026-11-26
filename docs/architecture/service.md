# The real service — `LocalService` (W2-A)

`LocalService` is the production implementation of `DblService`
(`crates/core/src/service/mod.rs`). It owns the instance lifecycle state machine
and wires together everything Wave 1 built: the SQLite store, the catalog, the
downloader, the process layer, credential storage and the engine adapters.

Code: `crates/core/src/instance.rs` and `crates/core/src/instance/**`.
Contract references: `CONTRACT.md`, PRD `F-INS-01..05`, `F-RTM-01..04`,
`F-PRT-01..02`, `F-CRD-01..03`, `F-LOG-01..02`, `F-CAT-01..02`; decisions
D-02, D-11, D-16, D-17, D-18, D-19, D-24.

## Threading model

The trait is synchronous and every call must return quickly. Two rules follow
from that and from D-11:

1. **A dedicated runtime on its own OS thread.** `LocalService::new` builds a
   two-worker tokio runtime and moves it onto a thread named
   `dblocal-service-rt`, where it is driven by `block_on` until a shutdown
   signal arrives. The GUI's executor never sees a tokio task from core.
2. **Nothing long runs on the caller's thread.** A trait method either does a
   short, synchronous piece of work (validation, one compare-and-set status
   change) or spawns the long part on the service runtime and returns an
   `OperationId`. Everything that touches disk, SQLite, a process or the network
   runs inside the runtime or on `spawn_blocking`.

The store is one `rusqlite::Connection` behind a `Mutex`. Its lock is only ever
taken by a short closure (`Inner::db`) and never held across an `.await`, which
is why the async helpers call the store through `spawn_blocking`
(`helpers::db_async`). Engine adapters build their own tokio runtime internally,
so any adapter call (`initialize`, `health_check`, `shutdown`) also goes through
`spawn_blocking` (`helpers::blocking`).

Ownership detail worth knowing: the runtime lives in `LocalService`, not in the
shared `Inner`. Background tasks hold `Arc<Inner>`; if `Inner` owned the runtime,
the task would keep the runtime — and therefore itself — alive forever. With the
runtime in `LocalService`, dropping the last handle stops the supervisor through
the `stopping` flag and shuts the runtime down, and `Inner` drops with its tasks.

## Operation lifecycle and events

Each long operation (`create`, `start`, `stop`, `delete`, `refresh_catalog`)
follows the same shape:

1. Validate cheaply and, where a status changes, claim it with
   `Store::transition_status` — the compare-and-set that is also the
   cross-process lock.
2. Allocate an `OperationId`, spawn the work, return the id.
3. Emit `OperationProgress` for each stage in `OperationStage` order,
   `DownloadProgress` only when the downloader reports it, and
   `InstanceChanged` with a full `InstanceView` after every persisted change.
4. Emit `OperationFinished` exactly once, carrying `UserFacingError` (stable
   `DBL-NNNN` code, English message from `strings`, optional start
   `diagnosis`) on failure.

Events go through a `broadcast` channel. A slow or absent subscriber can only
lose old events (`RecvError::Lagged`); it never blocks the service.

`helpers::finish_operation` is the single place that emits the terminal event, so
"exactly once" is structural rather than a rule each operation has to remember.
A failed operation reports `OperationStage::Failed`; a successful one
`OperationStage::Done`.

### The two events that describe a row's fate

`InstanceChanged` carries a view, so it cannot express "the row is gone" or "the
stop was forced". Two additive events (contract commit `b377431`) do:

- **`InstanceRemoved { id }`** — emitted after a row is actually removed: a
  finished delete, a rolled-back create, and an interrupted create cleaned up by
  reconciliation. It is never emitted when a row survived (for example an aborted
  delete that returned the row to `Stopped`).
- **`StopForced { id }`** — emitted once, before the final `InstanceChanged`, only
  when the terminator actually escalated. A graceful stop never emits it, and it
  is never claimed for a kill that did not land.

**W2-B must not poll for removals.** It reacts to `InstanceRemoved` by dropping
the row and any tab or log watcher for `id`; nothing has to be re-fetched on a
timer, because every state change already arrives as an event.

### Stages as they actually occur

The downloader runs download → verify → extract as one fused, safe pipeline
(verification while streaming, extraction only after the hash matches). The
service therefore derives the stages from observable progress: reaching the
expected byte count means the download is finished and verification is starting.
`Verifying` is only emitted once the bytes have arrived, so a failed download
never claims to have verified, and `Extracting` is emitted only when the pipeline
returns successfully. This is a deliberate, documented compromise; a future
downloader callback that names its own phase would let the service emit these
stages without inference.

`OperationStage` has no delete- or stop-specific phase. Rather than misreport
`Starting`, both report `Queued` then `Done` with the status changes in between,
which is honest: the only long phase either has is the adapter shutdown bounded
by `STOP_GRACE_SECS`.

The exact sequences the tests assert (`mutations.rs`,
`create_start_stop_delete_emit_the_expected_instance_changes`):

```text
create  Progress(queued) Changed(initializing) Progress(initializing)
        Changed(stopped) Progress(done) Finished(ok)
start   Changed(starting) Progress(queued) Progress(starting)
        Changed(running) Progress(done) Finished(ok)
stop    Changed(stopping) Progress(queued) Changed(stopped)
        Progress(done) Finished(ok)
delete  Progress(queued) Changed(deleting) Progress(done) Finished(ok)
```

A create whose runtime must be downloaded inserts the download stages the same
way: `queued, downloading, verifying, extracting, initializing, done`.

## State machine and persistence

`InstanceStatus::apply(InstanceEvent)` is the only transition path; the service
never duplicates its rules.

**Persist before the side effect.** `Starting` is written before the spawn,
`Stopping` before the signal, `Deleting` before anything is moved or removed. The
process identity (`pid`, start time, executable) is persisted in the same step as
the spawn result, so a crash at any instant leaves a row reconciliation can
interpret.

**Every transition is a compare-and-set.** `Store::transition_status(id, from,
to)` is one `BEGIN IMMEDIATE` statement; it returns whether the row existed and
matched. Two services over one home cannot both move a row out of `Stopped`, so
exactly one wins a start and exactly one wins a delete. The loser gets a typed
`InvalidTransition { from: "busy" }`, which is the "busy" error the task asks
for. Inside one process, `Ops::lock_instance` additionally serialises operations
on the same instance.

**No status is ever stranded.** Three paths could otherwise leave a row in a
non-terminal status forever, and each has an explicit settle step:

- `run_start` wraps its body; any escaping error moves a row still in `Starting`
  to `Failed` with the identity cleared (`settle_failed`).
- `run_stop` always ends with `settle_stopped`, which finishes a row still in
  `Stopping` as `Stopped` with no identity. A stop whose graceful request *and*
  forced kill both fail therefore reports the failure and still leaves a usable
  row.
- `Deleting` is terminal by contract, so reconciliation finishes it: the data was
  archived or removed before the app died, and the row is removed by the same
  guarded code path the live delete uses (`reconcile::finish_delete`).

## Operations

### Create (`create.rs`)

1. Validate the spec and reject a duplicate name.
2. Take the requested port or `suggest_port`; probe it with a real bind
   (`F-PRT-02`) and check it against ports reserved by other instances.
3. Choose the artifact for this OS, architecture and glibc from the catalog. A
   `RuntimeSource` injected through options wins, which is how tests drive the
   real downloader over a loopback server.
4. Insert the row as `Provisioning` (download needed) or `Initializing` (runtime
   already installed) so the name and port are taken immediately.
5. If the runtime is missing: download, verify, extract with progress and the
   disk-space preflight, then record the runtime. `F-RTM-03` AC3: an installed
   runtime skips all of this.
6. Create the data, log and run directories, generate the password through
   `secrets` (or the explicit no-password value), and call the adapter's
   `initialize`.
7. Store the credential **before** the row can claim it exists, then write the
   credential pointer and move to `Stopped`.

Cancellation is checked before each stage and honoured inside the downloader
through its token. On any failure the instance directory (data, logs, run), the
credential on both backends and the row are removed; an already verified runtime
is kept because it is reusable.

### Start (`start.rs`)

Reject unless the state allows it; claim `Starting` with the CAS; re-probe the
port and fail with `PortInUse`; rotate the log; prepare engine directories; build
the command through the adapter; spawn detached through the injectable spawn
(stdin null, log appended, own process group in production); resolve the *server*
identity (from `postmaster.pid`/`valkey.pid` for PostgreSQL and Valkey, the child
for the others); persist it; then poll the adapter's protocol-level health check
within the configured timeout. Healthy means `Running`.

On timeout, early exit or probe failure the service terminates whatever is left
(identity-validated), reads the log tail, runs `diagnose_start_failure` through
the adapter, records a `FailureInfo` in memory and on the event/view, and moves
to `Failed`. A `Failed` instance can be started again.

### Stop (`stop.rs`)

Claim `Stopping`; ask the adapter for a graceful shutdown; escalate through the
injected terminator (production: `process::terminate`, which validates the
identity before every signal and never signals a mismatch). The row's final
status is decided by a **probe taken after the attempt**, never by whether a
signal "succeeded":

- the process is gone (or was never ours) → `Stopped`, identity cleared;
- the process is still alive under our identity → **`Running`, identity kept**,
  and the operation finishes with an error carrying a DBL code. An engine that
  survived must stay controllable; clearing its identity would orphan it, since
  reconciliation cannot adopt a row with no identity.

An unreadable credential never blocks a stop: it only removes the graceful path,
because the adapter cannot authenticate, and escalation still runs. A stop is
supposed to make the engine go away, so a locked keychain is downgraded to "no
credential" with a log line.

A stop that really escalated emits `ServiceEvent::StopForced { id }` exactly
once, before the final `InstanceChanged`, and never for a graceful stop
(`F-INS-03` AC3: the user is told it was a kill paksa).

The same "only clear an identity the probe proved gone" rule applies to
`start.rs::fail_with_tail` (a failed start whose launcher still runs keeps its
identity) and to `delete.rs::stop_for_delete` (a surviving engine fails the
delete instead of orphaning it).

### Delete (`delete.rs`)

Stop first when running. Claim `Deleting` (terminal) synchronously with the
compare-and-set, so a second caller is refused as busy before any operation
starts. `ArchiveMode` moves the data directory into `archive/<name>-<stamp>`,
records it, and removes the instance shell; `PurgeData` removes the whole
instance directory. Both paths re-validate every path against the home with
`safe_within` first. The row (and, by cascade, the credential pointer) is removed
only after the data is safe, and the secret is deleted from both backends before
the pointer that says where it lived is gone. The archive path is always derived
by the service, never from user text. The archive id is stable
(`arch-<instance-id-prefix>`, not time-derived) so two writers land on one row.
A finished delete ends with `ServiceEvent::InstanceRemoved`.

While it runs, the delete writes `instances/.<instance-id>.delete.inflight`,
carrying its PID, start time, executable and a per-operation nonce. That is what
lets another process (or a sibling service in the same process) tell "a delete is
running" from "the app died mid-delete", which is the input to the recovery table
below. The marker sits **beside** the instance directories, not inside one: a
delete removes `instances/<name>/` (data, logs, shell) before it removes the row,
so a marker stored in there would be destroyed by the very operation it
announces, and a sibling service reconciling in that window would "finish" a
delete that is still running and remove the row underneath it (`flake 8`). The
file name is keyed by the instance id; an instance name cannot collide with it
because it must start with `[a-z0-9]`. A delete removes its own marker on
completion, and reconciliation removes a stale one after it has decided the
delete is over.

If the engine survives the pre-delete stop, the delete **fails** rather than
removing the row: an orphan with no identity left could never be stopped.

### Reconciliation (`reconcile.rs`) and supervision (`supervisor.rs`)

Once from `new()`, on the runtime, never blocking it. Each case has one safe
answer:

| Row | Condition | Action |
|---|---|---|
| `Provisioning` / `Initializing` | — | run the create rollback (data, instance dir, both credential backends, the row), keep the verified runtime, emit `InstanceRemoved` |
| `Deleting` | a live in-flight marker owns it | leave it alone: the delete is running, not crashed |
| `Deleting` | data directory present | abort: back to `Stopped` (the delete never moved anything) |
| `Deleting` | data gone, no row for this instance's archive id, `archive/<name>-<digits>` exists | `add_archive` that directory, then remove the row |
| `Deleting` | data gone, a record exists (or a purge finished) | remove the row |
| `Starting` | no identity recorded | leave it: a spawn is in flight in some process |
| `Starting` / `Running` / `Stopping` | probe says still ours | adopt as `Running`, identity kept |
| `Starting` / `Running` / `Stopping` | probe says absent/reused/unknown | `Stopped`, identity cleared |

**Reconciliation never purges.** The delete mode is not on the row, so removing
data during recovery would silently discard a user's `ArchiveData` choice (the
D-18 default). A live `Stopping` row is adopted as `Running`, not written
`Stopped`: nothing is signalled during reconciliation, so the engine is still
there and a "stopped" label would only collide on the port at the next start.
Rows removed by reconciliation end with `InstanceRemoved`.

**Which archive directory belongs to which instance.** Recovery matches
`archive/<name>-<stamp>` exactly: the part after `<name>-` must be ASCII digits
only, so instance `pg` never claims `pg-main-1700000000000` (its remainder
`main-...` is not a stamp). Directories that any existing archive row already
points at (compared after normalisation) are skipped too. The "already recorded"
test is by the **stable archive id** `arch-<short instance id>`, never by name: a
user who deletes `xx`, recreates `xx`, and crashes that delete must still get the
second archive recorded even though an archived row for the earlier `xx` exists.
Both rules are tested (`tests/archive_recovery.rs`) and were mutation-checked
(a prefix match, or a name-based "recorded" test, each fails a test).

The in-flight marker records the owner's **full process identity** — PID, kernel
start time and executable — not a bare PID. A marker is live only when
`process::probe` validates that identity as `Match`; a reused PID after a reboot
(an unrelated process now holding the number) reads as stale, so a half-deleted
row is still recovered (AGENTS.md invariant 7). A per-operation nonce additionally
covers two services in one process.

**When the owner's own identity cannot be recorded.** `own_identity` reads this
process from the probe, but a hardened kernel (or a scripted probe in a test) can
refuse to report it; the marker then falls back to the current instant as its
start time. A reader in another process can never validate that stamp, so
`delete_in_flight` answers "not in flight" and reconciliation treats the
`Deleting` row as **interrupted**. That is safe by construction, because recovery
never purges: with the data directory still present the delete is aborted back to
`Stopped` and the user can retry in either mode; with the data gone the row is
removed (registering an unrecorded `archive/<name>-<digits>` first when one
exists). The worst case is that a delete still running in another process is
finished slightly early; the case that matters — never discarding data on a guess
— holds either way. Pinned by
`tests/recovery.rs::an_unverifiable_marker_identity_is_treated_as_interrupted_and_never_purges`.

### Stale claims and the compare-and-set

`helpers::claim_from(inner, id, expected, event)` applies the state machine to
`expected` and then persists `expected -> next` with
`Store::transition_status`. The caller passes the status it **observed**, never a
fresh read: if another writer moved the row in between, the CAS loses and the
call returns the typed busy error `InvalidTransition { from: "busy" }`, leaving
the row exactly where the winner put it. Delete uses
`helpers::claim_status(inner, id, expected, Deleting)`, the same CAS without the
state machine, because a delete claims from any non-terminal status by design
(it stops the engine itself, D-18). The two-service race test releases both calls
from a `Barrier` so they genuinely overlap, and a deterministic test calls
`claim_from` with a deliberately stale expected status.

### How the supervisor race is tested

The supervisor's `validate` call is the decision point, so the test gates it: a
`GatedProbe` signals "reached", blocks, and returns the answer the test releases.
The test waits for "reached", performs a complete honest stop (row `Stopped`,
identity cleared), then releases the gate with `Absent`. The supervisor now holds
a stale "dead" decision; the test asserts the row stays `Stopped`, no `Failed`
view is emitted, and no failure record is written. One deterministic round
proves what fifty timing-based rounds could not.

The supervisor polls at `STATUS_POLL_SECS` (injectable) and does nothing while no
instance holds a process. It only considers settled `Running` rows, skips any
instance with an operation in flight, and writes `Failed` with the
compare-and-set `Running -> Failed`; the failure record and the identity clear
happen only when that CAS wins. A stop that finishes between the probe and the
write therefore wins, and a clean stop is never relabelled a crash. When a watched
process really disappears the row becomes `Failed` with the log tail diagnosed —
never a silent `Running`.

The "operation in flight" skip is not redundant with the CAS. A `Running` row is
briefly lock-held by the start operation between persisting `Running` and dropping
its `InstanceLock`; during that window the supervisor must not even probe it. A
second gated test holds the lock, arms the probe gate, and asserts the supervisor
never reaches `validate` while the row is busy — and does once the lock is
released (`tests/gates.rs::the_supervisor_skips_a_running_row_while_an_operation_holds_it`).
Removing the guard fails that test.

### Operation bookkeeping

`Ops` owns three short-lived maps behind one `Arc`: which instances an operation
has locked (the in-process exclusivity), the cancellable operations' tokens, and
the ids of operations that reported `OperationFinished`. The finished ids exist so
a consumer that subscribed after an operation ended can still learn that it did
(by polling `is_finished` instead of blocking on a broadcast it missed). That
history is bounded to the most recent `FINISHED_HISTORY` (1024) ids in insertion
order, evicting the oldest, so a process that runs for a week does not retain one
id per operation.

The service shutdown latch is a **handle counter**, not `Arc::strong_count`: the
supervisor task and the workers hold their own `Arc<Inner>`, so the strong count
never reaches one while they run. Every `LocalService::clone` increments the
counter and each `Drop` decrements it; only the decrement that reaches zero sets
`stopping` and stops the watchers. A clone dropped while another handle lives
therefore leaves the service fully working
(`tests/gates.rs::dropping_a_clone_does_not_stop_the_service_but_the_last_handle_does`).

### Catalog (`catalog_ops.rs`)

`catalog()` serves the cached or bundled snapshot, already filtered for this
platform and marked with installed versions. `refresh_catalog` honours the TTL
unless `force`, runs in the background, reports `Fresh`/`Stale`/`Offline` through
`CatalogUpdated`, and a failed refresh never loses the working catalog.

### Reads and small mutations (`read_ops.rs`, `runtime_mgmt.rs`)

`change_port`, `data_size`, `set_autostart`, `cancel_operation`, `tail_logs`,
`watch_logs`, `connection_uri`, `validate_spec`, `suggest_port`, `list_runtimes`,
`remove_runtime`, `list_archives`, `purge_archive`.

- `change_port` is only allowed while the instance does not hold a process. It
  probes the target port, rewrites the engine config before persisting the new
  port (atomic write in the same directory), and only then updates the store and
  emits `InstanceChanged`. If the config rewrite fails, neither the file nor the
  store changes. `start` re-syncs the engine config from the stored port before
  spawning, so a crash between config rewrite and store update is repaired on the
  next start.
- `watch_logs` starts a follower only while watching; an idle app does no log
  I/O, and the follower stops when disabled or when the instance is gone.
- `connection_uri` reads the secret through `SecretStore::get` using the
  reference recorded on the instance and returns the adapter's URI as a
  `SecretString`. A no-password instance (`AuthMode::None`) has no credential
  row by design, so it renders through `connection_uri_no_password` instead
  (PRD `F-CRD-03`, `F-CLI-03`): the URI exists without a password and the `.env`
  block omits the password key. A password-mode instance whose credential row is
  missing stays a typed `NotFound { kind: "credential" }`.
- `remove_runtime` refuses with a typed error while any instance uses the
  engine and version.
- `purge_archive` accepts only ids it issued, and confines the recorded path to
  `archive/` after normalisation; a symlink inside `archive/` is refused by both
  its own metadata and its resolved target, so `..`, absolute paths and symlinks
  cannot steer the removal outside the home.

## W4-A — admin, update and open-in-client on the real service

W4-A added the database/user administration (`F-DBU-01..03`), self-update
(`F-PKG-05`) and open-in-external-client (`F-OPN-01..02`) surface to
`DblService`, implemented it in `LocalService` and `MockService`, and exposed it
through the CLI. Nothing under `crates/gui/**` is touched; the Database tab and
the open menu are W4-B's.

### Trait surface

The new methods return `ServiceResult<T>` (`Result<T, UserFacingError>`) rather
than `crate::error::Result<T>`, because the admin (`DBL-81xx`), update
(`DBL-80xx`) and client (`DBL-83xx`) modules own their own code ranges and the
CLI must print the code the module chose. Every method has a typed
not-implemented default, so a front-end compiled against an older service still
builds; both real implementors override all of them.

```rust
fn list_databases(&self, id: &InstanceId) -> ServiceResult<Vec<DatabaseInfo>>;
fn create_database(&self, id: &InstanceId, name: &str) -> ServiceResult<()>;
fn drop_database(&self, id: &InstanceId, name: &str, force: bool) -> ServiceResult<()>;
fn list_users(&self, id: &InstanceId) -> ServiceResult<Vec<UserInfo>>;
fn create_user(&self, id: &InstanceId, name: &str, db: &str, level: PrivilegeLevel)
    -> ServiceResult<SecretString>;
fn reset_superuser_password(&self, id: &InstanceId) -> ServiceResult<SecretString>;

fn config_view(&self, id: &InstanceId) -> ServiceResult<ConfigView>;
fn config_apply(&self, id: &InstanceId, changes: Vec<(String, String)>)
    -> ServiceResult<ConfigApplied>;
fn config_restore(&self, id: &InstanceId) -> ServiceResult<()>;
fn config_raw_path(&self, id: &InstanceId) -> ServiceResult<PathBuf>;

fn check_update(&self) -> ServiceResult<UpdateStatus>;
fn apply_update(&self) -> ServiceResult<()>;

fn detect_clients(&self, refresh: bool) -> ServiceResult<Vec<DetectedClient>>;
fn open_in_client(&self, id: &InstanceId, client: ClientKind, dry_run: bool)
    -> ServiceResult<LaunchPlan>;
```

`UpdateStatus` (`service::UpdateStatus`) is the renderable read model: the
current version, an optional `AvailableUpdateView` (version, tag, bounded notes,
asset name, size, whether this install may self-update), the
`update::InstallKind`, and the package-manager hint for a managed install.

**Long operations vs fast reads.** The task allowed either shape. Every new call
here is a **fast read or a short RPC**, not a background operation: an admin
statement or a client launch returns its result directly, and `check_update` is
one bounded HTTPS round-trip the caller explicitly asked for. `apply_update`
does the download + verify + apply synchronously (a self-update is a deliberate,
blocking action and the CLI is the caller). None of them needs an `OperationId`,
so no new stage was added to `OperationStage`.

### New events (additive, `#[non_exhaustive]`-safe)

W2-B/W4-B match on `ServiceEvent`; both new variants are additive and the GUI's
existing `translate` already has a `_ => None` arm.

- `AdminChanged { id }` — the list of databases or users for `id` is stale;
  re-read through `list_databases`/`list_users`. It carries **no rows** and no
  password, so the one-time create-user/reset-from-secret path can never ride an
  event. Emitted after a create/drop database, a created user, and a successful
  reset.
- `UpdateChecked { version: Option<String>, can_self_update: bool }` — an update
  check finished; `None` means the running version is current.
- `ConfigChanged { id }` — the config file of `id` was applied or restored
  (`F-CFG-01`); the Config tab re-reads it through `config_view`. It carries **no
  content**, because `valkey.conf` can hold `requirepass` and an event must never
  carry a secret.

### `LocalService`

- **Admin (`instance/admin_ops.rs`).** Every call loads the instance, enforces
  the engine gate (only PostgreSQL + a password instance is supported), reads
  the superuser password from the secret store, and dials loopback through
  `PgAdmin`. MySQL/MariaDB (awaiting `W4-A2`) and Valkey return the admin
  module's typed `Unsupported` (`DBL-8111`) with its message — never
  an empty list or a `todo!()`. The engine calls sit behind an injectable
  `AdminExecutor`; the production factory is `PgAdmin`, tests inject a fake.
  - `create_user` drops the half-created role if the GRANT fails, so a failure
    never leaves a login role with no privileges and no password anyone holds.
  - `reset_superuser_password` writes the new password through a `StoreSink` over
    the service's secret store, so the write prefers the keychain exactly like a
    create. The credential pointer is re-recorded **only after** the reset
    succeeded, so a failed reset leaves the pointer (and the keychain) untouched.
  - Neither the password the reset sets nor the password `create_user` returns is
    ever logged, put in an event, or in an error: the `AdminError` type has no
    free-form string slot (W3-A), and the canary test greps the event stream.
- **Update (`instance/update_ops.rs`).** `check_update` runs only when called (no
  timer, no startup check) through the injected `UpdateFetcher`; production is
  `ReqwestUpdateFetcher`. `apply_update` runs the same check, then refuses a
  package-manager-owned install (`F-PKG-05` AC2) **before any byte is
  downloaded**, returning the module's `UnsupportedInstallKind` (`DBL-8008`) with
  the manager hint. The download/verify/apply tail is an injected
  `UpdateInstaller`; production is `DownloadInstaller`, which stages under
  `~/.dblocal/update` and reuses `update::plan_install`/`apply` (so a managed
  install is double-refused). Tests inject a fake, so the whole path is offline.
- **Clients (`instance/clients_ops.rs`).** `detect_clients` caches the result on
  the first call and re-runs it when `refresh`; the cache is per-service (per
  process). `open_in_client` builds the target from the instance row, finds the
  detected client of that kind, resolves the terminal only for a CLI client (a
  GUI plan never reads it, so a headless host opens a GUI client fine), reads the
  password (for a CLI launcher only), and calls the existing `plan_open`. `run`
  executes it unless `dry_run`. The password is only ever in the launcher script
  `plan_open` builds; the plan's argv/URL/command string never carry it, and the
  service never returns it.

New `ServiceOptions` seams (defaults are the production values):
`host_probe`, `spawner`, `update_fetcher`, `update_installer`, `install_kind`,
`current_version`, `update_repo`, `admin_factory`.

### `MockService`

The mock implements every new method with deterministic data so a GUI built
against it exercises each branch offline: PostgreSQL reports two seeded
databases and the superuser role, create/drop/create-user mutate an in-memory
list and emit `AdminChanged`, a reset rotates the map `connection_uri` reads (so
the printed URI carries the new password), every other engine is typed
unsupported, `check_update` reports a fixed `0.9.0` unless the seeded current
version is already that, `apply_update` refuses a package-manager install set via
`set_install_kind`, and detection returns a fixed `psql` + `TablePlus` cast.

### Tests

- `instance/tests/service_admin.rs` (24 unit tests) drives the real
  `LocalService` through the public trait with an injected `AdminExecutor`,
  `UpdateFetcher`, `UpdateInstaller`, `HostProbe` and `Spawner`: the engine gate
  per engine, invalid-name rejection before any statement, the grant rollback,
  the one-time password, the reset pointer update only on success, the
  package-manager refusal before any download, the up-to-date and network-error
  codes, cached detection, dry-run vs launch, and that no event or plan carries
  the password.
- `tests/service_admin_e2e.rs` (5 `#[ignore]`d tests, registered in `cargo xtask
  e2e`) proves the wiring on a real PostgreSQL under the e2e lock:
  UTF-8 create + list with a size, drop refusing a live session then succeeding,
  read-only cannot write, read-write can, reset updates engine + stored
  credential + URI, and a refused sink rolls the reset back while the old
  password still works and the pointer never moves. Recorded: 5 passed in 12.97 s
  at load 16.57.
- `crates/cli/src/commands/tests/service_admin.rs` (19 tests) covers `db`, `user`,
  `password`, `update` and `open` against the mock, including the typed-name
  confirmation and the exit codes.

### What W4-A does not prove

- MySQL/MariaDB admin execution (the executor is `W4-A2`); Valkey has none by
  design.
- A live OS keychain: the e2e uses the production `0600` file backend, and the
  refreshable detection cache is proven with a fake host.
- A real GitHub release download/apply (tests inject the fetcher and installer).
- Windows/macOS clients and terminals.

### Mutation checks

Each reversion was applied to the working tree, the red test pasted, and the file
restored (`git status --short` empty afterwards).

**1. Typed-name confirmation dropped** — `commands/db.rs`, the
`if !yes && let Err(code) = confirm_name(…)` guard replaced by two `let _`:

```text
test commands::tests::service_admin::db_drop_without_confirmation_is_refused_non_interactively ... FAILED
test commands::tests::service_admin::db_drop_rejects_a_mismatched_confirm ... FAILED
test result: FAILED. 17 passed; 2 failed; 0 ignored; 0 measured; 96 filtered out
```

**2. Identifier validation bypassed** — `instance/admin_ops.rs`, `create_database`
sends a fixed valid token instead of the caller's name:

```text
test instance::tests::service_admin::create_database_rejects_a_bad_name_before_calling_the_engine ... FAILED
test instance::tests::service_admin::create_database_announces_the_admin_change ... FAILED
test result: FAILED. 32 passed; 2 failed; 0 ignored; 0 measured; 1209 filtered out
```

**3. Password sent through an event** — `instance/admin_ops.rs`,
`reset_superuser_password` emits a `LogLines` carrying the new password:

```text
test instance::tests::service_admin::the_admin_event_carries_no_secret ... FAILED
an event leaked the password: LogLines { id: InstanceId("2c16b941-…"), lines: ["reset-once-pw"] }
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1242 filtered out
```

**4. Credential pointer written before the reset succeeds** — `instance/admin_ops.rs`,
the pointer `set_credential` moved ahead of `reset_superuser_password`:

```text
test instance::tests::service_admin::a_failed_reset_leaves_the_credential_untouched ... FAILED
assertion `left == right` failed: the pointer must not move on a failed reset
  left: CredentialRef { … backend: File … }
 right: CredentialRef { … backend: Keychain … }
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1242 filtered out
```

**5. `update apply` runs for a package-manager install** — `instance/update_ops.rs`,
the `if !kind.can_self_update()` guard becomes `if false`:

```text
test instance::tests::service_admin::apply_update_refuses_a_package_manager_install_before_downloading ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1242 filtered out
```

### Recorded runs

- `cargo test -p dblocal-core --features mock --lib service_admin` x40 at load
  16.01: **40/40 passed**.
- `DBLOCAL_E2E=1 flock … cargo test -p dblocal-core --test service_admin_e2e --
  --ignored --test-threads=1`: **5 passed** in 14.15 s at load 16.31 (a second run
  earlier passed 5 in 12.97 s at load 16.57).
- `cargo xtask ci` under the heavy lock: **all gates passed** at load 16.19.

## Cross-process safety

The GUI and a CLI share `~/.dblocal`. The compare-and-set
`Store::transition_status` is the lock: the row is the shared resource, and only
one process can move it from `Stopped` to `Starting`. The test
`two_services_on_one_home_spawn_exactly_once` runs 50 rounds of two independent
services racing to start the same stopped instance and asserts exactly one start
is accepted each time with at most one spawn.

## Security invariants

- Listens on `127.0.0.1` only (engines are configured by their adapters).
- Never sudo or elevate; every write stays under `DblPaths`.
- The password never reaches argv (adapters use env vars or config files),
  SQLite (only a `CredentialRef` pointer is stored), a log line, an event, an
  error or `Debug` output. `sentinel_password_never_appears_in_events_errors_or_the_database`
  greps the event stream and the database file for a sentinel.
- PID validation before any signal: the service never calls `kill`; it goes
  through `process::terminate`, which refuses anything that does not validate.
- Paths read from the store are re-validated with `safe_within` before being
  deleted or moved.
- Instance-layout directories (`instances/`, `archive/`, `runtimes/`,
  `downloads/`) and per-instance `data/`, `logs/`, `run/` are created/tightened
  to `0700` on Unix. Valkey `prepare_run`/`initialize` also tighten data/run/logs
  to `0700` so a permissive pre-existing directory cannot leak local access.

## Tests

`crates/core/src/instance/tests/`:

| File | What it covers |
|---|---|
| `harness_fakes.rs` | Scripted adapter (fail at init/health/shutdown), fake process table, terminating fake, secret backends, and the `GatedProbe` that parks the supervisor at its decision point |
| `harness.rs` | The service builder (injectable spawn/port/timings/probe/secrets), a two-handle builder sharing the process table and secret store, and the bounded event/state waiters |
| `lifecycle.rs` | Happy create/start/stop/delete; stage order; duplicate name; create failure leaves nothing; port taken between create and start; diagnosed unhealthy start plus retry; forced stop emits `StopForced` and a graceful one does not; delete in both modes; delete-while-running; credential pointer vs SQLite; no-auth |
| `download.rs` | Real loopback server + real `tar.gz`: download→verify→extract with stages; wrong checksum leaves no runtime; cancel mid-download |
| `catalog.rs` | Platform filtering; failed refresh keeps the catalog; TTL rule; `CatalogUpdated`; `suggest_port` skips owned ports; `validate_spec` names conflicts |
| `mutations.rs` | Immediate-exit diagnosis and failure; event sequences for create/start/stop/delete; `change_port`; `data_size`; `set_autostart`; `tail_logs`; unknown operation; a refused kill leaves the row `Running` with its identity; a stop with an unreadable credential still succeeds |
| `gates.rs` | Deterministic gate races: a stale supervisor decision never overwrites a finished stop; the supervisor skips a `Running` row while an operation holds its lock and probes it once free; a claim from a stale status is refused (start and delete); the in-flight marker trusts identity, not a bare PID; `rollback_create` refuses a symlinked instance directory; dropping a clone does not stop the service but the last handle does |
| `archive_recovery.rs` | Archive recovery matches only `<name>-<digits>` (never a longer name); records nothing when no own directory exists; a recreated name records its own second archive by stable id; a recorded directory that sorts last is skipped so recovery keeps its own |
| `recovery.rs` | Delete crash-point table (both modes) never loses data; interrupted create leaves nothing behind and emits `InstanceRemoved`; live `Stopping` adopted as `Running`; in-flight delete owned elsewhere is left alone; an unverifiable marker identity is treated as interrupted and never purges |
| `races.rs` | Delete ends with `InstanceRemoved`; failed create ends with `InstanceRemoved` and one `OperationFinished(Err)`; a recovered `Failed` row carries a diagnosis from the log; a delete refuses a surviving engine |
| `safety.rs` | Two-service start race and two-service delete race (50 rounds each, both released from a `Barrier`, exactly one spawn / one removal); archive traversal/absolute/symlink; runtime in use; reconciliation adopts/clears; supervisor notices a kill; log watcher idle; sentinel scan; corrupt row cannot delete outside home |

All ports come from `ports::test_support` (20000-29999); tests never bind 5432,
3306, 3307, 6379 or an ephemeral port. `cargo test` needs no network beyond the
in-process loopback server and no real engine.

## What this round does not do

- MySQL and MariaDB use the stub adapters (Wave 3 / W2-C own the real ones), so
  their create/start paths are exercised only for the state machine, not against
  a real server.
- Autostart only stores the flag; launching at login is Wave 4.
- Windows-only paths (spawn flags, archive ACLs) are compiled but not run here.
- `Verifying`/`Extracting` stage boundaries are inferred from download progress
  (see "Stages as they actually occur").
- The delete in-flight marker distinguishes a live delete from a crash using the
  owner PID against the live process table. A PID reused by a *different*
  process in the few-millisecond window between the crash and the next launch
  would read as "still running" and delay recovery by one launch; that is the
  same accepted residual PID-reuse trade-off documented for engine identities,
  and it never loses data (recovery simply waits, and the marker's nonce is not
  registered so the following launch proceeds).
- The two crash-point and race tests use the harness's fake process table. The
  real `SysinfoProbe` and `process::terminate` paths they stand in for are
  covered by W1-D's own evidence tests and by W2-C's real-engine e2e.

## W4-A fix 1 — three defects and one untested data-loss path

The reviewer's follow-up round on the merged W4-A (`43e7f8f`). All five gates
stay green; the real-binary and e2e transcripts are below.

### F1 — the credential pointer after a reset is now tested

`admin_ops::reset_superuser_password` re-records the credential pointer to the
backend that actually stored the new password (keychain unavailable → file). The
new test `reset_moves_the_credential_pointer_to_the_backend_that_stored_it` seeds
a **keychain** pointer with the old password in the keychain, makes the keychain
unavailable so the reset's write falls back to the **file**, and asserts the
pointer now says `File`, the named credential holds the new password, and
`connection_uri` carries it. Replacing the pointer write with a no-op turns it
red (pasted below). This is the F-DBU-03 data-loss path: without the pointer
move, the engine would hold the new password while DBLocal reads the old
backend — an instance DBLocal could never log in to.

### F2 — `user create` with an unknown database

`create_user` checks the target database exists **before** creating the role, so
a missing database is `DBL-8105` (Database not found) instead of the GRANT
step's connect failure (`DBL-8100`, "server unreachable"). `map_connect_error`
also maps SQLSTATE `3D000` on connect to `DatabaseNotFound`, so a database
dropped between the check and the grant still answers correctly. No role is left
behind either way (the check precedes `create_user`).

### F3 — `--no-password` PostgreSQL says "engine not supported"

`executor_for` used to gate on `engine != Postgresql || auth != Password`, so a
no-password PostgreSQL instance answered `DBL-8111` "not supported for this
engine". The gate is now split: a non-PostgreSQL engine is `DBL-8111`, and a
PostgreSQL instance created without a password is the new `DBL-8117` ("this instance
was created without a password, …"), which names what to do.

### Update source not found (`dblocal update check`)

An HTTP 404 (and a 403 that is not a rate limit) from the releases API is the
repository missing or private, not a connection failure. It now maps to
`DBL-8011` "the update release source was not found or is not public", instead of
`DBL-8004` "check connection".

### Mutation checks (each reverted, red pasted, restored, `git status --short` empty)

| Mutation | Red test | Evidence |
|---|---|---|
| F1: no-op the pointer write | `reset_moves_the_credential_pointer_to_the_backend_that_stored_it` | `left: Keychain, right: File` |
| F2: drop the existence check | `create_user_reports_database_not_found_without_creating_a_role` | the create is attempted and the call no longer returns DBL-8105 |
| F3: collapse the gate back | `a_no_password_postgres_instance_reports_its_own_error` | `left: "DBL-8111", right: "DBL-8117"` |
| Update: 404 back to Network | `check_maps_an_unknown_repository_to_source_not_found` | `left: "DBL-8004", right: "DBL-8011"` |

### Recorded runs

- `cargo test -p dblocal-core --features mock --lib`: 1257 passed (2 ignored).
- `DBLOCAL_E2E=1 flock … cargo test -p dblocal-core --test service_admin_e2e --
  --ignored --test-threads=1`: **6 passed** in 13.22 s at load 12.61 (the new F2
  case is one of them).
- Real binary on an isolated `DBLOCAL_HOME` with a real PostgreSQL 18.7.0:
  - F3: `dblocal create --engine postgresql --version 18.7.0 --name nopw
    --port 25432 --no-password`, then `db list nopw` and `password reset nopw`
    both print `[DBL-8117] This instance was created without a password, …` (exit 1), not
    `DBL-8111`.
  - F2: `dblocal user create pg1 u9 --db nosuchdb --level ro` prints
    `[DBL-8105] Database not found.` (exit 2) with the server running;
    `SELECT count(*) FROM pg_roles WHERE rolname='u9'` is `0` (no role left
    behind); `db drop pg1 nosuchdb --yes` still reports `DBL-8105`; `user create`
    into an existing `appdb` succeeds.
  - Cleanup: both instances deleted with `--purge`; `pgrep -x postgres` = 0.

### Open questions (from the fix task; answers, no code this round)

1. **Storing the created user's password (F-DBU-02 AC2).** The PRD says the
   password is shown once *and stored in the keychain like the superuser*. Doing
   that costs: a credential key that is (instance, user) rather than per instance
   — `credentials` is currently keyed by `instance_id` with a single row, so the
   schema gains a `username`/role dimension (or a second table
   `user_credentials(instance_id, rolname, backend, key_ref, …)` with a composite
   key); `CredentialRef` already carries a `username`, but `credential_ref_for`
   derives the key as `dblocal:<instance-id>`, so a per-role key
   (`dblocal:<instance-id>:<role>`) and the file-backend name encoder must
   change with it; the delete/purge paths (`delete.rs`, `rollback_create`,
   reconciliation) must delete every role credential for the instance, not one;
   and instances created before the change need a migration (a new table with no
   backfill is the safe form). It is a schema + multi-key change, not a line.
   Recommendation: take it to the product owner as a deliberate deviation for v1
   — show-once is implemented and honest (`user create` says so), and a stored
   per-role secret multiplies the credential surface (N secrets per instance to
   rotate, back up and delete) for a password the user pastes into their app
   once. If it is required, the second-table form above is the smallest correct
   shape, owned as its own task with its own migration and e2e.
2. **Per-instance admin guard.** `admin_ops` calls take no `InstanceLock`, so a
   `password reset` can overlap a `stop`/`delete`. Corruption, not a crash:
   - A `delete` removes the credential from **both** backends and drops the
     pointer. If it lands between the reset's `sink.store` and the pointer
     update, the reset writes a fresh secret into a backend the delete already
     swept and then re-records a pointer for a row that is about to be removed —
     a secret leaked in the keychain with no row pointing at it (a cleanup
     orphan, not data loss). If it lands after the pointer write, the delete's
     own "remove both backends" still removes the new secret, so the row cannot
     end up claiming a secret that is gone.
   - A `stop` mid-statement makes the `ALTER ROLE`/`GRANT` fail with a
     connection error, which surfaces as `DBL-8100`/`DBL-8110`; the engine's
     `reset_with` rolls back to the old password, so the stored credential and
     engine stay consistent. The worst case is a reset that had already applied +
     verified but not yet stored when the engine died: the rollback re-applies the
     old password over a connection that no longer exists, which returns
     `RollbackFailed` — an honest "state inconsistent" that tells the operator to
     check the credential. Nothing silently diverges.
   - The smallest fix is a read-only guard next to `LockKind`: a new
     `LockKind::Admin` that `admin_ops` takes for the duration of a call and that
     a `stop`/`delete` (and the supervisor's crash write) already treats as
     "busy" through the existing `is_busy`/`lock_instance` exclusion. Failing
     that, refusing when `ops.is_busy(id)` is the one-line version, but it has a
     TOCTOU window the lock does not. It is not implemented in this round.
3. **`update check` against a missing/private repo** is answered in code above:
   `DBL-8011` "the update release source was not found or is not public", with a fake-fetcher
   test. `consts::UPDATE_REPO` now names the real repository,
   `rezkycodes/dblocal`, but that repository is private, so a production check
   still hits `DBL-8011` until it is made public (or releases are published
   elsewhere) — the message says so instead of blaming the connection.
