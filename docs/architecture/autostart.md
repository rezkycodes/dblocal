# Autostart (W4-D)

This document describes the login-item backends and the `--autostart` mode
implemented in `crates/core/src/autostart` and wired through the CLI and the GUI
bootstrap. It covers PRD `F-AUT-01` (app starts at login) and `F-AUT-02`
(chosen instances start at login).

## Scope and ownership

| File | Role |
|---|---|
| `crates/core/src/autostart/mod.rs` | Wiring, `host_backend`, home/config resolution |
| `autostart/backend.rs` | `AutostartBackend`, `CommandRunner`, `LoginItem`, quoting helpers |
| `autostart/linux.rs` | `systemd --user` unit + XDG desktop fallback |
| `autostart/macos.rs` | LaunchAgent plist |
| `autostart/windows.rs` | `HKCU\...\Run` value |
| `autostart/plan.rs` | Pure decisions: `decide_mode`, `flagged_targets` |
| `autostart/runner.rs` | Bounded, independent start fan-out |
| `autostart/mode.rs` | `run_pass`: plan + service |
| `autostart/error.rs` | `DBL-8500..8505`, Indonesian messages in `messages.rs` |
| `crates/cli/src/commands/autostart.rs` | `dblocal autostart status\|enable\|disable` and the login mode |
| `crates/gui/src/app/bootstrap.rs` | `run_autostart_with_service`: tray-only login path |
| `crates/gui/src/lib.rs` | `run_autostart()` |

## Login item per OS (`F-AUT-01` AC1)

Every backend implements `AutostartBackend` (`enable`, `disable`, `status`) and
receives a `CommandRunner` and an injected config root, so the whole surface is
unit-tested on Linux with a temp root and a fake runner. **Nothing here writes
to the user's real home in a test**, and the development process never runs the
real `systemctl --user enable`, `launchctl`, or `reg`.

The command is always the absolute path of the running binary, quoted for the
target OS, plus `--autostart` (`consts::AUTOSTART_FLAG`):

| OS | Mechanism | Location | Quote form |
|---|---|---|---|
| Linux (systemd) | `systemd --user` unit, `systemctl --user enable` | `$XDG_CONFIG_HOME/systemd/user/dblocal.service` | `ExecStart="<abs path>" "--autostart"` |
| Linux (no user systemd) | XDG desktop entry | `$XDG_CONFIG_HOME/autostart/dblocal.desktop` | `Exec="<abs path>" "--autostart"` (`%` doubled) |
| macOS | LaunchAgent plist, `launchctl bootstrap gui/<uid>` | `~/Library/LaunchAgents/dev.dblocal.DBLocal.plist` | `<string>` with XML escaping |
| Windows | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\DBLocal` | registry value via `reg.exe` | CommandLineToArgvW quoting |

The Linux backend asks `systemctl --user show-environment` whether a user
manager answers; only then does it write the unit and `enable` it. Otherwise it
writes the desktop entry. When systemd becomes available later, `enable`
removes a stale desktop entry so the app cannot start twice.

**Removal (`F-AUT-01` AC2).** `disable` deletes every file or value the backend
created: the unit, its `default.target.wants` symlink (only a symlink whose
target is our unit, never a neighbour), the desktop entry, the plist, or the
`Run` value. It is idempotent: removing an absent item is a success.

## `--autostart` mode (`F-AUT-01` AC3)

The login item passes `--autostart`. `cli::run` normalizes that token into the
hidden `autostart-run` subcommand before parsing.

`gui::run_autostart()` (used when a display is available) and the headless CLI
path both decide presentation through the pure `decide_mode(tray_available,
flagged_instances)`:

| Tray | Flagged instances | Decision | Result |
|---|---|---|---|
| yes | any | `TrayOnly` | straight to the tray, no window |
| no | ≥ 1 | `HeadlessKeepAlive` | start them, keep the process alive headless |
| no | 0 | `HeadlessExit` | exit with `MODE_EXIT_NO_INSTANCES` |

The decision is a pure function (`plan::decide_mode`) tested without a tray or
engine. A login item never surprises the user with a window.

## Instance autostart (`F-AUT-02`)

`plan::flagged_targets` selects only instances with `autostart == true` whose
status is `Stopped` or `Failed` — a running or transitioning instance is left
alone so a double login cannot race a live engine.

`runner::run_start_targets` fans the starts out to at most
`consts::AUTOSTART_CONCURRENCY` (4) workers, one target per worker iteration:

- one failure is recorded in the summary and **never blocks the rest** (AC2);
- there is **no modal dialog**: a failed start becomes the `failed` status the
  tray and window already render, carrying the usual `DBL-1005`/diagnosis.

`mode::run_pass` reads `list_instances`, applies the plan, runs the fan-out and
returns the decision together with one outcome per target in input order. The
service call (`start_instance`) returns an `OperationId` immediately; the
eventual success or failure reaches the tray and window through the normal
`ServiceEvent`s, so the pass never waits on an engine.

## CLI

```
dblocal autostart status [--json]
dblocal autostart enable [--json]
dblocal autostart disable [--json]
```

`status`/`disable` print `autostart: aktif` / `autostart: nonaktif` on stdout
and `lokasi: <path>` on stderr; `--json` prints one line
`{"enabled":bool,"location":"..."}`. Exit codes follow `docs/architecture/cli.md`:
0 success, 2 unknown action, 1 backend failure (with the `DBL-85xx` code on
stderr). `autostart` needs no state database, so the CLI dispatches it before
building `LocalService`; a login item must not fail because the store could not
be opened.

## Errors

`DBL-8500..8599` is the reserved "packaging and autostart helpers" range
(`docs/architecture/error-codes.md`). Messages are Indonesian and live in
`autostart/messages.rs`; `Debug` and `Display` never carry a path or helper
stderr that could hold a secret.

## Tests

- Unit (no engine): every backend against a fake `CommandRunner` and a temp
  root; quoting for spaces, `%`, `&`, embedded quotes; `decide_mode` and
  `flagged_targets` tables; the bounded fan-out (order, independence,
  concurrency bound, fail-fast concurrency proof).
- CLI: `run_action_with` driven by a fake backend for the enabled/disabled/
  failing/unknown paths and the JSON shape; parse tests for the subcommands and
  the `--autostart` normalization.
- GUI lib: the existing bootstrap suite still passes with the login mode added.
- E2e (`#[ignore]`, run under the e2e lock, ports 20000-29999):
  `autostart_two_flagged_one_blocked` starts two flagged PostgreSQL instances,
  holds one's port with a foreign listener, and asserts the free one reaches
  Running while the blocked one ends Failed with `DBL-1005` — the other start is
  not blocked.

