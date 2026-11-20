# W1-D — Detached processes, PID validation, ports, logs

Branch: `w1-process`. Read first: AGENTS.md, `docs/architecture/CONTRACT.md`, PRD §6 `F-INS-03`, `F-INS-04`, `F-PRT-01..02`, `F-LOG-01`,
decisions D-02, D-16, and **W0-B findings C-07, C-08, C-09** in `docs/spikes/engines/README.md` (measured on real PostgreSQL and Valkey).

## Goal

Everything about operating-system processes and ports that is engine-independent: start a child that survives the app, prove a PID is still *our* process before
ever signalling it, reconcile after a crash, probe ports for real, and tail logs.

## You own

`crates/core/src/process.rs`, `crates/core/src/process/**`, `crates/core/src/ports.rs`, `crates/core/src/ports/**`, `docs/architecture/process.md`. Nothing else.

## Deliverables

### Ports (F-PRT-01, F-PRT-02)

- `probe_bind(port)`: a **real** bind on `127.0.0.1` (and `::1` when the host has IPv6), closed before returning. **Unix: set `SO_REUSEADDR`** (corrected in review round 2: the engines set it, and without it a port in TIME_WAIT after a stop is falsely reported `InUse` for about 60 s; an active listener still blocks the bind). **Windows: do not set it** (it would let a bind succeed over an active listener). Reports `Free` or `InUse`
  (and ideally who, best effort).
- `suggest_port(engine, reserved: &HashSet<Port>)`: canonical port first, then +1 upward until it is neither reserved by another DBLocal instance nor in use.
  Bounded search with a typed error when exhausted.

### Processes (F-INS-03, F-INS-04)

- `ProcessIdentity { pid, exe, started_at }` and a `ProcessProbe` trait (`sysinfo`-backed real implementation, fake for tests).
- `validate(identity) -> Match | Absent | Reused | Unknown`: process alive **and** executable path equals the recorded one (tolerate Linux's ` (deleted)` suffix)
  **and** start time equals the recorded one within a small tolerance. A live but unrelated PID is `Reused`. **Never signal anything that is not `Match`** (invariant 7;
  W0-B proved `pg_ctl status` calls a live unrelated `sleep` "running").
- `terminate(identity, grace)`: validate, SIGTERM, wait, then SIGKILL only if still `Match`, and report that the kill was forced (so the UI can say "kill paksa").
  On Windows use the safe `sysinfo` kill. Everything must compile with `unsafe_code = deny`; if you believe one audited `pre_exec(setsid)` is unavoidable, isolate it
  in its own file with `#[allow(unsafe_code)]`, a `SAFETY` comment, and tell me why in the report.
- `spawn_detached(Command, log_path)`: stdin null, stdout and stderr appended to the log file (created `0600`), new process group (Unix `process_group(0)`) or
  `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW` on Windows. Returns the identity. **Prove survival**: a test spawns a real `sleep`, drops
  every handle, and shows the child is still `Match` afterwards, then terminates it.
- `reconcile(records, probe) -> Vec<ReconcileOutcome>` for instances recorded as Starting, Running or Stopping: still `Match` is adopted as Running; `Absent` or `Reused`
  yields `ReconciledAbsent`. Pure logic over the probe, so it is fully unit-tested with a fake. It returns outcomes only; the service applies the state events.

### Logs (F-LOG-01)

- `tail_lines(path, n)` reading from the end (no full-file read), tolerant of a partial last line and of missing files; a poll-based follower for "live" mode; a size cap
  with keep-the-last-N rotation at instance start (consts). Log lines are data: never interpret them, and never log a secret.

## Tests (RED first)

Port probe against a listener you open yourself on an ephemeral port (never 5432, 3306, 3307, 6379 or any port < 20000); suggest skips reserved and busy ports; PID reuse
is `Reused` (spawn `sleep`, record identity, kill it, spawn another process, show the old identity is not `Match`); terminate never signals a mismatch (assert the unrelated
process is still alive); detached survival; reconcile table; tail on a 50 MB file stays fast; partial line; missing file.

## Done when

`cargo xtask ci` green, >= 80% lines on files you own, report includes the survival and PID-reuse evidence. Windows and macOS parts are compiled only; say so plainly.
