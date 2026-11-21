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
- `create` is non-interactive: prints download/init progress on stderr (TTY only
  for progress noise; plain lines otherwise), prints URI on stdout on success.
  Unknown engine/version, bad name/port, duplicate name: mapped through exit-code
  table, never a stack trace.
- `delete` refuses without `--yes` when not interactive (exit 2). Default mode
  archives data and reports archive path; `--purge` deletes data. Interactive
  sessions prompt for exact instance name confirmation.
- `logs <name> [-f] [--lines N]` shows last N lines (default 200) via service
  `tail_logs`. `-f` follows by polling `tail_logs` and exits 0 on SIGINT.

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