## Mutation checks

Each reversion was applied, the red output pasted, the file restored
(`git status --short` empty afterwards).

**1. Leave files behind after disable** — `linux.rs`, drop the
`remove_wants_symlink`/`remove_if_present` calls in `disable`:

```text
% cargo test -p dblocal-core --lib autostart::linux::tests::disable_removes
test autostart::linux::tests::disable_removes_the_unit_the_wants_link_and_the_desktop_file ... FAILED
panicked at crates/core/src/autostart/linux.rs:470:9: unit left behind
```

**2. Skip quoting a path with spaces** — `backend.rs::quote_systemd_exec`
returns the raw path when it contains a space:

```text
% cargo test -p dblocal-core --lib autostart::backend::tests::systemd_exec_quotes
test autostart::backend::tests::systemd_exec_quotes_a_path_with_spaces ... FAILED
assertion `left == right` failed
  left: "/opt/My App"
 right: "\"/opt/My App\""
```

**3. Start instances serially** — `runner.rs::consts_workers` returns 1:

```text
% cargo test -p dblocal-core --lib autostart::runner::tests::starts_are_concurrent_not_serial
test autostart::runner::tests::starts_are_concurrent_not_serial ... FAILED
assertion `left == right` failed: both starts must be in flight at once
  left: 0
 right: 2
test result: FAILED. 0 passed; 1 failed ... finished in 4.01s
```

The bounded 2 s deadline is what makes this a four-second red test rather than a
deadlocked barrier.

**4. Show a dialog / abort on failure** — `runner.rs`, after the joins, return
`AutostartError::PlanFailed` when any outcome is an error:

```text
% cargo test -p dblocal-core --lib autostart::runner::tests::one_failure_does_not_block
test autostart::runner::tests::one_failure_does_not_block_the_others ... FAILED
panicked at crates/core/src/autostart/runner.rs:182:58: run: PlanFailed { .. }
```

## Unverified

- **macOS:** cannot be built or run here; the LaunchAgent code is compile-checked
  logic only and is exercised on Linux against a temp home and a fake runner.
- **Windows:** the `Run` value path is compile-checked with
  `--target x86_64-pc-windows-gnu` and unit-tested through `reg.exe` quoting;
  a real registry round trip is listed in `docs/dev/windows-verification.md`.
- **Real `systemctl --user enable`:** the reviewer performs one real
  enable/disable round trip and `systemd-analyze verify` on a generated unit; the
  worker never runs it during development (task rule).
