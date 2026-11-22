# W2-B2 — the debug build panics with "Duplicate a11y node id" when an overlay opens

New branch from main, worktree under `~/orca/workspaces/db-local/`. Rules: `docs/orchestration/tasks/_COMMON-RULES.md` incl. 11-14. You own `crates/gui/**` and, only if the cause is there, `crates/core/src/strings.rs`. Not allowed: `unwrap`/`expect`/`panic` outside tests, silencing the assertion (no `catch_unwind`, no patching the vendored gpui).

## Evidence (reviewer, 2026-11-23, Linux/Xwayland, debug build of `dblocal-cli` at W4-D merged with main)

Launch `env -u WAYLAND_DISPLAY DISPLAY=:0 DBLOCAL_HOME=<isolated> dblocal --autostart` with one instance flagged autostart and running (a tray host is present), click the tray item "Keluar" while the instance is running. The quit dialog ("Biarkan tetap berjalan" / "Hentikan semua lalu keluar", F-TRY-02 AC2) is meant to open, and instead the process aborts:

```
thread 'main' panicked at .../gpui-pre-0.3.12/src/window/a11y.rs:415:13:
Duplicate a11y node id: #4375755302221953578. In a release build, this node would be silently discarded from the a11y tree.
```

So in a release build the node is silently dropped from the accessibility tree: the dialog or part of it is invisible to a screen reader (F-PLH-01 accessibility). The same class was seen earlier on another overlay. Release builds do not panic, which is why `cargo xtask ci` (debug tests) and the earlier real-window runs of the release binary did not show it everywhere.

## Do

1. Reproduce on the real debug binary and write down which element ids collide: the id is a hash, so find the colliding pair by logging the ids at the failing call (a throwaway local patch is fine, do not commit it) or by reading how `crates/gui` builds the quit dialog and what `gpui-component`'s dialog adds. Say whether the duplicate comes from our code (the same `ElementId` reused for two elements in one frame, an id derived from a constant used twice, a list built without per-row ids) or from `gpui-component` itself.
2. Red first: a headless GPUI test that opens each overlay we own (quit dialog, new-instance dialog, command palette, delete confirmation, the status-bar popover, any other `Dialog`/`Popover`/`Sheet` the app uses: grep for them) under `cfg(debug_assertions)` and renders a frame with the accessibility tree built, so the assertion would fire. If the harness cannot build the a11y tree, find out why and make it (the existing 232 tests never saw this panic, so they do not build it). Paste the red output on the current head.
3. Fix at the cause. If the collision is inside `gpui-component`, work around it from our side with distinct ids and say so plainly (do not fork the dependency here).
4. Audit: every list/row/dialog id in `crates/gui/src/**` is derived from something unique per element (instance id, row key), never from a bare literal used twice; list what you changed.
5. Real window: after the fix, the tray Quit with a running instance shows the dialog on the debug binary; choose "Biarkan tetap berjalan" and confirm the app exits and the engine survives, then repeat with "Hentikan semua lalu keluar" and confirm the engine stops. Paste the transcripts and clean up (`pgrep -x valkey-server` = 0, remove the isolated home).

## Bar

fmt, clippy workspace (also `--features demo`), deny, GUI lib tests x20 from a private copy of the test binary (print distinct passed counts), `cargo xtask ci` under the heavy lock (`CARGO_BUILD_JOBS=8`, MemAvailable above 8000 MB). Not verifiable here: Windows/macOS accessibility trees. `worker_done` only with the red and green output and the real-window transcript.
