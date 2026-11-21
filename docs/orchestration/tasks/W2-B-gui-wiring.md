# W2-B — Wire the GUI to the real service: LocalService, tray, close and quit behaviour

Branch: `w2-gui-wiring`. **Start only after W1-G and W2-A are merged** (the dispatch message tells you the base commit).
Read first, in this order: `AGENTS.md`, `docs/architecture/CONTRACT.md` (note the events `InstanceRemoved` and `StopForced`), `docs/architecture/service.md` (W2-A: the real service, its threading and its event sequences), `docs/architecture/gui.md` (W1-G, including its "Wave 2 seam" section), `docs/design/UI-DIRECTION.md`, and `docs/spikes/gpui/README.md` sections on the tray (OPEN-02: `ksni` on Linux, `tray-icon` on macOS and Windows, event loop caveats).
PRD (`docs/prd/dblocal/PRD.md`): `F-TRY-01`, `F-TRY-02`, `F-INS-01..05`, `F-LOG-01..02`, `F-CAT-01..02`, decisions D-02, D-04, D-23.
Never open an image with the Read tool; check renders numerically with the region-sampling helpers W1-G wrote.

## Goal

`dblocal_gui::run()` starts the real product: it builds a `LocalService` over `DblPaths::from_env()` and shows the workspace, with a tray icon and the close and quit behaviour the PRD specifies. The in-memory mock stays only as a demo behind a cargo feature.

## You own

`crates/gui/**`, `docs/architecture/gui.md`, `crates/core/src/strings.rs` (you are its only editor in this task), and the one-line call site in `crates/cli/src/main.rs` if the entry point's signature changes. You may not edit `crates/core/src/instance/**` or any other core module. If the service is missing something you need, use `ask` with the exact call; do not work around it in the GUI.

## Deliverables

1. **Real service by default.** `run()` builds `LocalService::new(paths, ServiceOptions::default())`. If that fails (unwritable home, corrupt state), show a clear Indonesian message in a small window and exit with a non-zero code; never panic, never a black window. The mock moves behind a `demo` feature (`run_demo()`, also selectable with `DBLOCAL_DEMO=1` only in builds with that feature). Remove `features = ["mock"]` from the production dependency edge of `dblocal-gui`; the mock is then a dev-dependency or the `demo` feature only. `cargo build -p dblocal-gui` without the feature must not link `service::mock`.
2. **No service call blocks the GPUI thread.** W1-G documented that `refresh_instances`, `validate_spec`, `create_instance`, `connection_uri` and the rest are called on the UI thread. With `LocalService` those take the store mutex and touch SQLite; another process (a CLI) holding a write lock makes them wait up to `STORE_BUSY_TIMEOUT_MS` (5 s), and `connection_uri` may wait on an OS keychain prompt. Route every `DblService` call through `cx.background_executor()` and apply the result with `this.update`, showing a neutral pending state in the meantime. Test with a fake service whose calls sleep 2 s: assert that the window keeps rendering and handling input during the call, and that the result still lands. This is the single most important item; do not weaken it.
3. **Events drive everything.** `InstanceRemoved` drops the row, tab, log buffer and watcher (W1-G wired it), `StopForced` toasts, `OperationFinished(Err)` toasts with the DBL code, `Lagged` triggers `Resync`. On startup the initial list is read in the background and the catalog refresh is started without waiting for it; an offline start shows the offline banner and the bundled catalog, never an empty form (F-CAT-01/02).
4. **Tray (F-TRY-01).** A `tray` module with a pure model: `TrayMenuModel::from_views(&[InstanceView])` giving entries (name, status dot, start or stop action), "Buka DBLocal", "Keluar", fully unit-tested (grouping, labels from `strings.rs`, disabled states while `Starting` or `Stopping`). Platform glue behind a small trait: Linux `ksni` (verified here), macOS and Windows `tray-icon` (compile-checked only if the target toolchain exists; otherwise say so, do not claim it works). Menu actions go through the same `DblService` calls and the same event bridge as the window, so the window reflects a tray action in <= 1 s (F-TRY-01 AC2) without polling; measure it and report p50/max like W0-C did, now through the real path. No timer while idle (OPEN-08): the tray wakes the UI over a channel.
   If no tray host exists (GNOME without the AppIndicator extension, as on this machine), say it once in the status bar (F-TRY-01 AC3), keep the whole app working, and use the no-tray close rule below.
5. **Close and quit (F-TRY-02).** Closing the window hides it and keeps the app alive in the tray, engines untouched (AC1). If there is no tray, closing the window is a quit. A quit with running instances opens the dialog with "Biarkan tetap berjalan" as the default and "Hentikan semua lalu keluar" as the alternative (AC2); the first leaves every engine alive (AC3, proven by W2-C later), the second stops them through `stop_instance`, shows progress, reports a failed stop honestly and does not exit while a stop is still pending unless the user confirms. Model the quit flow as a pure state machine (Idle, Confirming, StoppingAll(pending), Done) with tests for every transition, including "a stop fails" and "the user cancels".
6. **Strings.** Every new sentence in Indonesian in `strings.rs`, none literal in `crates/gui`. Keep the PRD wording where it quotes UI text.
7. **Docs.** Update `docs/architecture/gui.md`: the real entry point, the background-call rule, the tray model and platform glue, the quit state machine, what is unverified per OS.

## Tests and bar

Pure view-models and state machines at >= 90% lines, other non-layout files >= 80%, layout files reported separately. Headless render tests for the quit dialog (both buttons, default marked), the tray-unavailable note, and a pending state. A 40-run loop for the GUI lib tests and for any test that uses threads or time, pass counts pasted. `cargo xtask ci` and `cargo deny check` green. No `unwrap`/`expect` outside tests, files < 800 lines (quote the real `wc -l` maximum), functions < 50 lines except pure layout. Render tests write to `target/`, never into tracked paths.

## Done when

The checklist above, the slow-service test result, the tray latency numbers, the real `git log --oneline -3` hash after committing, and a plain list of what you could not verify (Windows and macOS runtime and tray, X11, the AppIndicator path on GNOME). Commit before you report.
