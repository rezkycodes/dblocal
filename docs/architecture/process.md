# Process, port and log core (W1-D)

This document describes the engine-independent process, port and log primitives
implemented for DBLocal (task `W1-D`, branch `w1-process`). It covers the code
under `crates/core/src/process.rs`, `crates/core/src/process/**`,
`crates/core/src/ports.rs` and `crates/core/src/ports/**`.

## Goals and invariants

- **No blind signals (invariant 7, `F-INS-04`, W0-B C-08).** A PID is never
  enough. Every signal path validates the recorded `ProcessIdentity` (pid +
  executable path + start time) against the live process table and only acts on
  `Match`. `Absent`, `Reused` and `Unknown` are never signalled.
- **Detached engines (`F-INS-03` AC2, D-10).** Engines outlive the app: stdin is
  null, stdout/stderr are appended to the instance log, the child is put in its
  own process group, and the parent drops every handle.
- **Loopback-only, real bind probes (`F-PRT-01..02`, invariant 1 edge cases).**
  Port checks bind on `127.0.0.1` and `::1` when present with default socket
  options (no `SO_REUSEADDR`), close the socket before returning, and treat a
  foreign listener as `InUse`.
- **Log data is opaque (`F-LOG-01`, invariant 5).** Tail/follow/rotate never
  parse and never log line contents; secrets must not reach engine logs.

## Ports (`crates/core/src/ports/`)

