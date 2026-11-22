# W4-D fix 1 — the login pass never finishes a start, and four smaller defects (reviewer ran the real binary)

Same branch `rezkycodes/w4-autostart`, same worktree. Rules `docs/orchestration/tasks/_COMMON-RULES.md` incl. 11-14. FIRST `git merge main` (main moved a lot since your base: W2-A flake fixes, W3-B2 MariaDB, W2-B). Do not touch `dblocal-gpui-kit-source/`.

## What the reviewer verified and accepts (do not redo)

fmt, clippy (also `--features demo`), deny, core lib x3 (1201 passed), autostart tests x40 from a private copy of the test binary (40/40, 61 distinct passed counts), cli tests x3, gui lib x3 (232). Real binary on Linux with an isolated config root and the real user systemd: `dblocal autostart enable/status/disable` round trip works (unit written, `systemd-analyze --user verify` clean, `systemctl --user is-enabled` = enabled, `disable` removes the unit and the wants symlink, nothing left). GUI `--autostart` under Xwayland with a tray host: no window, tray menu shows the flagged instance `running` (engine up, store says Running), tray Quit reached the quit dialog.

## D1 (HIGH, blocks the merge) — the headless login pass exits before any start completes

Reproduction (real binary, no `DISPLAY`/`WAYLAND_DISPLAY`, isolated `DBLOCAL_HOME` with one valkey instance flagged autostart, status stopped):

```
env -u DISPLAY -u WAYLAND_DISPLAY DBLOCAL_HOME=... dblocal --autostart
-> stderr: "Autostart: tray tidak tersedia; instance tetap berjalan tanpa jendela."   exit 0 after 0 s
-> 4 s later: no valkey-server process, port not listening, the store row is `starting` with pid NULL
```

Cause (read it, confirm it): `AutostartService::start` for `Arc<dyn DblService>` calls `start_instance(id).map(|_op| ())`: it only requests the start and discards the operation id (`crates/core/src/autostart/mode.rs`); `report_pass` (`crates/cli/src/commands/autostart.rs`) prints the message and returns, the process exits, the service runtime is dropped and the in-flight start dies with it. The message is false: nothing was started. This is exactly the path a systemd user unit takes at boot or login when the display variables are not imported yet, so F-AUT-02 AC1 would not work there.

