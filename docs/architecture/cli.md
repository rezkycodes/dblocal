# CLI (W3-D)

This document describes the `dblocal` command-line interface implemented in
`crates/cli` (F-CLI-01..03, `logs`, PRD §8, D-02/D-16/D-18).

## Shape

- One binary: `dblocal`. No args opens the GUI (`dblocal_gui::run()`), a
  subcommand runs the CLI. The GUI hand-off in `cli.rs` is preserved.
- Command handlers are library functions over `&dyn DblService` with injected
  `out`/`err` writers and an `is_interactive` flag (`commands::Ctx`). `main`
  builds `LocalService::new(DblPaths::from_env()?, ServiceOptions::default())`
  and dispatches. A service-open failure prints a clear Indonesian error on
  stderr and exits 1; it never panics.
- User-facing CLI strings live in `crates/cli/src/messages.rs` (Indonesian).
  Flags/subcommand names stay English. `crates/core/src/strings.rs` is not
  modified.

## I/O contract

- **stdout is data, stderr is diagnostics/progress.**
- `uri` prints the URI alone plus newline, nothing else. `uri --env` prints the
  `.env` block from the core URI builder (`service.connection_uri(..., DotEnv)`),
  not hand-built. `uri` is the only command that may print a password.
- `list` prints an aligned table (no colour when stdout is not a TTY). `--json`
  emits a stable array with English keys: `name`, `engine`, `version`, `port`,
  `status`.
- `status <name>` prints status; exit 0 only when `running`, 3 when missing, 1
  otherwise for existing non-running instances.
- `start` blocks until healthy or timeout (`--timeout`, default 60s). It waits on
  the service event stream, not blind sleep-poll loops. If already running it
  prints `sudah berjalan` and exits 0. Failure prints DBL code, Indonesian
  message and diagnosis when present; exit 4 for engine/start failures.
- `stop` waits for `Stopped`.
- `create` is non-interactive: stage names print on stderr as plain lines; the
  high-frequency download percentage is only printed when stderr is a TTY (so a
  captured log is not flooded). The URI goes to stdout on success. Unknown
  engine, bad name/port, duplicate name, port in use and no installable artifact
  all exit 2; a failed download exits 1; a rolled-back create exits 4.
  Never a stack trace.
- `delete` refuses without `--yes` when not interactive (exit 2). Default mode
  archives data and reports archive path; `--purge` deletes data. Interactive
  sessions prompt for exact instance name confirmation.
- `logs <name> [-f] [--lines N]` shows last N lines (default 200) via the
  service's own reader, `tail_logs` — no ad-hoc file reads, so rotation and
  truncation stay in `process::logs`. `-f` re-reads a 200-line window every
  250 ms and prints only the lines past the previous snapshot, so a followed log
  is not duplicated; it selects on SIGINT and the interval (no busy loop), exits
  0 on Ctrl-C, and returns when the instance is deleted.

## Exit codes

| Code | Meaning |
|---:|---|
| 0 | success / help / version |
| 1 | general error |
| 2 | bad arguments / invalid input / refused confirmation |
| 3 | instance not found |
| 4 | engine failed to start |

`DblError` mapping lives in `output::code_for`; command handlers may refine based
on operation context (e.g. create argument errors -> 2).

## Cross-process / GUI note

`LocalService` uses SQLite compare-and-set transitions and emits events per
process. The CLI observes changes through its own service instance over the same
`DBLOCAL_HOME`. A second process cannot receive another process's in-memory
broadcast events; GUI reflection within 2s (F-CLI-02 AC2) therefore depends on
GUI-side polling/refresh of store state or a future cross-process notification
mechanism. This worker does not implement GUI behaviour.

## Tests

- Unit tests: arg parsing, exit-code table, table/JSON formatters, delete
  confirmation logic, secret-never-printed property for non-`uri` commands.
- Integration: every command against `dblocal_core::service::mock::MockService`.
- Binary smoke: `CARGO_BIN_EXE_dblocal` `--version`, `--help`, bad arg exit 2,
  `list`/`list --json` over empty temp `DBLOCAL_HOME`.
- E2e (ignored): real binary, real Valkey lifecycle, temp home, ports 20000-29999,
  cleanup guard. Run via `DBLOCAL_E2E=1 cargo xtask e2e`.

## Mutation checks

Two reversions were applied and the red output pasted here, so the tests are
known to pin the behaviour rather than pass by accident.

**1. `code_for` mapping `NotFound` to 1 instead of 3** (`output.rs`):

```text
% sed -i 's/DblError::NotFound { .. } => EXIT_NOT_FOUND/DblError::NotFound { .. } => EXIT_GENERAL/' crates/cli/src/output.rs
% cargo test -p dblocal-cli --lib
test cli::tests::exit_codes_follow_the_contract ... FAILED
test commands::tests::inspect::exit_code_table_maps_codes ... FAILED
  left: 1
 right: 3
test result: FAILED. 66 passed; 2 failed
```

**2. The non-interactive `delete` refusal made unreachable** (`delete.rs`,
`if !ctx.is_interactive` -> `if !ctx.is_interactive && false`):

```text
% cargo test -p dblocal-cli --lib
test commands::tests::inspect::delete_refuses_without_yes_when_not_interactive ... FAILED
thread '...delete_refuses_without_yes_when_not_interactive' panicked at
  crates/cli/src/commands/tests/inspect.rs:160:5
test result: FAILED. 67 passed; 1 failed
```

## Start race — what the loser really sees

`shared_store_race.rs` runs 20 rounds of two `LocalService` instances over one
temp home, releasing both `start_instance` calls from a `Barrier` each round.
Exactly one wins every time. The loser gets `DBL-1004`; the wording is either `busy` (the row moved
between its read and the CAS) or the concrete `starting -> starting` (it read
the row just after the winner persisted `Starting`). The first 40-run sweep
found the second shape, so `start.rs` now treats `busy`, `starting`, `running`
and `stopping` as "another process owns it": print `sudah berjalan`, wait on the
shared store, exit 0. Ten sweeps of the 20-round test (200 races) were green
after the fix.

## Recorded e2e runs (2026-11-21, Fedora, Linux x86_64)

`DBLOCAL_E2E=1 cargo xtask e2e` (both targets: the W2-C `service_e2e` suite and
this task's `cli_e2e`), three consecutive runs on a seeded runtime cache:

| Run | service_e2e | cli_e2e | Result |
|---|---|---|---|
| 1 | 15 passed | 1 passed, 0.45 s | every target passed |
| 2 | 15 passed | 1 passed, 0.48 s | every target passed |
| 3 | 15 passed | 1 passed, 0.45 s | every target passed |

The CLI case creates a real Valkey 9.2.0 through the **installed binary**, starts
it, asserts `status` exits 0 and the URI connects with the production `redis`
client (`PING`, `SET`, `GET`), stops it and asserts `status` is non-zero, then
deletes with archive mode and asserts the archive directory exists. The first
cold-cache run of the *core* suite (before its download populated
`~/.cache/dblocal-dev`) failed two of W2-C's own cases, which demand a cached
artifact; a warm re-run was green. That cache requirement is pre-existing and
unchanged by this task.

On this machine the shared `CARGO_TARGET_DIR` can hold an `xtask` binary built by
a sibling worktree; `cargo build -p xtask` before `cargo xtask e2e` if the CLI
target does not appear in the output.
