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
   `DBL-NNNN` code, Indonesian message from `strings`, optional start
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

While it runs, the delete writes `delete.inflight` inside the instance directory,
carrying its PID and a per-operation nonce. That is what lets another process (or
a sibling service in the same process) tell "a delete is running" from "the app
died mid-delete", which is the input to the recovery table below.

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

### `LocalService`

- **Admin (`instance/admin_ops.rs`).** Every call loads the instance, enforces
  the engine gate (only PostgreSQL + a password instance is supported), reads
  the superuser password from the secret store, and dials loopback through
  `PgAdmin`. MySQL/MariaDB (awaiting `W4-A2`) and Valkey return the admin
  module's typed `Unsupported` (`DBL-8111`) with its Indonesian message — never
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
