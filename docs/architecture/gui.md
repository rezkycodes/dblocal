# GUI architecture (W1-G shell)

Owner: W1-G. This document describes the `crates/gui` shell built against `dblocal_core::service::DblService` and the in-memory mock. Real service wiring and tray integration land in later waves.

## Boundaries

- `crates/gui` is a **library** (`dblocal_gui::run()`); `crates/cli` is the only binary, and no-args launches the GUI.
- The UI talks only to `Arc<dyn DblService>`. No direct state/engine/process access from GUI code.
- `tokio` work (mock timers, simulated downloads, future service I/O) runs on its own runtime. It never runs on the GPUI executor. UI updates cross into GPUI through a `flume` channel that a GPUI task `await`s, so the loop wakes only on real activity — there is **no idle timer**.
- Every user-facing string lives in `crates/core/src/strings.rs` (including the Vulkan pre-flight text and the port-taken validation sentence). `crates/gui` contains no literal UI sentence.

## Modules

| Module | Responsibility | Tests | Line coverage |
|---|---|---|---|
| `lib.rs` | Public `run()`, the GUI-availability seam, `verify_theme_contrast` | unit | 57% (entry/seam) |
| `app.rs` | `Workspace` entity, construction, event pump | layout + render | 586 lines |
| `app/actions.rs` | Actions and key bindings | layout | 46% (layout) |
| `app/state.rs` | Pure event reduction over `ShellState` | unit | 99.2% |
| `app/create_submit.rs` | GPUI-free create submission (validate, spec, submit) | unit, thorough | 90.8% |
| `app/workspace_api.rs` | One-line accessors the views call | layout | — |
| `app/views.rs` | Element tree (sidebar, tabs, overview, logs) | layout + render | 72% (layout) |
| `app/preview.rs` | Preview constructors for headless renders | render | 95% |
| `app/bootstrap.rs` | Process bootstrap and window creation | layout | 20% (entry) |
| `app/title_bar.rs` | Client-side title bar | render | 94% |
| `app/status_bar.rs` | Running count, catalog freshness, downloads popover, version | layout | 44% (layout) |
| `app/palette.rs` | Command palette on `gpui_component::command::Command` | layout | 70% (layout) |
| `app/create_dialog.rs` | Create-instance form on `InputState` | layout + render | 82% |
| `app/delete_dialog.rs` | Delete modal with the two D-18 actions | layout + render | 81% |
| `format.rs` | Pure view-models (grouping, labels, bytes/speed/ETA, masking, palette, validation) | unit, thorough | 98.5% |
| `logs.rs` | Per-instance `LogStore` of `LogBuffer`s, follow, copy | unit, thorough | 99.5% |
| `service_bridge.rs` | Event bridge, no polling, Resync on lag | unit | 92.1% |
| `theme.rs` | JSON theme loading, font stack, system sync | unit | 96.2% |
| `contrast.rs` | Parses the rendered theme JSON and asserts WCAG 4.5:1 | unit | 89.6% |
| `vulkan.rs` | Vulkan pre-flight (exit 78) | unit | 89.5% |
| `assets/themes/*.json` | Theme source of truth (loaded with `include_str!`) | read by contrast tests | — |

Coverage totals are from `cargo llvm-cov -p dblocal-gui`. Layout-heavy files (`app.rs`, `status_bar.rs`, `palette.rs`, `actions.rs`) render element trees that only a real frame exercises; they are listed separately from the logic files that carry the required ≥80%.

## Event flow (no polling)

