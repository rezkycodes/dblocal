# GUI architecture (W1-G shell)

Owner: W1-G. This document describes the `crates/gui` shell built against `dblocal_core::service::DblService` and the in-memory mock. Real service wiring and tray integration land in later waves.

## Boundaries

- `crates/gui` is a **library** (`dblocal_gui::run()`). `crates/cli` is the only binary; no-args launches the GUI.
- The UI talks only to `Arc<dyn DblService>`. No direct state/engine/process access from GUI code.
- `tokio` work (mock timers, simulated downloads, future service I/O) runs on a dedicated multi-thread runtime owned by the GUI process. It never runs on the GPUI executor. UI updates cross into GPUI via an async channel (`flume`) polled by a lightweight GPUI task that `await`s the receiver and wakes only on real activity (no periodic idle timer).
- All user-facing UI strings live in `crates/core/src/strings.rs`. `crates/gui` must not contain literal UI sentences.

## Modules

| Module | Responsibility | Tests |
|---|---|---|
| `lib.rs` | Public `run()` and `run_with_service` seam | integration/headless |
| `app.rs` | GPUI window bootstrap, Root, key bindings, event bridge | layout (render) |
| `vulkan.rs` | Vulkan pre-flight (exit 78, actionable text) | unit + report |
| `theme.rs` | Load One Dark/Light JSON themes, system sync, font reporting | unit |
| `contrast.rs` | Parse rendered theme JSON and assert WCAG 4.5:1 pairs | unit (enforces contract) |
| `format.rs` | Pure view-model helpers: bytes/speed/ETA, status labels, URI masking, palette filtering, validation hints | unit, >=80% |
| `service_bridge.rs` | `flume` channel bridge: tokio/broadcast -> GPUI wake | unit |
| `workspace.rs` | Zed-style shell: title bar, docks, tabs, status bar, modals, toasts, command palette wiring | render/layout |
| `logs.rs` | Ring buffer + virtualised log view + follow/copy | unit + render |
| `assets/themes/*.json` | Theme source of truth (loaded by `include_str!`) | read by contrast tests |

`workspace.rs` may stay under the 800-line hard limit by extracting pure sub-views into small sibling modules if it grows; view-model logic stays out of render code.

## Event flow (no polling)

1. `service.subscribe()` returns a `broadcast::Receiver<ServiceEvent>` (tokio).
2. A tokio task forwards events into an unbounded `flume::Sender<UiEvent>` (coalescing high-frequency progress if needed).
3. The GPUI app spawns an async task: `while let Ok(ev) = rx.recv_async().await { view.update(...); cx.notify(); }`. It parks on the channel — zero CPU while idle.
4. Log streaming is opt-in per opened instance via `watch_logs(id, true/false)`.

## Themes and contrast

- Dark: `DBLocal One Dark`, light: `DBLocal One Light`, based on Atom One palettes. Source files: `crates/gui/assets/themes/one-dark.json`, `one-light.json`, loaded with `include_str!` and registered via `ThemeRegistry::load_themes_from_str`.
- The contrast test parses the **same JSON files** (not constants) and checks every `*.foreground` on its paired `*.background` and base `background`/`sidebar.background`, plus `foreground`, `muted.foreground`, and status/semantic colours, in both themes, failing below 4.5:1. Disabled controls are exempt. Status is dot + text label.
- Known W0-C light failures fixed in tokens (not in the test): `primary/info/sidebar.primary` foreground/background pairs (~3.88:1) and `muted.foreground` on `muted.background` (~4.35:1). Dark pairs already passed.

## Fonts

- Default: system font resolved by the platform (`.SystemUIFont` / theme default). GPUI's text system resolves the actual family at runtime; the app logs the resolved UI and mono families on startup for diagnostics.
- Linux fallback stack is explicit/sensible via theme defaults (no Windows-only assumption). No bundled font in this wave; if a font is bundled later it must ship its SIL/OFL licence.

## Performance (OPEN-08)

No 50 ms timer. The idle baseline is measured as: (1) empty GPUI window (floor), (2) app idle with mock service, (3) app after a mock download completes. Report all three. If the GPUI floor exceeds 150 MB RSS, that is reported with evidence rather than worked around in app code.

## Vulkan

`vulkan::preflight()` runs before `gpui_platform::application()`. It detects missing/invalid ICD manifests/overrides and, as a final gate, wgpu Vulkan adapter enumeration failure; on failure it prints distro install commands, `vulkaninfo` verification, and exits `78`. `DBLOCAL_SPIKE_SKIP_PREFLIGHT=1` is a test-only escape hatch (not for release). No panic, no black window.
