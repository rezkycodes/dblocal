# W2-B fix round 2 — what the reviewer found by reading your Q1-Q5 code, and the gates you left open

Same branch `rezkycodes/w2-gui-wiring`, same worktree. Rules: `docs/orchestration/tasks/_COMMON-RULES.md` (rules 10-14). Original round: `tasks/W2-B-fix-1.md` (still binding for anything not changed here). You own `crates/gui/**`, `docs/architecture/gui.md` and the GUI strings in `crates/core/src/strings.rs` (additions only).

You ended your turn with a list headed "Not verified" (render x10, `xtask ci`, 40x lib loop, coverage, a clean mutation rerun) and no `worker_done`. Orca does not wake an idle worker, so that report sat unread for an hour. **Never end your turn with work open.** `worker_done` only with pasted evidence for every item of the bar; if something is blocked (heavy lock, load), send `ask` and keep working on the unblocked parts, with background jobs and bounded `until` loops.

Before anything else: your old background jobs (the 40x lib loop, a queued `cargo xtask ci` under the heavy lock) test a tree you are about to change. Find them with `ps -eo pid,etime,args | grep -E '[c]argo|[f]lock'`, kill only YOUR OWN, and rerun everything on the final tree.

Verified by the reviewer by reading `git diff 2e5222b..HEAD -- crates/gui` at `01cafe0`: the Q1 core works (acceptance no longer quits; `OperationFinished` drives `StopFinished`; bounded wait from `STOP_GRACE_SECS`), Q3 is platform-gated through pure functions that take the OS as a parameter, the welcome/select-instance split and the status bar use the real home. Do not rework those. The defects:

## R1 (HIGH) — tray "Keluar" with no window quits without asking, whatever is running

`bootstrap.rs::handle_app_tray_action` for `TrayAction::Quit` with no window opens a NEW `Workspace` and calls `request_quit` on it at once. A fresh `Workspace` has `state.instances == []` (the startup load is asynchronous), so `running_instance_ids` is empty, `QuitMachine` goes `Idle -> Done` and `cx.quit()` runs. Result on the real app: close the window (tray stays), PostgreSQL is running, choose "Keluar" in the tray: the app exits silently and the user never sees the "Biarkan tetap berjalan / Hentikan semua" choice. F-TRY-02 AC2 is only met while a window is open.

Fix: with no window, read the instance list from the service on the background executor (`DblService::list_instances`, never on the GPUI thread), then open the window and run the quit request with THAT list (`request_quit_with(running)` or equivalent), so the dialog shows when anything is running and the app exits at once only when nothing is. A failed list read must not exit: show the window with the error. Tests (red first, paste the red run): window closed + one `Running` instance in the service + tray Quit -> a window exists, quit state is `Confirming`, `cx.quit` was NOT called; window closed + nothing running -> the app quits; window closed + `list_instances` returns `Err` -> no quit, a window with the error. Mutation: call `request_quit` on the empty fresh state again: the first test goes red.

## R2 (HIGH) — a stop that finishes before it is registered is lost, quit then waits for the timeout

`quit_flow.rs`: the stop is registered in `quit_stops` in the continuation that runs after `stop_instance` returns `Ok(op)`; `on_quit_stop_finished` drops any `OperationFinished` whose id is not registered (`let Some(id) = self.quit_stops.remove(op) else { return }`). `OperationFinished` travels bridge thread -> flume -> event pump, the continuation travels the background executor -> GPUI thread: nothing orders them. A stop that is fast (an instance that was already stopped, an engine that exits at once, a loaded machine where the GPUI thread is busy) can finish first: the event is discarded, the machine stays in `StoppingAll` and ends in `StopFailed` after `STOP_GRACE_SECS + 10` s although every engine is down. Same class as the "subscribe after start" flake the other worker is fixing in core.

Fix: remember the outcome of an operation that finishes while the quit flow is in `StoppingAll` and whose id is not registered yet (bounded map, cleared when the flow leaves `StoppingAll`), and consult it when registering. Tests (red first): a service wrapper whose `stop_instance` sends `OperationFinished` for the id on the event channel BEFORE it returns the id: quit completes without waiting for the timeout; the same with an error result -> `StopFailed`; two instances, one finishing before registration and one after -> `Done` only when both are in. Mutation: drop the buffer -> the first test goes red. Also drive at least one of these tests through the real `spawn_bridge` and a real broadcast channel instead of calling `apply_event_with_cx` by hand, so the ordering path is the production one.

## R3 (MEDIUM) — two event bridges, and an unbounded backlog while the window is closed

`AppRuntime::new` calls `spawn_bridge(service)` into `AppRuntime.events`; `run_with_service` then calls `spawn_bridge(service_for_tray)` a SECOND time for the tray refresh. While no window is open nothing receives from `AppRuntime.events`; it is an unbounded flume channel, so every service event (progress, state changes, log-related events) piles up for as long as the window stays closed, and the next window replays all of it. A tray-resident app is exactly the case where the window is closed for days.