1. `service.subscribe()` returns a `broadcast::Receiver<ServiceEvent>` (tokio).
2. `service_bridge::spawn_bridge` runs a dedicated thread with a current-thread tokio runtime that forwards events into an unbounded `flume::Sender<UiEvent>`.
3. The GPUI app spawns an async task: `while let Ok(ev) = rx.recv_async().await { view.update(...); cx.notify(); }`. It parks on the channel, so an idle app consumes no CPU.
4. Log streaming is opt-in per opened instance via `watch_logs(id, true/false)`. Each open tab owns its own buffer in a `LogStore` keyed by `InstanceId`, so a line for instance B can never appear under instance A; closing the tab drops it. The Logs tab renders through `uniform_list` over that buffer, and lines are stored as `SharedString`, so a frame clones pointers, never text.
5. **Removal and forced stop (addendum G).** `InstanceRemoved` goes through `ShellState::apply_removed`, which drops the row, closes its tab, clears the selection if it pointed there and drops its log buffer; the workspace then calls `watch_logs(id, false)`. An unknown id is a no-op. The mock emits it when a delete completes (before `OperationFinished`) and when a create rolls back; both are tested. `StopForced` (a scripted `StopNeedsForce`) is emitted before the final `InstanceChanged` and raises the Indonesian toast `UI_TOAST_STOP_FORCED`; a render test proves a deleted instance leaves the dock (status-dot pixels 540 → 360, 113 sidebar rows differ).
6. **Resync on lag.** `broadcast` drops the oldest events of a slow receiver. When the bridge sees `RecvError::Lagged` it emits `UiEvent::Resync`; the workspace then re-reads `list_instances()` and the catalog, so a row can never stay "berjalan" after missed `InstanceChanged` events.

## Themes and contrast

- Dark: `DBLocal One Dark`, light: `DBLocal One Light`, based on Atom One palettes. Source files: `crates/gui/assets/themes/one-dark.json`, `one-light.json`, embedded with `include_str!` and registered through `ThemeRegistry::load_themes_from_str`.
- The contrast test parses the **same JSON files** and checks every `X.foreground` on its paired `X.background` and on `background`/`sidebar.background`, plus `foreground`, `muted.foreground`, the status/semantic colours, the tab/list/table rows, and every `base.*` colour in both themes, failing below 4.5:1. 122 pairs are checked (61 per theme).
- The test is load-bearing: setting the light theme's `primary.background` back to `#4078f2` reproduces the review's `3.88:1` failure.
- Light-theme tokens fixed in the tokens, not the test: `primary`/`info`/`sidebar.primary` background `#4078f2 → #2a56c0` (fg 3.88 → 6.31), `base.blue(.light)` and `chart.1` moved with it, and `muted.foreground #696c77 → #5f6169` (on `muted.background` 4.35 → 5.14). The dark theme already passed.

## Fonts

