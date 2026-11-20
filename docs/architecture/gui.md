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
5. **Resync on lag.** `broadcast` drops the oldest events of a slow receiver. When the bridge sees `RecvError::Lagged` it emits `UiEvent::Resync`; the workspace then re-reads `list_instances()` and the catalog, so a row can never stay "berjalan" after missed `InstanceChanged` events.

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

Measured with `crates/gui/examples/measure` (release, strip + `lto=thin`) on Fedora 44 / Wayland / AMD RADV. Raw numbers in `docs/design/renders/measurements.md`.

| Phase | VmRSS | CPU |
|---|---:|---:|
| Empty GPUI window (floor) | 165.4 MB | 0.20 % |
| Shell idle (mock) | 178.6 MB | 0.10 % |
| Shell after a mock download | 178.7 MB | 0.00 % |

- **CPU target met and improved:** the 50 ms pump is gone; idle CPU dropped from the spike's ~3.2 % to 0.10 %, an order of magnitude under the 1 % target.
- **RSS target not met, and the floor is the cause:** an *empty* GPUI window is 165 MB on this host, ≥15 MB over the 150 MB target before any DBLocal widget exists. App code cannot go below that without changing the renderer or toolkit; per OPEN-08 this is reported with evidence rather than worked around.

## Headless render evidence

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

## Wave 2 seam (must change before the production build ships)

- **The production GUI links the mock today.** `crates/gui/Cargo.toml` has
  `dblocal-core = { features = ["mock"] }` and `run_with_mock()` builds a
  `MockService`. That is acceptable for W1 only. W2-B must:
  1. Replace `run_with_mock()` with `run_with_service(Arc<LocalService>)` in
     `dblocal_gui::run()`.
  2. Gate the mock behind a `demo` feature so a release build does not compile
     it, and keep `run_with_mock()` behind that feature for the render tests.
- **Service calls run on the GPUI thread.** `refresh_instances`,
  `create_instance`, `validate_spec`, `change_port` and the other trait calls
  are synchronous and short, so calling them from the render/handler path is
  fine for the mock (and the trait contract says they return quickly). W2-B
  must keep them non-blocking: the real service must not do I/O on the caller's
  thread, or these call sites must move to the bridge's task and report back
  through `UiEvent`.

## Vulkan

`vulkan::preflight()` runs before `gpui_platform::application()`. It detects missing or invalid ICD manifests/overrides and, as a final gate, wgpu Vulkan adapter enumeration failure; on failure it prints distro install commands, `vulkaninfo` verification and exits `78`, never a panic or black window. `DBLOCAL_GUI_HEADLESS=1` and the main-thread/display checks in `lib::gui_unavailable_reason` keep `dblocal` (no args) usable on a headless box and from test harnesses.

## Unverified here

Windows, macOS and the X11 backend cannot be built or run on this Linux/Wayland host: `-lxkbcommon-x11` needs the `libxkbcommon-x11-devel` package, which is not installed (see `docs/dev/linux-build-deps.md`). Tray wiring, quit dialogs and the real service are Wave 2.