Do:
1. Red first: a unit test with a service double whose `start_instance` returns an operation that finishes only later (poll `operation_result` or subscribe to `OperationFinished`): `run_pass`/the headless path must not return before every started operation has finished, and a failed operation must be reported per instance. Paste the red output on the current head.
2. Fix: the headless pass waits for each operation with a bound (take the engine start timeout from `crates/core/src/consts.rs` plus a margin, do not invent a number), keeps the instances independent (one failure never blocks another; do not serialize them), prints one stderr line per flagged instance with the outcome (started / failed with the `DBL-xxxx` code), and exits 0 only when every flagged instance started (non-zero otherwise, so `systemctl --user status dblocal` shows it; say in the docs that this is the visible failure channel since there is no modal and no tray).
3. The GUI path must do the same observation per instance so the tray/window shows `failed` (it does show running today; check that a start that FAILS shows `failed` with no modal dialog, F-AUT-02 AC2, with a real port conflict: bind the flagged instance's port with `python3 -m http.server` or `nc -l` first).
4. Real binary, both paths, with two flagged instances where one has its port occupied: the other still comes up (running, identity recorded, listening), the failed one is `failed` and a log tail exists. Paste the outputs. Clean up engines afterwards (`pgrep -x valkey-server`).

## D2 (MEDIUM) — a failed `enable` leaves the unit behind and `status` then says enabled

Reproduction: `HOME=/tmp/x XDG_CONFIG_HOME=/tmp/x/.config dblocal autostart enable` (the user manager does not see that dir) -> `[DBL-8502] Perintah sistem ... gagal` but `/tmp/x/.config/systemd/user/dblocal.service` stays and `dblocal autostart status --json` says `{"enabled":true,...}`. `LinuxBackend::status` reports `enabled` from `unit_path.exists()` alone, and `enable` has no rollback after `write_unit` (`crates/core/src/autostart/linux.rs:88-109`).
Do: on a failing `daemon-reload` or `enable`, remove the unit file this call wrote (only if it did not exist before; restore the previous content otherwise) and report the original error; and make `status` count the systemd unit as enabled only when the wants symlink exists (the dir is already known to the backend: `wants_dir`). Tests with the fake runner for both. Red first.

## D3 (MEDIUM, decide with evidence) — `WantedBy=default.target` starts a GUI app before the session has a display

The generated unit is `[Install] WantedBy=default.target`. A user unit on `default.target` starts at the first login session, before the compositor has imported `DISPLAY`/`WAYLAND_DISPLAY` into the user manager environment (`systemctl --user import-environment`), so `gui_unavailable_reason()` is true and the process takes the headless path (D1) instead of the tray path F-AUT-01 AC3 wants. Reviewer recommendation: `After=graphical-session.target`, `PartOf=graphical-session.target`, `WantedBy=graphical-session.target`. Caveat to document: window managers that never activate `graphical-session.target` (plain i3/sway without systemd integration) will not run it; the XDG `.desktop` fallback is the more portable mechanism there, and today the fallback is chosen only when no user systemd manager exists.
Do: show with `systemd-analyze --user verify` that your final unit is valid, print `systemctl --user show -p Environment`/`show-environment | grep -E 'DISPLAY|WAYLAND'` from inside a `systemd-run --user --wait --collect -P env` (what a unit really sees here), say what you changed and why, update `docs/architecture/autostart.md`. If you keep `default.target`, justify it with evidence and make D1 carry the whole boot case.

## D4 (MEDIUM) — display present but no tray host: an invisible 170 MB process that never exits

Reproduction: `env -u WAYLAND_DISPLAY DISPLAY=:0 dbus-run-session -- dblocal --autostart` (a private session bus has no StatusNotifierWatcher): the process stays alive forever with no window and no tray (RSS 173 MB at 15 s), the flagged instance is up, and nothing the user can click quits it. PRD F-TRY-01 AC3: "tray tidak tersedia ... aplikasi tetap berfungsi penuh lewat jendela". Do: when the tray cannot be installed, the login-mode GUI process must open the main window (the PRD fallback) instead of staying invisible; keep the no-window behaviour only when the tray is really there. Test the decision (pure function) and show the real run.

## D5 (LOW) — the login item points at `current_exe()`

`crates/cli/src/commands/autostart.rs:42` registers `std::env::current_exe()`. For an AppImage that is a path inside the transient squashfs mount (`/tmp/.mount_...`), which no longer exists after the app exits, so the login item would be dead. Use `$APPIMAGE` when set (and the file exists), otherwise `current_exe()`. Test with the env var injected through a seam, no real AppImage needed; say plainly in your report that you could not run a real AppImage.

## Not in this round

The GUI toggle for the login item (Config is a placeholder until F-CFG), Windows/macOS (cannot be run here), the debug-build a11y duplicate-node panic when the tray Quit opens the quit dialog (that is W2-B2, known; `cfg(debug_assertions)` only).

## Bar

fmt, clippy workspace (also `--features demo`), deny, the new tests x100, `cargo test -p dblocal-core --lib` and `-p dblocal-cli` x3, autostart tests x40 from a PRIVATE COPY of the test binary (copy it out of the shared `CARGO_TARGET_DIR` right after `cargo test --no-run`, print the distinct passed counts), `cargo xtask ci` under the heavy lock on the final head (`CARGO_BUILD_JOBS=8`, `MemAvailable` above 8000 MB: other workers link GUI test binaries at the same time). Known on main (do not chase): `dblocal stop` is sometimes refused as busy by startup reconciliation until W2-A flake 6 item 0 lands; retry in your own scripts, or do not stop engines through the CLI (kill the process you started instead). `worker_done` only with the red and green outputs pasted and the real-binary transcripts for D1 and D4.