- Default: a system font, with `.SystemUIFont` kept as the final fallback (W0-C's recommendation). On Linux GPUI maps `.SystemUIFont` through a stack that includes `Segoe UI`; this review host happens to have Segoe UI installed under `~/.local/share/fonts`, which is why W0-C observed that Windows family on a Fedora machine.
- Fix: `theme::init` names the first installed family from an explicit stack and logs what it picked. Measured on this host: `UI font family: "Adwaita Sans"`, `Monospace font family: "Adwaita Mono"` (previously `"Segoe UI"` / `"Noto Sans Mono"`). Windows and macOS keep their native system font.
- No bundled font in this wave; if one is added it must ship its SIL/OFL licence.

## Performance (OPEN-08)

Measured with `crates/gui/examples/measure` (release, strip + `lto=thin`) on Fedora 44 / Wayland / AMD RADV.

W1-G's original run on this host:

| Phase | VmRSS | CPU |
|---|---:|---:|
| Empty GPUI window (floor) | 165.4 MB | 0.20 % |
| Shell idle (mock) | 178.6 MB | 0.10 % |
| Shell after a mock download | 178.7 MB | 0.00 % |

Those are kept as the historical W1-G data point. Absolute RSS for the *same* binary drifts by up
to ~8 MB between sessions on this host (page cache, GPU driver state, compositor load), so they are
not reproducible at the 1 MB level: on 2026-11-21 the same `main` binary reports FLOOR 167.6 MB /
IDLE 180.0 MB. For any before/after comparison, use a same-session A/B.

**W2-D A/B (direct `gpui-pre` deps vs the `gpui-kit` umbrella, same `measure.rs`, alternating
3 reps each, one session):** FLOOR 176.8 → 177.3 MB (**+0.5 MB**), shell IDLE 188.0 → 188.7 MB
(**+0.7 MB**), AFTER_DOWNLOAD 188.2 → 188.9 MB (+0.7 MB), idle CPU 0.08 → 0.12 %. Cold start is
indistinguishable (p50 494 vs 489 ms over 20 interleaved runs each). The umbrella edge therefore
costs ~0.5–0.7 MB of RSS, not the +12.8 MB an earlier draft claimed from comparing different
methods. Release binaries: `dblocal` +1.52 MB, `measure` +1.51 MB. Full tables and method:
`docs/spikes/gpui-kit/README.md` §2.

- **CPU target met:** idle CPU 0.08–0.12 %, an order of magnitude under the 1 % target (the 50 ms
  pump is gone; see the bridge above).
- **RSS target not met, and the floor is the cause:** an *empty* GPUI window is ~166–180 MB on this
  host depending on session, ≥15 MB over the 150 MB target before any DBLocal widget exists. App
  code cannot go below that without changing the renderer or toolkit; per OPEN-08 this is reported
  with evidence rather than worked around.

## Headless render evidence

W2-E replaced the W1-G `shell-*.png` set with prototype-screen renders (same method, new file names),
regenerated with `DBLOCAL_UPDATE_RENDERS=1`:
`overview-dark`, `overview-light`, `overview-failed-dark`, `overview-revealed-dark`,
`logs-light`, `logs-diagnosis-dark`, `databases-product-light`, `databases-demo-light`,
`create-download-dark`, `create-unavailable-dark`, `dock-all-dark`, `dock-filtered-dark`,
`palette-dark`, `delete-light`, `stale-light`. The old `shell-*.png` names no longer describe the
screens and were removed.

`crates/gui/tests/render.rs` renders the real view off-screen through GPUI's
headless wgpu path. Frames are written to `target/renders/`; committed
screenshots under `docs/design/renders/` are refreshed only with
`DBLOCAL_UPDATE_RENDERS=1`, so `cargo test` never dirties a tracked path.

Output is deterministic: two consecutive full runs produce byte-identical PNGs.
The earlier non-determinism came from `MockService::seed_instance` using a
random UUID v4, which changed `BTreeMap` order (and tab order) every run; it now
derives a UUID from the seedable RNG.

Assertions sample specific regions against the theme tokens, so a layout
regression fails the test rather than passing on a "> 40 colours" check:

| Check | Evidence (this host) |
|---|---|
| Sidebar surface on the left edge, both themes | sampled `#21252b` / `#f0f0f0` over > half the height |
| Centre background differs per theme | luminance 44.21 (dark) vs 241.30 (light) |
| Status dots: running + failed in the sidebar | green 360 px, red 180 px |
| Selected-row accent, both themes | 47903 / 47993 px |
| Logs tab really selected: text in the lower body | 3978 px vs Overview 0; 65 text rows |
| Two tabs each show their own lines | a=3978, b=5994, Overview 0 |
| Catalog banner strip | 98785 warning px |
| Failure panel danger surface | 606864 px |
| Palette panel surface | present over the dimmed backdrop |

## W2-B: the real service, tray, close and quit

The Wave 2 seam is closed. `dblocal_gui::run()` builds a real
`LocalService`; the in-memory mock is behind the `demo` cargo feature.

### Entry point

1. `gui_unavailable_reason()` (headless flag, main-thread check, display check) and
   `vulkan::preflight()` run first, exactly as before.
2. `LocalService::new(DblPaths::from_env()?, ServiceOptions::default())`. Construction already
   opens SQLite, reads the catalog and probes glibc **on the service's own thread**
   (`service.md`, "Threading model"), so the GPUI thread does no I/O.
3. If construction fails (unwritable home, corrupt store), `show_startup_failure` opens a small
   window with the Indonesian `DblError::user_message()` and the process exits `1`. With no display
   it prints `UI_STARTUP_FAILED_TITLE` to stderr and exits `1`. There is no panic and no black window.
4. `app::run_with_service(Arc<dyn DblService>)` opens the window.

`demo` feature: `dblocal-core/mock` is enabled and `DBLOCAL_DEMO=1` selects
`app::run_with_mock()` (the prototype cast) instead. Without the feature the variable is ignored
with a warning. The production dependency edge has no `mock`: `cargo build -p dblocal-gui` does
not link `service::mock` (checked with `strings` on the built rlib: 0 `MockService` symbols).

### The background-call rule (deliverable 2)

`DblService` is synchronous; `LocalService` takes the store mutex and hits SQLite on most reads, and
`connection_uri` may block on an OS keychain prompt. **No trait call runs on the GPUI thread.**

- `Workspace::bg(cx, work)` (`app/background.rs`) runs `work` with `cx.background_spawn`, then applies
  the returned closure inside `this.update(...)`. The closure receives `(&mut Workspace,
  &mut Context)`, so the result lands on the UI thread with no shared mutable state.
- A `pending` counter is bumped before the spawn and decremented in the apply step. The status bar
  shows `UI_PENDING` ("Memuat...") while it is non-zero — the neutral pending state.
- Every call site goes through it: initial load, resync, catalog reload, tab open (logs, URI,
  `.env`, data size, port suggestion), reveal, copy, start, stop, delete, autostart, create submit,
  `watch_logs(false)` on removal/tab close, and each quit-flow stop.
- `cargo test` proves it: `app::bg_service_tests::slow_service_calls_do_not_block_gpui` uses a fake
  service whose every call sleeps 2 s. It fires a background refresh, immediately toggles the
  palette, and asserts the toggle took < 500 ms and the result still landed (palette state intact,
  service called at least once).

Preview/render constructors (`Workspace::new_preview`) read synchronously on purpose: they are
not on a user path, they seed a deterministic frame, and they never start background work that
could change the render between frames (`bg_startup = false`).

### Every `DblService` call site and its thread (W2-B-r2, deliverable 4)

Audited with `git grep -n 'self\.service\.' crates/gui/src/`. `DblService` is synchronous; the only
permitted UI path is `Workspace::bg`, which runs the closure on `cx.background_spawn` (a GPUI
background executor thread, never the render thread) and applies the result inside
`this.update(...)`.

| Site | Call | Runs on | Proof |
|---|---|---|---|
| `app/background.rs:56-57` | `list_instances`, `list_runtimes` | background | inside `self.bg(cx, move \|\| { … })` at `:54` |
| `app/background.rs:77-80` | `list_instances`, `catalog`, `list_runtimes`, `refresh_catalog` | background | inside `self.bg` at `:75` |
| `app/background.rs:143` | `catalog` | background | inside `self.bg` at `:141` |
| `app/instance_actions.rs:26,33` | `watch_logs`, `connection_uri` | background | inside `self.bg` at `:25` / `:32` |
| `app/instance_actions.rs:55` | `connection_uri` (`.env`) | background | inside `self.bg` at `:54` |
| `app/instance_actions.rs:76` | `suggest_port` | background | inside `self.bg` at `:75` |
| `app/instance_actions.rs:91` | `tail_logs` | background | inside `self.bg` at `:90` |
| `app/instance_actions.rs:119` | `data_size` | background | inside `self.bg` at `:118` |
| `app/instance_actions.rs:145` | `start_instance` | background | inside `self.bg` at `:144` |
| `app/instance_actions.rs:161` | `stop_instance` | background | inside `self.bg` at `:160` |
| `app/instance_actions.rs:208` | `connection_uri` (copy) | background | inside `self.bg` at `:207` |
| `app/instance_actions.rs:242` | `connection_uri` (reveal) | background | inside `self.bg` at `:241` |
| `app/instance_actions.rs:293` | `set_autostart` | background | inside `self.bg` at `:292` |
| `app/instance_actions.rs:331,371` | `suggest_port` | background | inside `self.bg` at `:330` / `:370` |
| `app/instance_actions.rs:455` | `create_submit::submit` → `validate_spec`, `create_instance` | background | inside `self.bg` at `:454` |
| `app/instance_actions.rs:493` | `data_size` | background | inside `self.bg` at `:492` |
| `app/instance_actions.rs:531` | `delete_instance` | background | inside `self.bg` at `:530` |
| `app/quit_flow.rs:54` | `stop_instance` | background | inside `self.bg` at `:53` |
| `app/workspace_api.rs:267` | `change_port` + `start_instance` | background | inside `self.bg` at `:264` |
| `app.rs:340,361,375` | `watch_logs(false)` | background | inside `self.bg` at `:339`/`:360`/`:374` |
| `service_bridge.rs:74` | `subscribe()` | bridge thread | `spawn_bridge` runs `forward(events, tx)` on `runtime.block_on` in the `dblocal-service-bridge` thread |

**The one call that ran on the GPUI thread, and the fix.** `app/instance_actions.rs:267` used to be
`toggle_reveal_sync`, which called `self.service.connection_uri(...)` directly from a `&mut self`
method — a GPUI-thread call that can block on the OS keychain. It is only used by the preview
constructors (`preview_revealed`); the production reveal path is `toggle_reveal`, which already goes
through `bg`. The synchronous helper therefore stays but is confined to previews, alongside
`load_logs_sync`, `open_tab_sync` and `new_preview`'s initial reads. These are documented as
preview-only because they seed a deterministic frame and never run on a user path
(`Workspace::new_preview` sets `bg_startup = false`).

Two dead synchronous wrappers were removed in this round rather than left unreachable:
`Workspace::refresh_instances` (was `#[allow(dead_code)]`, called `list_instances` and
`list_runtimes` on the caller's thread) and `service_suggest_port` / `service_connection_uri` /
`service_change_port`. Nothing calls them; leaving them invites a future GPUI-thread call.

The proof that the background rule holds is the 2 s slow-service test:
`app::bg_service_tests::slow_service_calls_do_not_block_gpui` fires `refresh_instances_bg`, toggles
the palette immediately and asserts the toggle took under 500 ms while every fake call sleeps 2 s.

### Startup (deliverable 3, F-CAT-01/02)

`Workspace::new` starts with an **offline** catalog read model and an empty instance list, seeds the
event pump and the tray pump, and calls `startup_load` in the background: `list_instances`,
`catalog`, `list_runtimes`, then `refresh_catalog(false)` **without awaiting it**. So the first frame
never blocks on the network; an offline start shows the banner + bundled catalog and never an empty
create form. The refresh reports back through `CatalogUpdated`.

Events drive everything (`app.rs::apply_event` / `apply_event_with_cx`):

| Event | Effect |
|---|---|
| `InstanceChanged(view)` | upsert the row, refresh the tray menu |
| `InstanceRemoved(id)` | drop row, tab, selection, log buffer, URI, dotenv/port/size caches; `watch_logs(false)` in the background |
| `StopForced(id)` | toast `UI_TOAST_STOP_FORCED` |
| `OperationFinished(Err)` | toast `<DBL code> — <message>` |
| `Lagged` → `Resync` | background re-read of instances + runtimes |
| `CatalogUpdated` | background catalog re-read |

### Tray (F-TRY-01)

`crates/gui/src/tray.rs`:

- `TrayMenuModel::from_views(&[InstanceView])` is pure: instance rows grouped by engine in dock
  order, each with a shape+word status label (`●`/`○`/`▲`/`◐` + status) and a start or stop action,
  disabled while `Starting`/`Stopping`; plus "Buka DBLocal" (`UI_TRAY_OPEN`) and "Keluar"
  (`UI_TRAY_QUIT`). Unit-tested for grouping, labels, disabled states and id→action lookup.
- `TrayBackend` is the platform glue trait. Linux implements it with **ksni 0.3** (StatusNotifierItem
  over D-Bus, `blocking` + `async-io`, no GTK). `start_tray()` also runs a `StatusNotifierWatcher`
  probe via `gdbus` and reports "no tray" instead of failing the app when the item cannot register.
- macOS and Windows are **not implemented**: `start_tray()` returns `TrayStartup::none()` there, the
  status bar says the tray is unavailable, and close-to-quit applies. `tray-icon` 0.26 remains
  reasoned-only (`docs/spikes/gpui/README.md` §6).
- No idle timer (OPEN-08): the tray sends actions over a `flume` channel and the workspace parks on
  `recv_async`, so the loop wakes only on real activity. The tray menu is pushed on
  `InstanceChanged`/`InstanceRemoved` — no polling. There is no `sleep`, `poll`, or timer in the
  bridge (`service_bridge::spawn_bridge` does `rx.blocking_recv()` / `recv_async()` only) and the tray
  pump (`start_tray_pump`) is a `recv_async` loop.
- Tray actions call the **same** `start_instance`/`stop_instance`/`request_quit` methods the window
  uses, so the window reflects a tray action through the normal event path (F-TRY-01 AC2).
- Previews never start the tray: `tray::enable_for_app()` is set by `run_with_service`, so a headless
  render cannot register a StatusNotifierItem on the test host.

#### Tray latency (W2-B-r2, deliverable 5)

Measured by `crates/gui/src/app/tray_latency.rs` (`tray_action_reaches_the_model_through_bridge`),
which drives the real path named by the spec: `service.start_instance` emits `InstanceChanged` on the
broadcast channel, `service_bridge::spawn_bridge` forwards it on its dedicated thread to a flume
receiver, and the test updates `TrayMenuModel::from_views` with the received view. No GPUI test
scheduler is used because it rejects cross-thread wakers; the measured path is bridge thread + model
update.

Declared parallel load: 4 busy-spin threads started inside the test (`CpuLoad::spawn(4)`, no
`stress-ng`, no external process). `n=50`, one warm-up round discarded.

| Metric | Value | Target |
|---|---:|---:|
| p50 | 0.1 ms | <= 1000 ms |
| max | 0.1 ms | <= 1000 ms |
| n | 50 | 50 |

Command/output: `TRAY_LATENCY n=50 p50=0.1ms max=0.1ms target=1000ms load=4_busy_spin_threads`. The
test asserts `max <= 1000.0`.

### Close and quit (F-TRY-02)

`crates/gui/src/quit.rs` is the pure state machine:

```text
Idle --QuitRequested{running: []}--> Done                      (Exit)
Idle --QuitRequested{running}-----> Confirming                 (ShowDialog)
Confirming --KeepRunning----------> Done                       (Exit)
Confirming --StopAll--------------> StoppingAll                 (StopInstances)
Confirming --Cancel---------------> Idle
StoppingAll --StopFinished{ok}----> StoppingAll | Done         (None | Exit)
StoppingAll --StopFinished{!ok}---> StoppingAll | StopFailed   (None | ShowDialog)
StoppingAll --ForceExit-----------> Done                       (Exit)
StopFailed --ForceExit------------> Done                       (Exit)
StopFailed --Cancel---------------> Idle
```

Every transition is unit-tested, including "a stop fails" and "the user cancels".

Window behaviour: `window.on_window_should_close` calls
`Workspace::handle_window_close_request`. With a tray it calls `cx.hide()` and returns `false`, so the
window is hidden and the app keeps running with engines untouched (AC1). Without a tray the close is
a quit. The quit dialog's default (primary) button is "Biarkan tetap berjalan" (AC2, AC3); the
alternative issues `stop_instance` for each running instance and shows `UI_QUIT_STOPPING`; a failed
stop shows `UI_QUIT_STOP_FAILED` with cancel or exit-anyway, and while stops are pending the user
may confirm `UI_QUIT_FORCE_CONFIRM` — the app never exits with a stop pending without that
confirmation.

### Modules added in W2-B

| Module | Responsibility |
|---|---|
| `app/background.rs` | `bg`, `is_pending`, startup load, pumps, tray refresh, catalog reload |
| `app/instance_actions.rs` | tab open + derived reads, start/stop, reveal/copy, autostart, create and delete dialogs |
| `app/quit_flow.rs` | tray actions, quit transitions → service calls/window ops, the quit dialog |
| `app/tests_secret.rs` | `LocalService`-shaped URI/`.env` never reaches a `Label` before Show |
| `quit.rs` | the pure quit machine |
| `tray.rs` | the pure menu model + Linux ksni glue |

## Known issue (W2-B-r2, a11y)

A debug build panic was seen once on a real desktop with accessibility enabled: `Duplicate a11y node id: ... In a release build, this node would be silently discarded from the a11y tree.` It is a `debug_assert` in gpui (release builds drop the duplicate silently); it is not diagnosed in this round. Task W2-B2 will own it.

### Unverified per OS

- **Linux/Wayland (this host):** ksni registers (a `StatusNotifierWatcher`/AppIndicator extension is
  present here); the tray-to-window latency through the real service path is reported with p50/max
  in the W2-B report.
- **GNOME without the AppIndicator extension:** untested removal path; the code reports "tray tidak
  tersedia", keeps the app usable and applies close-to-quit.
- **X11:** builds and runs (W2-D), tray not exercised there.
- **macOS / Windows:** neither the tray nor the window/quit behaviour can be built or run here.
  `tray-icon` integration is reasoned only; `start_tray` returns no tray on those targets.

