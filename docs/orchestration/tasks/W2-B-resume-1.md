# W2-B — RESUME 1 (the machine was rebooted; your first worker was lost with 21 uncommitted files)

You are a NEW worker continuing branch `rezkycodes/w2-gui-wiring`. Its worktree is `/home/rezkycodes/orca/workspaces/db-local/w2-gui-wiring`. **HEAD is `306907c` and the tree is DIRTY: 13 modified files and 8 new files (`crates/gui/src/{quit.rs,tray.rs}`, `crates/gui/src/app/{background,bg_service_tests,instance_actions,quit_flow,tests_secret,tray_latency}.rs`). That is the previous worker's work, about 2.5 hours of it, and it has NEVER been committed. Do not discard, reset, stash or clean anything. Do not start from scratch.**

## 0. First, in this order

1. `git status --short` and `git diff --stat`; read every new file and the diff of every modified file. Write down what exists and what is missing against the spec. The previous worker's last words were: "The tray-latency test is flaky under parallel load ... I'll widen the polling window".
2. Read `AGENTS.md`, then the original spec `/home/rezkycodes/Development/AI-Agent/db-local/docs/orchestration/tasks/W2-B-gui-wiring.md` (ownership and bar still apply), `docs/architecture/gui.md`, `docs/architecture/service.md`, `docs/design/renders/w2-e-report.md`.
3. Get to a compiling tree, then **commit within your first 30 minutes** (`wip(gui): ...` is fine, you will squash or add to it later) and commit after every meaningful step. A hung or lost session must never again cost the work.

## Hard rules (they bit earlier workers)

