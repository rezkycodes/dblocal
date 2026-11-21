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

### Catalog (`catalog_ops.rs`)

`catalog()` serves the cached or bundled snapshot, already filtered for this
platform and marked with installed versions. `refresh_catalog` honours the TTL
unless `force`, runs in the background, reports `Fresh`/`Stale`/`Offline` through
`CatalogUpdated`, and a failed refresh never loses the working catalog.

### Reads and small mutations (`read_ops.rs`, `runtime_mgmt.rs`)

`change_port`, `data_size`, `set_autostart`, `cancel_operation`, `tail_logs`,
`watch_logs`, `connection_uri`, `validate_spec`, `suggest_port`, `list_runtimes`,
`remove_runtime`, `list_archives`, `purge_archive`.

- `watch_logs` starts a follower only while watching; an idle app does no log
  I/O, and the follower stops when disabled or when the instance is gone.
- `connection_uri` reads the secret through `SecretStore::get` using the
  reference recorded on the instance and returns the adapter's URI as a
  `SecretString`.
- `remove_runtime` refuses with a typed error while any instance uses the
  engine and version.
- `purge_archive` accepts only ids it issued, and confines the recorded path to
  `archive/` after normalisation; a symlink inside `archive/` is refused by both
  its own metadata and its resolved target, so `..`, absolute paths and symlinks
  cannot steer the removal outside the home.

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
| `gates.rs` | Deterministic gate races: a stale supervisor decision never overwrites a finished stop; a claim from a stale status is refused (start and delete); the in-flight marker trusts identity, not a bare PID; `rollback_create` refuses a symlinked instance directory |
| `archive_recovery.rs` | Archive recovery matches only `<name>-<digits>` (never a longer name); records nothing when no own directory exists; a recreated name records its own second archive by stable id |
| `recovery.rs` | Delete crash-point table (both modes) never loses data; interrupted create leaves nothing behind and emits `InstanceRemoved`; live `Stopping` adopted as `Running`; in-flight delete owned elsewhere is left alone |
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
