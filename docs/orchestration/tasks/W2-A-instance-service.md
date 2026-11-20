# W2-A — The real `DblService`: instance lifecycle on top of every Wave 1 module

Branch: `w2-instance-service`. Wave 1 is merged except the GUI (W1-G, still running; you do not depend on it).
Read first, in this order: `AGENTS.md`, `docs/architecture/CONTRACT.md`, then the architecture note each module owner wrote:
`store.md`, `catalog.md` (or the catalog section of `CONTRACT.md`), `download.md`, `process.md`, `engines.md`, `secrets.md`, all in `docs/architecture/`.
PRD (`docs/prd/dblocal/PRD.md`): `F-INS-01..05`, `F-RTM-01..04`, `F-PRT-01..02`, `F-CRD-01..03`, `F-LOG-01..02`, `F-CAT-01..02`; decisions D-02, D-11, D-16, D-17, D-18, D-19, D-24. The trait you implement is `crates/core/src/service/mod.rs` and it is frozen: if you believe it must change, use `ask`.
Never open an image with the Read tool.

## Goal

`LocalService`: the production implementation of `DblService` that the GUI (W2-B) and the CLI (Wave 3) will call. It owns the lifecycle state machine and wires together the store, catalog, downloader, process layer, secrets and engine adapters. The mock that W1-G builds must behave the same way from the outside, so keep the observable behaviour (events, stages, error codes) exactly as the trait documents.

## You own

`crates/core/src/instance.rs`, `crates/core/src/instance/**` (the type and every operation), `docs/architecture/service.md`.
You may make **only these additive edits elsewhere**, each listed in your report: the `pub use`/`mod` lines in `service/mod.rs` and `lib.rs` that export `LocalService`; new constants in `consts.rs`; new `Store` methods in `store/**` (see "Cross-process safety"), with tests.
You may not change any other public signature of a merged module. If one is wrong, use `ask`; I would rather fix a contract than have you work around it.

## Threading model (AGENTS.md)

The trait is synchronous and returns quickly. `LocalService::new(paths, options)` owns a **dedicated tokio runtime on its own OS thread**; nothing runs on, or blocks, the caller's thread (the GUI's executor). Every call that touches disk, SQLite, a process or the network goes through that runtime or `spawn_blocking`. The store connection is shared behind one mutex and is never held across an `.await` or a long operation.
Return an `OperationId` immediately for long operations and report progress only through `subscribe()` events. `new()` must not block on network.

## State machine and persistence

Use `InstanceStatus::apply(InstanceEvent)` for every transition; do not duplicate its rules. **Persist every status change before the side effect it announces** (write `Starting`, then spawn; write `Stopping`, then signal), and persist the process identity (`pid`, start time, exe) in the same step as the spawn result, so a crash at any instant leaves a state that reconciliation can interpret.
After each persisted change emit `ServiceEvent::InstanceChanged` with the full `InstanceView`. Emit `OperationProgress` for each stage in `OperationStage` order and `OperationFinished` exactly once per operation, with the `UserFacingError` (stable `DBL-` code, Indonesian message from `strings.rs` keys, optional `FailureInfo`) on failure.
Throttle `DownloadProgress` to what the downloader already reports; never flood the channel. A slow or absent subscriber must never block the service (broadcast semantics, lagging receivers just lose old events).

## Operations

1. **create_instance** (`F-INS-01/02`, `F-RTM-*`, `F-PRT-01`, `F-CRD-01/03`): validate the spec; take the requested port or `suggest_port`; verify with a real bind probe (`F-PRT-02`) and against ports reserved by other instances in the store; select the artifact for this OS, arch and glibc from the catalog; if the runtime is not installed, download, verify, extract (reuse the downloader, stages Downloading, Verifying, Extracting, throttled progress, disk-space preflight error surfaced as a user error);
   generate the password through `secrets` (or the explicit no-password value), initialise through the engine adapter, store the secret **before** the instance row says it exists, then record instance plus `CredentialRef` atomically in the store; end in `Stopped`.
   **Nothing half-created, ever**: on failure or cancellation at any stage remove the data directory, the credential (both backends), and any row; keep an already verified runtime (it is reusable). Cancellation is honoured between and inside stages through the downloader's token and checked before each stage.
2. **start_instance** (`F-INS-03`, `F-PRT-02`, `F-LOG-02`): reject unless the state allows it; re-probe the port and fail with the typed `PortInUse` (and who owns it when known); rotate the log; build the command through the adapter, spawn detached through the process layer (stdin null, log appended, own process group), persist identity, then poll the adapter's health check within the configured timeout. Healthy means `Running`.
   On timeout, early exit or probe failure: terminate whatever is left (validated), read the log **tail**, run `diagnose_start_failure`, store a `FailureInfo` (diagnosis, code, raw tail) and move to `Failed`; the event and the view carry it. A `Failed` instance can be started again.
