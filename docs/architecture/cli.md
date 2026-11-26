# CLI (W3-D)

This document describes the `dblocal` command-line interface implemented in
`crates/cli` (F-CLI-01..03, `logs`, PRD §8, D-02/D-16/D-18).

## Shape

- One binary: `dblocal`. No args opens the GUI (`dblocal_gui::run()`), a
  subcommand runs the CLI. The GUI hand-off in `cli.rs` is preserved.
- Command handlers are library functions over `&dyn DblService` with injected
  `out`/`err` writers and an `is_interactive` flag (`commands::Ctx`). `main`
  builds `LocalService::new(DblPaths::from_env()?, ServiceOptions::default())`
  and dispatches. A service-open failure prints a clear error on
  stderr and exits 1; it never panics.
- User-facing CLI strings live in `crates/cli/src/messages.rs` (user-facing and
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

### No-password instances (`AuthMode::None`)

`create --no-password` creates an instance with no stored credential. The
instance is reachable only on loopback; a password-less database exposed beyond
`127.0.0.1` would be an open service, so this mode is appropriate only for local
development on a trusted machine. The contract is the same as for any other
instance, with the password removed rather than replaced by an empty one:

| Engine | `uri` / `create` stdout | `uri --env` |
|---|---|---|
| PostgreSQL | `postgresql://<superuser>@127.0.0.1:<port>/<db>\n` | `DATABASE_URL=...`, `DB_HOST/PORT/DATABASE/USERNAME`; **no `DB_PASSWORD` line** |
| MySQL / MariaDB | `mysql://<superuser>@127.0.0.1:<port>/\n` | `DATABASE_URL=...`, `DB_HOST/PORT/DATABASE/USERNAME`; **no `DB_PASSWORD` line** |
| Valkey | `redis://127.0.0.1:<port>/0\n` | `REDIS_URL=...`, `REDIS_HOST/PORT`; **no `REDIS_PASSWORD` line**, **no userinfo** |

The no-password URI never carries `user:@`, `default:`, or a colon before `@`:
a colon would be read by many clients as an empty password. The `.env` block
omits the password key entirely rather than writing `DB_PASSWORD=` or
`REDIS_PASSWORD=`, for the same reason. The core renders the shape
(`engine::uri::render_no_password` / the mock mirror in `service::mock::uri`);
the CLI prints whatever the service returns, verbatim.

- `list` has no colour option: `format_list_table(instances)` takes no `color`
  parameter. A parameter is added only when a colour path exists.
- `status <name>` exits 0 only when `running`, 3 when missing, 1 otherwise for
  existing non-running instances.
- `start` blocks until healthy or timeout (`--timeout`, default 60 s). It waits
  on the service event stream, not blind sleep-poll loops. Failure prints the DBL
  code, the message and the diagnosis when present; exit 4 for
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

### Verified on the real binary

A real Valkey 9.2.0 instance, its log padded to 16 lines, the shipped binary:

```text
% dblocal logs probe -f --lines 2   # run for 2 s, then SIGINT
exit after SIGINT: 0
lines printed: 2
```

and the reported repro with one line appended while following:

```text
log lines before: 16
exit: 0
printed 3 lines:
pad line 14
pad line 15
append-after-follow
```

Before the fix the same 16-line log printed 2 lines and then 11 more from the
start of the file (13 in total).

## Cross-process status table

`start` and `stop` both see a concrete status, either in their first
`list_instances` or as the `from` of a refused compare-and-set. The loser cases
differ, and treating them alike is what produced a false "already running"
followed by a 60 s wait:

| Command | Concrete status | Exit | Message |
|---|---|---|---|
| `start` | `busy` | 0 after waiting, or 4 on timeout | `already running` |
| `start` | `starting` | 0 after waiting, or 4 on timeout | `already running` |
| `start` | `running` | 0 | `already running` |
| `start` | `stopping` | 1 | `[DBL-1004] another process is stopping it` |
| `stop` | `stopped` | 0 | `already stopped` |
| `stop` | `stopping` | 0 after the wait, 1 on timeout | `[DBL-1004] another process is stopping it`, then `Ready.` |

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
| ---:|---|
| 0 | success / help / version |
| 1 | general error |
| 2 | bad arguments / invalid input / refused confirmation |
| 3 | instance not found |
| 4 | engine failed to start |

## `autostart` (W4-D)

`dblocal autostart status|enable|disable [--json]` manages the per-user login
item. It needs no state database, so the CLI dispatches it before building
`LocalService` — a login item that runs at boot must not fail because the store
could not be opened.

| Command | stdout | stderr |
|---|---|---|
| `autostart status` | `autostart: enabled` or `autostart: disabled` | `location: <path>` |
| `autostart enable` | same, after writing the login item | `location: <path>` |
| `autostart disable` | `autostart: disabled` | `location: <path>` |
| `autostart ... --json` | one line `{"enabled":bool,"location":"..."}` | empty |

Exit codes: 0 success, 2 unknown action, 1 backend failure (the `DBL-85xx` code
is printed on stderr). The mechanism per OS (systemd `--user`, XDG desktop,
LaunchAgent, `HKCU\...\Run`) is documented in `docs/architecture/autostart.md`.

The hidden `autostart-run` subcommand and the raw `--autostart` flag (which the
login item passes) run the login mode: the GUI opens tray-only and starts the
flagged instances, or the CLI starts them headless and exits with a clear
message when nothing needs to run.

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

## W4-A commands: `db`, `user`, `password`, `update`, `open`

W4-A put the admin, update and open-in-client surface behind `DblService` and
added these subcommands (`crates/cli/src/commands/{db,user,password,update,open}.rs`).
The I/O contract is unchanged: stdout is data, stderr the rest, module-local
`DBL-8xxx` codes drive the exit code via `output::report_facing`.

| Command | stdout | stderr |
|---|---|---|
| `db list <instance> [--json]` | the aligned NAME/OWNER/SIZE table, or one JSON array of `{name, owner, size_bytes}` | empty on success |
| `db create <instance> <name>` | nothing | `Database created.` |
| `db drop <instance> <name> [--yes \| --confirm <name>]` | nothing | `Database deleted.` |
| `user create <instance> <name> --db <db> --level ro\|rw` | the generated password, once | `User created. Save this password…` |
| `password reset <instance>` | the new connect URI, once | `Superuser password changed…` |
| `update check [--json]` | the available version or "up to date"; JSON with `--json` | the package-manager hint, when managed |
| `update apply [--yes]` | nothing | `Update installed.` |
| `open <instance> --client <kind> [--dry-run]` | the plan, only with `--dry-run` | the plan header / `Client opened.` |

- **`db drop` typed-name confirmation (`F-DBU-01` AC3).** Without `--yes`, the
  database name must be typed again: `--confirm <name>` must equal `<name>`, or,
  in an interactive session, the prompt reads it. A mismatch is exit 2 with
  `Delete cancelled: the database name does not match.`; a non-interactive session
  with no `--yes`/`--confirm` is exit 2 too.
- **`user create`** prints the password once to stdout and nothing else, like
  `uri`; `--level` accepts only `ro`/`rw` (anything else is exit 2). A failed
  GRANT drops the half-created role and reports the grant error's code.
- **`password reset`** prints the new URI once (re-read from the service after
  the reset), carrying the new password exactly as `F-DBU-03` AC1 requires.
- **`update apply`** requires `--yes` on a non-interactive session (exit 2). A
  package-manager-owned install is refused with `DBL-8008` and the hint line, exit 1.
- **`open --dry-run`** prints the plan (`program:`, `args:`, maybe `url:` and
  `launcher:`) and never a password; without `--dry-run` the client is launched
  through the service.
- **Exit codes.** A module-local `DBL-8101..8111` (admin validation/engine gate)
  maps to exit 2, `DBL-8301` (a detected client vanished) to exit 3, `DBL-8300/8303`
  to exit 2, everything else to exit 1.

## Tests

- Unit: arg parsing, exit-code table, table/JSON formatters, delete confirmation
  logic, secret-never-printed property for non-`uri` commands, the pure
  `new_lines` cases (a)-(e), and the follow command driven by a service that
  scripts one snapshot per read.
- Integration: every command against `dblocal_core::service::mock::MockService`.
  The no-password shape is pinned twice: `create --no-password` stdout is the
  exact no-password URI (not just a prefix), and `uri` / `uri --env` for a seeded
  no-password instance return exit 0 and the exact expected blocks for
  PostgreSQL/MySQL/MariaDB/Valkey. The mock renders through its own
  `service::mock::uri`, so the CLI tests fail if either side drifts back to the
  "credential required" behaviour that produced `DBL-1002` in W3-D round 1.
- Race table: `commands/tests/races.rs` drives a service wrapper for each
  concrete status in the table above, with its own deadline so a command that
  waits for a timeout fails as a red test instead of hanging.
- Binary smoke: `CARGO_BIN_EXE_dblocal` `--version`, `--help`, bad arg exit 2,
  `list`/`list --json` over empty temp `DBLOCAL_HOME`.
- E2e (ignored): real binary, real Valkey lifecycle, `logs -f` history and
  append, temp home, ports 20000-29999, cleanup guard. Run via
  `DBLOCAL_E2E=1 cargo xtask e2e`.

## Mutation checks

Each reversion was applied, the red output pasted, and the file restored
(`git status --short` clean afterwards).

**1. `uri` decorated** — `uri.rs`, `stdout_line(ctx.out, secret.expose())` becomes
`stdout_line(ctx.out, &format!("URI: {}", secret.expose()))`:

```text
% cargo test -p dblocal-cli --lib
test commands::tests::inspect::uri_stdout_is_exactly_the_uri_and_stderr_is_empty ... FAILED
test commands::tests::inspect::uri_env_stdout_equals_the_core_dotenv_block ... FAILED
  left: "URI: postgresql://postgres:pwd-eb731f57@127.0.0.1:5432/postgres\n"
 right: "postgresql://postgres:pwd-eb731f57@127.0.0.1:5432/postgres\n"
  left: "URI: DATABASE_URL=postgresql://postgres:pwd-eb731f57@127.0.0.1:5432/postgres\n..."
 right: "DATABASE_URL=postgresql://postgres:pwd-eb731f57@127.0.0.1:5432/postgres\n..."
test result: FAILED. 89 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out
```

**2. `new_lines` returns the whole window** — `logs.rs`, `&next[overlap(previous,
next)..]` becomes `&next[0..]`:

```text
% cargo test -p dblocal-cli --lib
test commands::logs::tests::a_full_window_that_rotated_reports_only_the_append ... FAILED
test commands::logs::tests::duplicate_lines_are_not_swallowed_below_the_window ... FAILED
test commands::logs::tests::duplicates_filling_the_whole_window_are_indistinguishable ... FAILED
test commands::logs::tests::first_snapshot_prints_only_the_requested_history ... FAILED
test commands::logs::tests::one_appended_line_is_exactly_one_new_line ... FAILED
test commands::logs::tests::poll_reports_retry_and_keeps_the_snapshot ... FAILED
test commands::logs::tests::follow_prints_the_history_plus_one_append_and_nothing_else ... FAILED
test commands::logs::tests::follow_keeps_the_snapshot_across_a_transient_read_error ... FAILED
test result: FAILED. 83 passed; 8 failed; 0 ignored; 0 measured; 0 filtered out
```

**3. `stopping` back in the loser list** — `start.rs`, add `| "stopping"` to the
`InvalidTransition` arm that prints `already running`:

```text
% cargo test -p dblocal-cli --lib
test commands::tests::statuses::start_refused_from_stopping_says_stopping_and_exits_1 ... FAILED
the command did not return within 5s; a timeout was ignored
test result: FAILED. 90 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.01s
```

The deadline is what makes this a five-second red test: under the mutation the
command waits out `--timeout`, and the old version of this test hung the test
binary for seven minutes.

**4. `Lagged` ignored again** — `output.rs`, both `RecvError::Lagged(_)` arms
become `=> {}`:

```text
% cargo test -p dblocal-cli --lib
test output::tests::wait_for_status_rx_recovers_a_dropped_final_event_after_a_burst ... FAILED
test output::tests::wait_operation_rx_resolves_a_lagged_finish_from_the_store ... FAILED
test result: FAILED. 89 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.01s
```

**5. The timeout ignored** — `start.rs`, `Duration::from_secs(timeout.max(1))`
becomes `Duration::from_secs(3600)`:

```text
% cargo test -p dblocal-cli --lib
test commands::tests::races::start_of_an_instance_another_process_is_starting_says_sudah_berjalan ... FAILED
test commands::tests::races::start_busy_loser_reports_sudah_berjalan ... FAILED
the command did not return within 5s; a timeout was ignored
the command did not return within 5s; a timeout was ignored
test result: FAILED. 89 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.01s
```

**6. No-password URI gains a colon** — `uri.rs`, the handler replaces
`redis://127.0.0.1:` with `redis://:@127.0.0.1:` for a no-password Valkey
shape. Both the mock-driven unit test and the real-binary e2e catch it:

```text
% cargo test -p dblocal-cli --lib uri_of_a_no_password_instance_is_the_no_password_shape
test commands::tests::inspect::uri_of_a_no_password_instance_is_the_no_password_shape ... FAILED
  left: "redis://:@127.0.0.1:6379/0\n"
 right: "redis://127.0.0.1:6379/0\n"
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 95 filtered out

% flock -w 3600 "$HOME/.cache/dblocal-e2e.lock" bash -lc \
    'DBLOCAL_E2E=1 cargo test -p dblocal-cli --test cli_e2e \
     no_password_instance_end_to_end -- --ignored --nocapture'
test no_password_instance_end_to_end ... FAILED
assertion `left == right` failed: uri stdout
  left: "redis://:@127.0.0.1:24850/0\n"
 right: "redis://127.0.0.1:24850/0\n"
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1 filtered out
```

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
| 1 | 15 passed, 82.8 s | 1 passed, 4.9 s | every target passed |
| 2 | 15 passed, 86.3 s | 1 passed, 4.6 s | every target passed |
| 3 | 15 passed, 86.2 s | 1 passed, 4.8 s | every target passed |

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
