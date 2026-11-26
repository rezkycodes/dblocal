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

The unit binds to the **graphical session**, not `default.target` (fix D3):
`After=graphical-session.target`, `PartOf=graphical-session.target`,
`WantedBy=graphical-session.target`. A unit on `default.target` starts at the
first login session, before the compositor imports `DISPLAY`/`WAYLAND_DISPLAY`
into the user manager, so the app would take the headless path instead of the
tray path `F-AUT-01` AC3 wants. On a session where the display is imported (as
here: `systemd-run --user --wait --collect -P env` shows `DISPLAY=:0`,
`WAYLAND_DISPLAY=wayland-0`, and `systemctl --user is-active
graphical-session.target` is `active`), the graphical-session unit runs with the
tray available.

**Caveat (documented, not fixed):** a window manager that never activates
`graphical-session.target` (plain i3/sway without systemd session integration)
will not run the unit. On those, the XDG `.desktop` fallback is the portable
mechanism — but today that fallback is chosen only when no user systemd manager
exists at all. A future task may broaden the choice; for now the limitation is
stated here.

**Failed enable is rolled back (fix D2).** `enable` records the unit file's
previous content before writing. If `daemon-reload` or `enable` fails, it
restores the previous content (or removes a unit this call created) and reports
the original error, so a half-written unit cannot make `status` claim the login
item exists. `status` counts the systemd unit as enabled only when a `.wants`
symlink points at it (`graphical-session.target.wants/` or the legacy
`default.target.wants/`); a bare unit file is reported `enabled: false`.

**Removal (`F-AUT-01` AC2).** `disable` deletes every file or value the backend
created: the unit, its `.wants` symlink in either target (only a symlink whose
target is our unit, never a neighbour), the desktop entry, the plist, or the
`Run` value. It is idempotent: removing an absent item is a success.

## `--autostart` mode (`F-AUT-01` AC3)

The login item passes `--autostart`. `cli::run` normalizes that token into the
hidden `autostart-run` subcommand before parsing.

### Headless CLI path (no display — the boot case, fix D1)

Without `DISPLAY`/`WAYLAND_DISPLAY` (exactly a systemd user unit at boot before
`import-environment`), the CLI runs `mode::run_pass` against a real service and
**waits for every flagged start to finish** before returning:

- `AutostartService::start` requests the start; `start_targets` then calls
  `wait_started(id, bound)` inside the same worker, so the waits run
  concurrently and one hanging engine never serializes the rest.
- `bound = engine start timeout + AUTOSTART_START_MARGIN_SECS` (15 s), both
  values from `consts` — never an invented number.
- One stderr line per flagged instance: `autostart: <name> started` or
  `autostart: <name> failed to start — [DBL-xxxx] …`.
- Exit 0 only when every flagged instance started; exit 1 otherwise. This is the
  visible failure channel for a login item: there is no modal and no tray at
  boot, so `systemctl --user status dblocal` (and the journal) show the failure.

A one-line note `Autostart: the tray is unavailable; the instances keep running without a
window.` is written once, then the process exits — but only *after* the engines
are up, so the detached engines outlive it.

### GUI path (a display is present)

`gui::run_autostart()` starts the flagged instances through the same
`start_targets` (which waits per instance), then presents itself through the
pure `decide_gui_presentation(tray_available)` (fix D4):

| Tray host | Decision | Result |
|---|---|---|
| present | `TrayOnly` | no window; the tray is the UI (AC3) |
| absent | `Window` | open the main window (PRD `F-TRY-01` AC3 fallback) |

The decision is written to stderr (`journalctl --user -u dblocal` shows it), so
a login-mode run is diagnosable even before a tracing subscriber exists. A
missing tray no longer leaves an invisible ~170 MB process that nothing can quit.

## Instance autostart (`F-AUT-02`)

`plan::flagged_targets` selects only instances with `autostart == true` whose
status is `Stopped` or `Failed` — a running or transitioning instance is left
alone so a double login cannot race a live engine. Each target carries its
engine so `start_wait_bound` can pick the right timeout.

`runner::run_start_targets` fans the starts out to at most
`consts::AUTOSTART_CONCURRENCY` (4) workers, one target per worker iteration:

- one failure is recorded in the summary and **never blocks the rest** (AC2);
- there is **no modal dialog**: a failed start becomes the `failed` status the
  tray and window already render, carrying the usual `DBL-1005`/diagnosis.

`mode::run_pass` reads `list_instances`, applies the plan, runs the fan-out and
returns the decision together with one outcome per target in input order. The
fan-out **waits** (fix D1): `start_instance` returns an `OperationId`
immediately and the engine comes up asynchronously, so the pass polls the store
until each instance leaves `starting`. A failed instance is reported with the
stored `DBL-xxxx` code; a wait that exceeds its bound reports `DBL-8401`.


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
- **Windows:** the `Run` value path is unit-tested through `reg.exe` quoting on
  Linux. The real Windows backend compiles for `--target x86_64-pc-windows-gnu`:
  a minimal probe crate that includes the unchanged `backend.rs`, `windows.rs`
  and `macos.rs` builds for that target (the full workspace `cargo check
  --target x86_64-pc-windows-gnu` cannot run because `aws-lc-sys` needs a mingw C
  compiler this host does not have — the same pre-existing limitation as
  `docs/architecture/process.md`). A real registry round trip is listed in
  `docs/dev/windows-verification.md`.
