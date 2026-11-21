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
- User-facing CLI strings live in `crates/cli/src/messages.rs` (Indonesian and
  the `DBL-84xx` codes). No handler formats a user-facing sentence or a
  diagnostic code itself; `messages::code_line`, `timeout_line`,
  `runtime_line`, `diagnosis_line`, `stage_line` and `download_progress` are the
  only builders. Flags/subcommand names stay English.
  `crates/core/src/strings.rs` is not modified.

## I/O contract (stdout is data, stderr is diagnostics)

Every stdout line below is asserted **exactly** (whole string, including the
trailing newline), not with `contains`, because stdout is what gets piped.

| Command | stdout | stderr |
|---|---|---|
| `uri <name>` | `<uri>\n` and nothing else | empty on success |
| `uri <name> --env` | the core builder's `.env` block (`service.connection_uri(..., DotEnv)`) + `\n`, verbatim | empty on success |
| `create ...` | `<uri>\n` and nothing else | progress stages; download percentage only on a TTY |
| `list` | the aligned table, no colour path at all | empty |
| `list --json` | one JSON line, a stable array of `{name, engine, version, port, status}` | empty |
| `logs <name>` | the last N lines | empty |
| `logs <name> -f` | history, then only appended lines | empty except a hard error |
| `status <name>` | the status word | empty |

`uri` is the only command that may print a password.

- `list` has no colour option: `format_list_table(instances)` takes no `color`
  parameter. A parameter is added only when a colour path exists.
- `status <name>` exits 0 only when `running`, 3 when missing, 1 otherwise for
  existing non-running instances.
- `start` blocks until healthy or timeout (`--timeout`, default 60 s). It waits
  on the service event stream, not blind sleep-poll loops. Failure prints the DBL
  code, the Indonesian message and the diagnosis when present; exit 4 for
  engine/start failures.
- `create` is non-interactive. Unknown engine, bad name/port, duplicate name,
  port in use and no installable artifact all exit 2; a failed download exits 1;
  a rolled-back create exits 4. Never a stack trace.
- `delete` refuses without `--yes` when not interactive (exit 2). Default mode
  archives data and reports archive path; `--purge` deletes data. Interactive
  sessions prompt for exact instance name confirmation.

## `logs -f`

The service exposes `tail_logs(id, n)`, so the CLI cannot stream. The follower
re-reads a window on a fixed interval and prints only what is new:

1. **One** first snapshot, `tail_logs(id, max(--lines, FOLLOW_WINDOW))`.
2. It prints that snapshot's **last `--lines` lines** as history; the rest of the
   window exists only as diff context and is never printed.
3. Every tick re-reads `FOLLOW_WINDOW` and diffs **snapshot against snapshot**
   through the pure `new_lines(previous, next) -> &[String]`, printing only the
   difference.

The earlier version kept the last N lines as `previous` but diffed against the
whole window, so `new_lines` compared the old tail with the new head, found no
overlap and reprinted the window. That is fixed; the guarantee is as follows.

| Situation | What is printed |
|---|---|
| nothing appended | nothing |
| 1 line appended | that 1 line |
| `k < FOLLOW_WINDOW` lines appended | those `k` lines |
| the window rotated (more than `FOLLOW_WINDOW` arrived in one interval) | the new window's tail; the lines pushed out of the window are lost |
| file truncated or rotated (no shared line) | the whole new snapshot, once |
| a run of duplicate lines shorter than the window, then an append | the appended line |
| the window is completely full and **every** line in it is the same text, then another copy is appended | nothing |

The last row is the documented limit: with no timestamps and no byte offsets in
`tail_logs`, a full window of identical text is byte-for-byte identical before
and after the append, so there is nothing left to compare. Everything else,
including a short run of duplicates, is reported correctly.

A read that fails transiently **keeps the previous snapshot and retries** on the
next interval; the diff is never reset by an error. Only "instance no longer
exists" ends the loop (exit 0), because there is nothing left to follow.

The SIGINT future is created **once outside** the loop and pinned
(`std::pin::pin!(tokio::signal::ctrl_c())`), so a signal that arrives while a
read is in flight is delivered to the next `select!` instead of being re-armed
and lost.