- `probe_bind(port) -> Result<BindReport>` performs a real bind on loopback v4
  and, if available, v6. It returns `BindStatus::Free` only when every probed
  interface is free. `AddrNotAvailable`/`Unsupported` for IPv6 are treated as
  "no IPv6 loopback", not as a conflict. `probe_or_in_use` folds unexpected
  errors into `InUse` so callers degrade conservatively.
  - **No `SO_REUSEADDR`.** `std::net::TcpListener::bind` sets `SO_REUSEADDR` on
    Unix (verified in this toolchain's std source), which would make a busy port
    look free. The probe therefore creates the socket through
    `tokio::net::TcpSocket` (`socket2` underneath, already a core dependency),
    binds with default options, and drops it. On Windows the platform default is
    `SO_EXCLUSIVEADDRUSE` semantics, which is the behaviour the PRD asks for.
- `suggest_port(engine, reserved) -> Result<Port>` starts at the engine's
  canonical port (5432/3306/3307/6379), steps upward, skipping ports in
  `reserved` (other DBLocal instances) and ports reported busy by the live
  probe. The search is bounded (`MAX_SCAN`) and returns `DblError::PortInUse`
  on exhaustion instead of looping.
- `PortChecker` is the abstraction used by `suggest_port_with` and tests; the
  production checker is `BindProbeChecker`.

## Process identity and probing (`crates/core/src/process/`)

- `ProcessIdentity { pid, exe, started_at }` is the recorded proof of "our"
  process. It rejects `pid == 0` and an empty executable at construction.
- `ProcessProbe::observe(pid)` reports `Missing`, or `Found { exe, started_at }`
  (either fact may be `None` if unreadable). `ProcessProbe::validate` is the
  single assembly point for checks and classifies into:
  - `Match` — same executable (tolerating Linux's ` (deleted)` suffix and a
    canonical-path fallback) and start time within `START_TOLERANCE` (±2 s).
  - `Absent` — no live process.
  - `Reused` — live, but a different executable or a start time outside
    tolerance.
  - `Unknown` — live but identity cannot be proven; treated as unsignalable.
- `SysinfoProbe` uses `sysinfo` 0.40 and refreshes only the requested PID
  (`with_exe`, no cmd/cwd) so reconciliation of ~20 instances stays fast
  (`F-INS-04` AC3). Zombie/dead statuses are reported as `Missing`. Start time
  comes from `process.start_time()` (kernel-derived epoch seconds).
- `fake::FakeProbe` is an in-memory table for deterministic unit tests.

## Spawn (`process::spawn`)

`spawn_detached(command, log_path) -> Result<ProcessIdentity>`:

1. Creates the log's parent directories and opens the log `create+append`
   (mode `0600` on Unix).
2. Wires `stdin = null`, `stdout/stderr = log`.
3. Detaches: Unix `process_group(0)` via `std::os::unix::process::CommandExt`;
   Windows `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW` via
   `creation_flags` (safe, no `unsafe`).
4. Spawns, captures the PID, drops the child handle (the process is not waited).
5. Observes the live PID and returns a `ProcessIdentity` (with a short retry to
   absorb `/proc` visibility races). If the child is gone immediately, returns
   `DblError::Engine`.

No `unsafe` code is used. `process_group(0)` is available in Rust 1.95's std and
is sufficient for a new session/process group on Unix for our use case; no
audited `pre_exec(setsid)` was required.

## Terminate (`process::terminate`)

`terminate(identity, grace)` and `terminate_with(identity, grace, probe, signals)`:

1. Validate before the first signal: `Absent -> NotRunning`, `Reused|Unknown ->
   Refused`.
2. Send a polite stop (Unix `SIGTERM`; Windows termination via `sysinfo` kill),
   poll until the identity no longer matches or the grace period elapses.
3. Re-validate: kill **only** if still `Match`. If the PID was recycled during
   the wait, return `Refused` and send nothing.
4. After escalation wait for confirmation; report `Forced` (the "kill paksa"
   case, `F-INS-03` AC3) or an error if it somehow survives `SIGKILL`.

Signals go through the `SignalSender` trait so the escalation/re-validation
logic is unit-tested without real processes. The default Unix sender uses
`nix::sys::signal::kill`.

## Reconcile (`process::reconcile`)

`reconcile(&[ReconcileRecord], &dyn ProcessProbe) -> Vec<ReconcileOutcome>` is
pure: it takes recorded identities and returns, in the same order, either
`Adopted { id, identity }` (`Match`) or `ReconciledAbsent { id, state }` for
`Absent`, `Reused`, `Unknown`, or a missing pidfile. It never signals and never
mutates state; the service applies the `ReconciledAbsent` state event or adopts
the running process.

## Logs (`process::logs`, `F-LOG-01`)

- `tail_lines(path, n)` reads from the end in fixed chunks (no whole-file read),
  tolerates a missing file and a partial last line, and handles `\r\n`.
- `LogFollower` is a poll-based follower tracking a byte offset and a pending
  partial line; it resets on truncation/rotation and tolerates a missing file.
  `follow_interval()` is 250 ms.
- `rotate_if_needed(path)` (and `_with` for tests) shifts
  `name -> .1 -> .2 -> ...` keeping `LOG_ROTATE_KEEP` generations when the file
  exceeds `LOG_MAX_BYTES`, then recreates an empty active file.

## Tests and evidence

- Unit tests cover port probing/allocation (ephemeral listeners, reserved/busy
  skips, bounded exhaustion), identity classification (match/absent/reused/unknown,
  tolerance, deleted-suffix), reconcile table, terminate escalation/refusal, log
  tail/follow/rotation (including a 50 MB tail and a 16 MB single-line file), and
  spawn error/log handling.
- Real-process evidence (Unix), in `process/evidence.rs` and `process/spawn.rs`:
  - **Survival:** `spawn_detached(/bin/sleep 30)`, drop every handle, wait, and
    the child still validates as `Match`; a second test has a helper process
    start the future engine and exit, so the engine is *reparented* and still
    matches — the exact post-quit shape of `F-INS-03` AC2.
  - **PID reuse:** after killing a recorded `sleep`, its identity no longer
    matches; a live PID that belongs to a different executable is `Reused`.
  - **No signal on mismatch:** `terminate` on an identity pointing at the real
    PID but a different executable sends nothing, and the process is asserted
    alive afterwards.
  - **Reconcile table:** adopted / absent / reused / missing-pidfile in one pass.
- A subtlety worth knowing: `spawn()` returns before the child finishes `exec`,
  so an immediate `/proc/<pid>/exe` read can report the *parent* binary. Spawn
  therefore records the identity only once two consecutive observations agree
  and the executable is not our own.
- Platform note: this worktree is Linux x86_64. The real-process evidence is
  `#[cfg(unix)]`, so macOS follows the same Unix path but was not executed here.
  **Windows was not executed**: the whole crate cannot cross-build in this
  container (`liblzma-sys` needs a mingw/msvc C toolchain), so the Windows-only
  pieces — the `sysinfo` kill path in `terminate` and the `creation_flags`
  detachment in `spawn` — were type-checked in isolation against
  `x86_64-pc-windows-gnu` in a scratch crate, and both compile. Windows runtime
  behaviour still needs a real Windows runner.
