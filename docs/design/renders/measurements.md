# W1-G shell measurements (OPEN-08)

Host: Fedora 44, kernel 7.3.0-cachyos1, GNOME Shell 50.6, Wayland `wayland-0`, AMD Radeon (RADV RENOIR, Vulkan).
Binary: `cargo build -p dblocal-gui --release --example measure` (`strip`, `lto=thin`), run with a live Wayland session.

## Command

```sh
cargo build -p dblocal-gui --release --example measure
$HOME/.cache/dblocal-target-gui/release/examples/measure
```

Phases:

1. `FLOOR` — an empty GPUI window (`EmptyView` inside `gpui_component::Root`), 15 s sample. This is the GPUI + wgpu baseline; app code cannot go below it.
2. `IDLE` — the DBLocal shell (`Workspace::new`) on the in-memory mock, 20 s sample, no download running.
3. `AFTER_DOWNLOAD` — after a mock `create_instance` ran the full download path (5 progress samples, verify, extract, initialize), sampled for 8 s once idle again.

`VmRSS`/`VmHWM` come from `/proc/self/status`; CPU is `(utime + stime)` from `/proc/self/stat` divided by wall time.

## Results

| Phase | VmRSS | VmHWM | CPU | Target |
|---|---:|---:|---:|---|
| FLOOR (empty GPUI window) | **165.4 MB** | 165.4 MB | **0.20 %** | <150 MB, <1 % |
| IDLE (DBLocal shell, mock) | **178.6 MB** | 178.6 MB | **0.10 %** | <150 MB, <1 % |
| AFTER_DOWNLOAD | **178.7 MB** | 178.7 MB | **0.00 %** | <150 MB, <1 % |

Earlier run before the font stack change: FLOOR 166.0 / 0.20 %, IDLE 178.6 / 0.15 %, AFTER 178.7 / 0.00 %.

## What changed vs the W0-C spike

- **CPU: fixed.** The spike's 50 ms pump tick cost ~3.2 % at idle. The shell now bridges `DblService` events with a `flume` channel that the GPUI task `await`s; there is no timer while idle. Measured 0.10-0.20 %, an order of magnitude under the 1 % target.
- **RSS: not fixed, and the floor is the cause.** The *empty* GPUI window alone is 165 MB on this host, i.e. ≥15 MB above the 150 MB target before any DBLocal widget exists. The shell adds ~13 MB (dock, tabs, status bar, palette, image atlas). Per OPEN-08's instruction, the floor is not hacked around: it is wgpu/Vulkan + gpui's retained structures. App-level code has no lever to go below ~165 MB without changing the renderer or the toolkit.

## Fonts

`tracing` logs at startup:

- Before the fix: `UI font family: "Segoe UI"`, `Monospace font family: "Noto Sans Mono"`. GPUI maps `.SystemUIFont` through a stack that contains `Segoe UI`; this review host happens to have Segoe UI installed under `~/.local/share/fonts`, so the spike's "Segoe UI on Fedora" claim was literally reproducible here but is not a Linux font.
- After the fix: `UI font family: "Adwaita Sans"`, `Monospace font family: "Adwaita Mono"`. `theme::init` now names the first installed family from an explicit stack (`Adwaita Sans`, `Cantarell`, `Noto Sans`, `Ubuntu`, `DejaVu Sans`, …, `.SystemUIFont`), keeping the system font as the final fallback. Windows and macOS keep their native system font.