Fix: ONE bridge. The app-level loop is its only consumer: it refreshes the tray, forwards each event to the live `Workspace` when there is one, and drops it when there is none (a new window reads its state from the service in `startup_load`, it needs no replay). Tests: with no window, push 10 000 events through the bridge and assert nothing accumulates (queue length stays bounded, assert the number); a fake service that counts `subscribe` calls sees exactly one; a window opened after a burst of events does not replay them (its state equals the service's list).

## R4 (MEDIUM) — the Q2 "tests" test nothing

`window_close_decision_is_tray_aware` asserts that `window_close_should_close(true)` is `true`; `existing_window_is_focused_not_duplicated` asserts that a function returning its argument returns it. A mutation that makes the real close handler return `false` with a tray (the Q2 defect itself) or that makes "Buka DBLocal" open a second window passes both. The spec asked for headless tests of the behaviour. Delete the two tests and write these (extract the body of the `on_window_should_close` closure and `handle_app_tray_action` into functions that take `&mut App` so a test can call them; give `AppRuntime` a test constructor with a fake tray that reports available / unavailable):

1. tray available + close request -> the window is actually closed (the window list is empty), `cx.quit` was not requested, `runtime.workspace` and `runtime.main_window` are `None`, the service still answers.
2. tray unavailable + close request, nothing running -> quit; with something running -> the quit dialog and the window stays.
3. tray "Buka DBLocal" with a window open -> still exactly one window, it was activated; with no window -> exactly one new window whose `Workspace` shows the instances of the service.
4. tray Start/Stop with no window reaches the service (the fake counts calls).

Mutations to paste (red, restored): `on_window_should_close` returns `false` with a tray (1 red); `open_main_window` always opens (3 red); the tray-unavailable branch skipped (2 red).

## R5 (MEDIUM) — errors swallowed in the window-less tray path

`handle_app_tray_action`: `let _ = svc.start_instance(&id)` / `let _ = svc.stop_instance(&id)` discard the result and an unparsable id is ignored silently. With no window there is nowhere to show an error, but a tray click that does nothing and logs nothing is undiagnosable. Log every failure with `tracing::warn!` (error code and instance id, never a secret), and when a window is open route it through the existing `report_error`/toast path. Test: a failing service -> the warning is emitted (capture with `tracing-test` or an equivalent, say which) and, with a window, the toast is set.

## R6 (LOW) — dead code and fabricated state left over from the move to an app-level tray

`Workspace.tray_actions` (`#[allow(dead_code)]`), `start_tray_pump` (`#[allow(dead_code)]`), the fake `TrayStartup` built in `build_live`/`build_preview`, `tray_available_cached` next to a `tray: TrayHandle::unavailable()` that is never the real one. Remove them; the `Workspace` reads tray availability from the `AppRuntime` (or from a plain `bool` passed in) and nothing else. `#[allow(dead_code)]` must not remain anywhere in `crates/gui`: `grep -rn 'allow(dead_code)' crates/gui` pasted empty (or each remaining hit justified in one line).

## R7 (LOW) — the status-bar size is computed once

`refresh_home_size_bg` is called only from `startup_load`. After creating an instance (hundreds of MB of runtime), deleting one or purging a runtime the bar keeps the startup number. Recompute off the GPUI thread after `OperationFinished` and `InstanceRemoved`, coalesced (at most one walk in flight, at most one per 5 s; a constant, not a literal). `dir_size_bytes` must not follow symlinks (it uses `DirEntry::metadata`, which does not, but nothing pins it): a test with a symlink to a parent directory (a loop) and a symlink to a big file outside the home must count neither. Test the refresh: temp home, start, write a file, finish an operation, the state updates.

## R8 (LOW) — Q5.1 is only half done

`status_dot_argb` exists, but it is used only for a constant grey `Stopped` `icon_pixmap` of the tray icon; the menu items still carry text glyphs, and `icon_name` still returns the theme icon (SNI hosts prefer the name over the pixmap). F-TRY-01 AC1 asks for green/grey/red dots. Implement: `icon_data` on every `StandardItem` from the status of its instance, and a tray icon that reflects the aggregate (any `Failed` -> red, else any `Running` -> green, else grey) via `icon_pixmap` with an empty `icon_name`. Pure functions with tests (aggregate decision per case; the bytes per status are ARGB in network order: assert alpha first). The reviewer reads the real D-Bus layout (`GetLayout`) for `icon-data`. If the DBLocal brand icon exists in `crates/gui/assets`, say which file and why it is or is not used.

## Q3 follow-up — paste the grep

You reported no other Linux-only startup assumption. Paste the exact command and every hit for `grep -rnE '/usr|/etc|/proc|XDG_|"HOME"|std::env::var\("HOME' crates/gui crates/cli crates/core/src --include=*.rs` outside `#[cfg(test)]` and outside `cfg(target_os = "linux")` blocks, each classified: runs on every OS and is a defect / Linux-gated / test only. Fix the first kind or list it in `gui.md`.

## Bar

On the FINAL tree, with the load of every run pasted: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `-p dblocal-gui --features demo`, `cargo deny check`, `cargo test -p dblocal-gui --lib` x40 (every failure named with its load), `cargo test -p dblocal-gui --test render` x10, `cargo xtask ci` under `flock -w 7200 $HOME/.cache/dblocal-heavy.lock` after waiting for load < 8, coverage of every file you touched at 80% or more and pure logic at 90% or more (`CARGO_LLVM_COV_TARGET_DIR=$HOME/.cache/dblocal-target-cov-w2b`, delete afterwards), the Q1 mutation rerun cleanly (red, restored) and the mutations above, `git status --short` empty. Red output for each new test before its fix. Do not run the GUI on the display; the reviewer repeats the real run. Commit after every step. `worker_done` only with all of it pasted; otherwise `ask` and keep working.