3. **stop_instance**: persist `Stopping`; ask the engine to shut down gracefully (adapter), wait up to `STOP_GRACE`, then escalate through `process::terminate` (identity-validated, never signals a mismatch). Report whether it was forced (the view/event says "kill paksa"). End in `Stopped` and clear the identity.
4. **Reconciliation at startup** (`F-INS-04`, D-02): for instances recorded as Starting, Running or Stopping, run `process::reconcile` with the real probe: a matching live process is **adopted as Running** (engines outlive the app), otherwise `Stopped` with the stored identity cleared. A `Stopping` that is found alive is finished or re-adopted, not left dangling. Run it once from `new()` on the runtime, never blocking `new()`, and emit the resulting events.
   A supervisor keeps watching adopted and started processes at `STATUS_POLL_SECS`: if one dies unexpectedly the instance becomes `Failed` with the log tail diagnosed, not silently `Running`. No tight loops, idle CPU stays near zero when nothing is running.
5. **delete_instance** (`F-INS-05`, D-18): `DeleteMode::Archive` moves the data directory into `archive/` and records it; `Purge` removes it. Stop first when running. Order the steps so a crash in the middle never leaves a row without data or data without a row you cannot find (write `Deleting`, act, then remove the row and the credential from both backends). The archive path is derived by the service, never from user text.
6. **change_port**, **data_size**, **set_autostart** (stores the flag only; actually launching at login is Wave 4), **cancel_operation**, **tail_logs**, **watch_logs** (opt-in, follower started only while watching, stops when disabled or the instance is gone; no idle file I/O), **connection_uri** (credential read through `SecretStore::get` with the reference recorded on the instance, URI built by the adapter, returned as `SecretString`), **validate_spec**, **suggest_port**.
7. **catalog / refresh_catalog** (`F-CAT-01/02`): serve the cached or bundled snapshot with its freshness; refresh honours the TTL unless `force`, never blocks the caller, reports Fresh, Stale or Offline, and a failed refresh never loses the working catalog.
8. **list_runtimes / remove_runtime / list_archives / purge_archive** (`F-RTM-04`): `remove_runtime` refuses with a typed error while any instance uses it; `purge_archive` accepts only ids it issued and **cannot be steered outside `archive/`** (test with `..`, absolute paths and symlinks).

## Cross-process safety (the GUI and a CLI share `~/.dblocal`)

Two processes will operate on the same home. A second `LocalService` over the same `DBLOCAL_HOME` must never double-start an instance or double-delete it. Use **compare-and-set status transitions in the store** (`Store::transition_status(id, from, to) -> bool`, one `BEGIN IMMEDIATE` statement; add it to `store/**` with tests) as the cross-process lock; the loser gets a typed "busy" error.
Inside one process also keep one operation per instance at a time. Test: two services on one temp home start the same stopped instance at the same instant, repeated 50 times: exactly one spawn each time, zero panics.

## Security invariants (AGENTS.md §3, all of them apply)

Nothing outside `DblPaths`. Never sudo or elevate. The password is never in argv, a log line, an event, an error, a `Debug` output or SQLite (grep your own events and errors in tests with a sentinel password). Listen on 127.0.0.1 only. PID validation before any signal (use the process layer; do not call `kill` yourself). Paths from the store are re-validated before they are deleted or moved (a corrupt row must not be able to delete outside the home).

## Tests (write first, red then green)

You cannot rely on the real engines here (that is W2-C). Use a scripted fake `EngineAdapter` (controllable init/health/shutdown, can fail at each step), a fake process probe where the process layer allows one, a local HTTP server on 127.0.0.1 port 0 for the downloader with a tiny real tar.gz fixture, `DBLOCAL_HOME` temp directories, and ports from 20000-29999 only (never 5432, 3306, 3307, 6379 or any port below 20000, and never ephemeral ports; see `ports::test_support`).
Required cases at least: the happy create/start/stop/delete with the exact event sequence; a failure injected at every stage of create leaves nothing behind (table-driven, assert directory, row, credential); cancel mid-download; port taken between suggestion and start; start timeout and early exit produce a diagnosed `Failed` and can be retried; forced stop is reported; reconciliation adopts a live process and clears a dead one, after a simulated app restart (drop the service, build a new one over the same home); the supervisor notices a killed process; delete in both modes including crash points; archive and runtime path traversal attempts; the two-service race above; sentinel-password scan; log watcher idle behaviour; refresh failure keeps the old catalog.
Every test that uses time, ports or threads must pass a **40-run loop** with zero failures; paste the pass count (this is the bar every earlier module had to meet). Coverage >= 80% lines on each file you own.

## Structure

Small files (< 400 lines typical, never > 800), one operation per file under `instance/`, shared plumbing (operation registry, event emitter, runtime handle, error mapping) in its own files. Tests live next to the code. No `unwrap`/`expect` outside tests. Immutability by default.

## Done when

`cargo xtask ci` and `cargo deny check` green; the 40-run loop result; the coverage table; the list of every edit outside your owned files; the event sequences for create, start, stop and delete as they actually occur (paste them); and a plain list of what you could not verify. **Commit before you report, and quote the hash printed by `git log --oneline -3` after the commit.**