- **Real `systemctl --user enable`:** the reviewer performs one real
  enable/disable round trip and `systemd-analyze verify` on a generated unit; the
  worker never runs it during development (task rule).

## Recorded e2e run (2026-11-22, Fedora, Linux x86_64)

```text
% flock -w 3600 "$HOME/.cache/dblocal-e2e.lock" DBLOCAL_E2E=1 \
    cargo test -p dblocal-core --test service_e2e -- --ignored --test-threads=1 \
    --nocapture autostart_two_flagged_one_blocked
auto-blocked failed with [DBL-1005] That port is already used by another process. (PortInUse)
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 15 filtered out; finished in 2.13s
```

Exit code 0; no `postgres` process from the temp home survived.

## Fix round 1 (D1–D5) — real-binary evidence (2026-11-23)

Re-verified on the merged head (main + W4-D, merge `59798f6`) with the shipped
binary; the `client`/`db`/`user` W4-A commands are present alongside
`autostart`.

### D1 red first (unit test on the pre-fix head)

```text
% cargo test -p dblocal-core --lib autostart::mode::tests::run_pass_waits_for_every_start_to_finish
test autostart::mode::tests::run_pass_waits_for_every_start_to_finish ... FAILED
panicked at crates/core/src/autostart/mode.rs:227:9:
run_pass returned before the start operation finished (D1)
```

### D1 green, one flagged instance, no display (real binary)

Isolated `DBLOCAL_HOME`, one valkey instance flagged `autostart`, status
stopped. Before the fix: exit 0 in 0 s, row left `starting`, no process.

```text
% env -u DISPLAY -u WAYLAND_DISPLAY dblocal --autostart
autostart: instance started: vk-auto
Autostart: the tray is unavailable; the instances keep running without a window.
exit=0 elapsed=0.37s
store: [('vk-auto', 'running', 3241730)]
LISTEN 127.0.0.1:20501   LISTEN [::1]:20501
```

### D1 + F-AUT-02 AC2, two flagged, one port held (real binary)

Two flagged valkey instances; a foreign listener holds `20502`.

```text
% env -u DISPLAY -u WAYLAND_DISPLAY dblocal --autostart
autostart: instance started: vk-auto
autostart: instance failed to start: vk-blocked — [DBL-1005] That port is already used by another process.
Autostart: the tray is unavailable; the instances keep running without a window.
exit=1 elapsed=0.32s
store: [('vk-auto', 'running', 3278370), ('vk-blocked', 'failed', None)]
valkey: 3278370 .../valkey-server 127.0.0.1:20501
20501 listening: yes
```

The free instance is up and listening; the blocked one is `failed` with the
`DBL-1005` port diagnosis; exit 1 so `systemctl --user status dblocal` shows the
failure. No modal dialog.

### D3 unit verify + what a unit really sees (real systemd user manager)

Generated with the CLI into a temp config root (fake `systemctl` so no real
enable ran), then verified on a copy:

```text
% systemd-analyze --user verify /tmp/w4d_d3/verify.service
(no output)  exit=0
```

```text
% systemd-run --user --wait --collect -P env | grep -E 'DISPLAY|WAYLAND'
DISPLAY=:0
WAYLAND_DISPLAY=wayland-0
% systemctl --user is-active graphical-session.target
active
```

A graphical-session unit therefore starts in a session that already has the
display, so the tray path is reachable (D3).

### D4 both branches (real binary, Xwayland)

Private session bus (no StatusNotifierWatcher):

```text
Autostart: the tray is unavailable, the window was opened.
windows: 0x01000001   (a mapped window exists)
```

Real session bus (tray host present):

```text
Autostart: tray active, no window.
windows: (no dblocal window)
```

### D5

`resolve_login_exe_from` prefers a real `$APPIMAGE` file over `current_exe()`;
unit-tested through the seam. **Not run on a real AppImage** — none available
here.

### Mutation checks for the fix round (red then restored, `git status` clean)

**D1 — do not wait for the start** (`mode.rs::start_targets` drops the
`wait_started` call):

```text
% cargo test -p dblocal-core --lib autostart::mode::tests::run_pass_waits_for_every_start_to_finish
test autostart::mode::tests::run_pass_waits_for_every_start_to_finish ... FAILED
panicked at crates/core/src/autostart/mode.rs:334:9 (run_pass returned before the start finished)
```

**D2 — no rollback and status ignores the wants symlink** (`linux.rs`):

```text
% cargo test -p dblocal-core --lib autostart::linux::tests::a_failed_enable_rolls_back_a_newly_written_unit
test ... FAILED  panicked at crates/core/src/autostart/linux.rs:674:9
% cargo test -p dblocal-core --lib autostart::linux::tests::status_is_not_enabled_for_a_unit_without_a_wants_symlink
test ... FAILED  panicked at crates/core/src/autostart/linux.rs:723:9
```

**D4 — always tray-only, never a window** (`plan.rs::decide_gui_presentation`):

```text
% cargo test -p dblocal-core --lib autostart::plan::tests::gui_presentation
test autostart::plan::tests::gui_presentation_without_a_tray_opens_a_window ... FAILED
assertion `left == right` failed
  left: TrayOnly
 right: Window
```