- NEVER open an image with the Read tool (you cannot see images; it fails with HTTP 422 and ends your turn). Verify visual claims with pixels: `python3 -I /home/rezkycodes/Development/AI-Agent/db-local/scripts/review/ink_bands.py IMAGE.png X0 Y0 X1 Y1` prints the horizontal ink bands of a rectangle (image pixels; renders are 2560x1640). A test is not evidence of a visual fix until it fails without the fix. **`/tmp/claude-1000/` no longer exists (reboot); do not refer to it.**
- ONE build cache: `CARGO_TARGET_DIR=$HOME/.cache/dblocal-target-gui`. One cargo build at a time. Never leave a GUI window or a background process running when you are not looking at it (the owner's RAM is tight and a low-memory reaper killed three processes). Report `df -h /` and `free -m` before and after your final gate.
- Another worker (W2-F) works on `crates/core/src/{download,engine,instance}` at the same time. You do not touch those. You own `crates/gui/**`, `docs/architecture/gui.md`, `crates/core/src/strings.rs`, and the call site in `crates/cli/src/main.rs`.
- Do not open real network connections or start real engines in GUI tests.

## 1. The original W2-B deliverables still apply

Real `LocalService` in `run()` with an Indonesian error window on failure (never panic); the mock only behind the `demo` feature; every `DblService` call off the UI thread (the 2 s slow fake service test: window keeps rendering and handling input); events drive everything; tray model pure and >= 90% covered; close hides to tray (quit when there is no tray); the quit dialog and its pure state machine; Indonesian strings in `strings.rs`; docs. See the original spec for the details.

## 2. The tray latency test (do not hide the number)

The spec bound is **tray menu reflects a change in <= 1 s**. A flaky test is not fixed by a wider window that can pass at any latency. Do this instead: measure the time from the event being emitted by the service to the tray model being updated through the REAL path (the bridge thread), run it 50 times, assert `max <= 1000 ms` and report p50 and max in `docs/architecture/gui.md` with the machine load. If it is flaky under parallel load, find why (a sleep in the bridge? a poll interval? a lock?) instead of loosening the bound. There must still be NO timer running while idle (OPEN-08): prove it with the idle-CPU measurement you already have in `docs/design/renders/measurements.md` extended to the real service.

## 3. TWO DEFECTS in `crates/gui` found by the reviewer on main (fix both; they are yours)

### 3a. Debug build panics at startup when desktop accessibility is on (real window)

Repro on this machine (Wayland; `gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus --method org.freedesktop.DBus.Properties.Get org.a11y.Status IsEnabled` prints `(<true>,)`): build the debug binary and run `dblocal` with no arguments. Result:

```
thread 'main' panicked at .../gpui-pre-0.3.12/src/window/a11y.rs:415:13:
Duplicate a11y node id: #18074014927432105136. In a release build, this node would be silently discarded from the a11y tree.
```

gpui builds the accessibility tree only when an assistive technology is connected, so the headless render tests never reach this code. Two elements that have an accessibility role share the same `GlobalElementId` path (the node id is a hash of it). A static grep found no repeated literal id in `crates/gui/src`; the ids that are built dynamically are `itab-{index}` (`screens.rs`), `engine-{}` and `version-{tag}` (`create_dialog.rs`), `group-{engine}` and `row-{id}` (`dock.rs`), `("log-line", index)` (`screens/logs.rs`), plus anything `gpui-kit` components derive internally. The backtrace has no DBLocal frame at the panic site, so you have to find the pair yourself. Suggested approach: look in `gpui-pre-0.3.12/src/window/a11y/debug.rs` (the `A11yDebug` capture) and `gpui/src/element.rs` around line 427 for how to activate or inspect the tree in a test; if the headless harness can activate the a11y tree for a frame (read `HeadlessAppContext` and `Window::a11y`), add a render test that draws every screen and dialog with it active and asserts no duplicate; if it cannot, write a test that walks the elements you build and asserts id uniqueness per parent. Fix the cause, then run the debug binary on the real display with `DBLOCAL_DEMO=1` for 10 seconds and paste that it does not panic (kill it afterwards, do not leave it running). Mutation-check the test: reintroduce one duplicate and paste the red output.

### 3b. The render watchdog kills healthy tests under load

`crates/gui/tests/render.rs`: `RenderWatchdog::arm(name)` starts a 120 s timer BEFORE the case waits for the global `GPU` mutex. The 9 cases run one after the other behind that mutex, so a case that is queued behind eight others is killed by the 120 s timer purely because of queueing. Evidence: on the reviewer's trial merge `cargo xtask ci` aborted with SIGABRT in `dblocal-gui --test render` while the machine was at load 25 (4 cases reported "running for over 60 seconds", 5 finished); two later runs at lower load passed 9/9 in 78 s and 86 s of test time, which is already close to 120 s. Fix: arm the watchdog AFTER the mutex is acquired (a hang in rendering or in shutdown is still caught because the guard is held by the case that hangs, and the watchdog stays armed during `shutdown_headless`), keep the "case name" message, make the limit a named constant, and also print the message to the real stderr before `abort()` (the test harness swallowed it). Add a test of the guard logic that does not need a GPU if you can (for example make the watchdog take a `Duration` and a callback, and test that it fires for a stuck case and does not fire for a queued one). Then the whole render binary must pass 10 times in a row with default test threads (paste the count and the total test time of each run).

## 4. Tests and bar (unchanged from the original spec, plus)

- `cargo xtask ci` and `cargo deny check` green; GUI lib tests 40 times in a row; render binary 10 times in a row (see 3b); pure logic >= 90% and other touched files >= 80% (`cargo llvm-cov`, paste numbers); files < 800 lines, no `unwrap`/`expect` outside tests, functions < 50 lines.
- The secret test: feed `LocalService`-shaped output (use the real `engine::uri` builder for all four engines with a secret such as `p@ss:w/rd%`) through the view model and assert the secret never appears in any rendered `Label` text before the user presses Show, and is gone again after Hide and after the tab is closed.
- Report, with pasted output: `git log --oneline -5`, `git status --short` (must be clean), the largest `wc -l`, the 2 s slow-service test result, the tray latency numbers, the quit state machine test names, the 3a and 3b evidence, what is NOT verified (Windows/macOS tray, real screen reader behaviour, anything that needs a human at the screen).

Report by sending `worker_done` through the orchestration channel when finished, and send `ask` if you are blocked. Do not end your turn without one of the two.