`cli_e2e` proves both halves on the shipped binary against a real Valkey 9.2.0:
`logs -f --lines 2` prints exactly 2 lines, and one line appended to the engine
log while following is printed exactly once.

## Cross-process status table

`start` and `stop` both see a concrete status, either in their first
`list_instances` or as the `from` of a refused compare-and-set. The loser cases
differ, and treating them alike is what produced a false "sudah berjalan"
followed by a 60 s wait:

| Command | Concrete status | Exit | Message |
|---|---|---|---|
| `start` | `busy` | 0 after waiting, or 4 on timeout | `sudah berjalan` |
| `start` | `starting` | 0 after waiting, or 4 on timeout | `sudah berjalan` |
| `start` | `running` | 0 | `sudah berjalan` |
| `start` | `stopping` | 1 | `[DBL-1004] sedang dihentikan oleh proses lain` |
| `stop` | `stopped` | 0 | `sudah berhenti` |
| `stop` | `stopping` | 0 after the wait, 1 on timeout | `[DBL-1004] sedang dihentikan oleh proses lain`, then `Siap.` |

`busy`/`starting`/`running` are the race **winners** a loser waits for: the
winner is on its way to `running`, so the loser waits on the shared store and
exits 0 (F-CLI-02 AC3). `stopping` is not: `stopping` can only become `stopped`
or `failed`, so `Running` will not come. A wait would burn the whole `--timeout`
and then claim an engine failure, which is wrong twice over, so it is reported
immediately. `stop` on an instance another process is already stopping is what
the user asked for: the idempotent success, never exit 2.

## Event-stream waits

`wait_for_status_rx` and `wait_operation_rx` treat
`RecvError::Lagged` as "the stream dropped events, the store is the authority":
they re-read `list_instances` and decide from the instance's status (or, for a
delete, from its absence) instead of ignoring the error and waiting for an event
that was itself dropped. A `Lagged` on a store that has not settled yet keeps
waiting, and a transient read failure during that re-read is ignored rather than
turned into a wrong verdict.

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

