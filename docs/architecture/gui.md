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
| `app.rs` | `Workspace` entity, window wiring, event pump | layout + render | 40% (layout) |
| `app/actions.rs` | Actions and key bindings | layout | 46% (layout) |
| `app/state.rs` | Pure event reduction over `ShellState` | unit, thorough | 99.2% |
| `app/views.rs` | Element tree (sidebar, tabs, overview, logs) | layout + render | 72% (layout) |
| `app/preview.rs` | Preview constructors for headless renders | render | 95% |
| `app/bootstrap.rs` | Process bootstrap and window creation | layout | 20% (entry) |
| `app/title_bar.rs` | Client-side title bar | render | 94% |
| `app/status_bar.rs` | Running count, catalog freshness, downloads popover, version | layout | 44% (layout) |
| `app/palette.rs` | Command palette on `gpui_component::command::Command` | layout | 70% (layout) |
| `app/create_dialog.rs` | Create-instance form on `InputState` | layout + render | 82% |
| `app/delete_dialog.rs` | Delete modal with the two D-18 actions | layout + render | 81% |
| `format.rs` | Pure view-models (grouping, labels, bytes/speed/ETA, masking, palette, validation) | unit, thorough | 98.6% |
| `logs.rs` | Ring buffer, follow, copy | unit | 100% |
| `service_bridge.rs` | Event bridge, no polling | unit | 89.6% |
| `theme.rs` | JSON theme loading, font stack, system sync | unit | 96.2% |
| `contrast.rs` | Parses the rendered theme JSON and asserts WCAG 4.5:1 | unit | 89.6% |
| `vulkan.rs` | Vulkan pre-flight (exit 78) | unit | 89.5% |
| `assets/themes/*.json` | Theme source of truth (loaded with `include_str!`) | read by contrast tests | — |

Coverage totals are from `cargo llvm-cov -p dblocal-gui`. Layout-heavy files (`app.rs`, `status_bar.rs`, `palette.rs`, `actions.rs`) render element trees that only a real frame exercises; they are listed separately from the logic files that carry the required ≥80%.

## Event flow (no polling)

1. `service.subscribe()` returns a `broadcast::Receiver<ServiceEvent>` (tokio).
2. `service_bridge::spawn_bridge` runs a dedicated thread with a current-thread tokio runtime that forwards events into an unbounded `flume::Sender<UiEvent>`.
3. The GPUI app spawns an async task: `while let Ok(ev) = rx.recv_async().await { view.update(...); cx.notify(); }`. It parks on the channel, so an idle app consumes no CPU.
4. Log streaming is opt-in per opened instance via `watch_logs(id, true/false)`; the Logs tab renders through `uniform_list` over the `LogBuffer` ring.

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

`crates/gui/tests/render.rs` renders the real view off-screen through GPUI's headless wgpu path and asserts on sampled pixels and distinct-colour counts. PNGs land in `docs/design/renders/`:

| State | Distinct colours | Notable sample |
|---|---:|---|
| Dark | 1274 | title/sidebar `#21252b`, centre `#282c34`, status `#21252b` |
| Light | 1336 | title/sidebar `#f0f0f0`, centre `#fafafa`, status `#f0f0f0` |
| Palette | 1059 | — |
| Logs | 978 | monospace lines over the editor surface |
| Create modal | 1703 | — |
| Delete modal | 1440 | — |
| Failure state | 1464 | danger panel `#b03730` present (651k pixels) |

Centre-region mean luminance: dark **43.91**, light **241.70** (the test requires a gap > 10).

## Vulkan

`vulkan::preflight()` runs before `gpui_platform::application()`. It detects missing or invalid ICD manifests/overrides and, as a final gate, wgpu Vulkan adapter enumeration failure; on failure it prints distro install commands, `vulkaninfo` verification and exits `78`, never a panic or black window. `DBLOCAL_GUI_HEADLESS=1` and the main-thread/display checks in `lib::gui_unavailable_reason` keep `dblocal` (no args) usable on a headless box and from test harnesses.

## Unverified here

Windows, macOS and the X11 backend cannot be built or run on this Linux/Wayland host: `-lxkbcommon-x11` needs the `libxkbcommon-x11-devel` package, which is not installed (see `docs/dev/linux-build-deps.md`). Tray wiring, quit dialogs and the real service are Wave 2.