CLI-only codes come from the `DBL-84xx` range in
`docs/architecture/error-codes.md`: `DBL-8401` (a CLI wait ran out) and
`DBL-8402` (the CLI's own async runtime could not be built). A CLI timeout used
to wear `DBL-4001`, which is the engine range and claims an engine failure that
did not happen.

## Cross-process / GUI note

`LocalService` uses SQLite compare-and-set transitions and emits events per
process. The CLI observes changes through its own service instance over the same
`DBLOCAL_HOME`. A second process cannot receive another process's in-memory
broadcast events; GUI reflection within 2s (F-CLI-02 AC2) therefore depends on
GUI-side polling/refresh of store state or a future cross-process notification
mechanism. This worker does not implement GUI behaviour.

## Tests

- Unit: arg parsing, exit-code table, table/JSON formatters, delete confirmation
  logic, secret-never-printed property for non-`uri` commands, the pure
  `new_lines` cases (a)-(e), and the follow command driven by a service that
  scripts one snapshot per read.
- Integration: every command against `dblocal_core::service::mock::MockService`.
- Race table: `commands/tests/races.rs` drives a service wrapper for each
  concrete status in the table above, with its own deadline so a command that
  waits for a timeout fails as a red test instead of hanging.
- Binary smoke: `CARGO_BIN_EXE_dblocal` `--version`, `--help`, bad arg exit 2,
  `list`/`list --json` over empty temp `DBLOCAL_HOME`.
- E2e (ignored): real binary, real Valkey lifecycle, `logs -f` history and
  append, temp home, ports 20000-29999, cleanup guard. Run via
  `DBLOCAL_E2E=1 cargo xtask e2e`.

## Mutation checks

Each reversion was applied and the red output pasted, so the tests are known to
pin the behaviour rather than pass by accident.

**1. `uri` decorated** (`uri.rs`, `stdout_line(ctx.out, secret.expose())` ->
`stdout_line(ctx.out, &format!("URI: {}", secret.expose()))`):

```text
% cargo test -p dblocal-cli --lib 2>&1 | grep -A4 'uri_stdout'
---- commands::tests::inspect::uri_stdout_is_exactly_the_uri_and_stderr_is_empty stdout ----
assertion `left == right` failed
  left: "URI: postgresql://postgres:pwd-...@127.0.0.1:5432/postgres\n"
 right: "postgresql://postgres:pwd-...@127.0.0.1:5432/postgres\n"
test result: FAILED. 86 passed; 1 failed
```

**2. `new_lines` returning the whole window** (`logs.rs`, `&next[0..]`):

```text
% cargo test -p dblocal-cli --lib 2>&1 | grep -E 'FAILED|left:|right:' | head
test commands::logs::tests::first_snapshot_prints_only_the_requested_history ... FAILED
test commands::logs::tests::one_appended_line_is_exactly_one_new_line ... FAILED
test commands::logs::tests::a_full_window_that_rotated_reports_only_the_append ... FAILED
test commands::logs::tests::duplicate_lines_are_not_swallowed_below_the_window ... FAILED
test result: FAILED. 83 passed; 4 failed
```

**3. `stopping` back in the loser list** (`start.rs`, add `| "stopping"` to the
`InvalidTransition` arm and drop the dedicated arm):

```text
% cargo test -p dblocal-cli --lib 2>&1 | grep -E 'FAILED|did not return' | head
test commands::tests::races::start_of_a_stopping_instance_says_stopping_and_exits_1 ... FAILED
test commands::tests::races::start_refused_from_stopping_says_stopping_and_exits_1 ... FAILED
test result: FAILED. 85 passed; 2 failed
```

**4. `Lagged` ignored again** (`output.rs`, `Err(RecvError::Lagged(_)) => {}`):

```text
% cargo test -p dblocal-cli --lib 2>&1 | grep -E 'FAILED|running from the store' | head
test output::tests::wait_for_status_rx_recovers_a_dropped_final_event_after_a_burst ... FAILED
test output::tests::wait_operation_rx_resolves_a_lagged_finish_from_the_store ... FAILED
test result: FAILED. 85 passed; 2 failed
```

**5. The timeout ignored** (`start.rs`, `timeout.max(1)` -> a one-hour value):

```text
% cargo test -p dblocal-cli --lib 2>&1 | grep -E 'FAILED|timeout was ignored' | head
test commands::tests::races::start_of_a_stopping_instance_says_stopping_and_exits_1 ... FAILED
thread '...' panicked at ...: the command did not return within 5s; a timeout was ignored
test result: FAILED. HANG-FREE
```

The last one is the point of the deadline wrapper: with `stopping` mutated back
into the loser list *and* the timeout one hour long, the old test hung for seven
minutes. The test now returns in under a second with a red assertion.

## Start race — what the loser really sees

`shared_store_race.rs` runs 20 rounds of two `LocalService` instances over one
temp home, releasing both `start_instance` calls from a `Barrier` each round.
Exactly one wins every time. The loser gets `DBL-1004`; the wording is either `busy` (the row moved
between its read and the CAS) or the concrete `starting -> starting` (it read
the row just after the winner persisted `Starting`).

## Recorded e2e runs (2026-11-21, Fedora, Linux x86_64)

`DBLOCAL_E2E=1 cargo xtask e2e` (both targets: the W2-C `service_e2e` suite and
this task's `cli_e2e`), three consecutive runs on a seeded runtime cache:

| Run | service_e2e | cli_e2e | Result |
|---|---|---|---|
| 1 | 15 passed | 1 passed | every target passed |
| 2 | 15 passed | 1 passed | every target passed |
| 3 | 15 passed | 1 passed | every target passed |

The CLI case creates a real Valkey 9.2.0 through the **installed binary**, starts
it, asserts `status` exits 0 and the URI connects with the production `redis`
client (`PING`, `SET`, `GET`), proves `logs -f --lines 2` prints exactly 2 lines
and that a line appended while following is printed once, stops it and asserts
`status` is non-zero, then deletes with archive mode and asserts the archive
directory exists. The first cold-cache run of the *core* suite (before its
download populated `~/.cache/dblocal-dev`) failed two of W2-C's own cases, which
demand a cached artifact; a warm re-run was green. That cache requirement is
pre-existing and unchanged by this task.

On this machine the shared `CARGO_TARGET_DIR` can hold an `xtask` binary built by
a sibling worktree; `cargo build -p xtask` before `cargo xtask e2e` if the CLI
target does not appear in the output.
