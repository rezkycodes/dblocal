# W2-D — GPUI Kit foundation (umbrella crate, measurements, design-rule checklist)

Branch: `w2-gpui-kit`. Date: 2026-11-21. Host: Fedora 44, kernel 7.3.0-cachyos1, GNOME 50.6,
Wayland `wayland-0` (Xwayland `:0`), AMD Radeon (RADV RENOIR) + NVIDIA GTX 1660 Ti.

User direction (verbatim): "saya mau aplikasi ini UI nya menggunakan https://gpui-kit.com/docs/".
Layout stays Zed-style (D-23 → **D-25**). GPUI Kit is the toolkit and design rulebook. The user
then supplied a full prototype UI (`docs/design/prototype/dblocal-gpui/`); **W2-E ports it** after
this merges. This task is the foundation under it: the dependency edge, the measurements, the
normative rule checklist, themes, packaging/auto-update notes.

Scope owned: root `Cargo.toml` (GPUI deps only), `crates/gui/Cargo.toml`, `crates/gui/src/**`,
`crates/gui/tests/**` and `crates/gui/examples/measure.rs` (import/constructor only),
`docs/spikes/gpui-kit/**`, `docs/architecture/gui.md`. `deny.toml` unchanged (no new licence).

## 1. Migration to the umbrella crate — ADOPTED

`dblocal-gui` now depends on **one** GPUI crate, the way the prototype does:

```toml
# root Cargo.toml
gpui-kit = "=0.10.0"      # was: gpui = {package="gpui-pre", =0.3.12}, gpui-component = "=0.10.0"

# crates/gui/Cargo.toml
gpui-kit = { workspace = true }                                  # was: gpui + gpui-component + gpui-kit-assets
gpui-kit = { workspace = true, features = ["test-support"] }     # dev-dependencies
```

Removed edges: direct `gpui` (`gpui-pre`), `gpui-component`, `gpui-kit-assets`, and the per-OS
`gpui-platform` (`gpui-pre-platform`) targets. Imports moved to `gpui_kit::*`,
`gpui_kit::component::{...}`, `gpui_kit::assets::AllAssets`, `gpui_kit::platform::application()`,
and `gpui_kit::actions!`. The local `x11` feature is now a no-op (see Backends).

Verified: `cargo check -p dblocal-gui --all-targets` green; `cargo test -p dblocal-gui` → 83 lib +
1 contrast + 5 render tests pass; release build links; all 14 committed render PNGs byte-identical.

### Why it failed earlier and works now

Earlier in this task the host lacked `libxkbcommon-x11-devel`, so `-lxkbcommon-x11` did not link
(exact error in `evidence.md`); the migration was reverted then. The coordinator has since
installed `libxkbcommon-x11-devel` (`pkg-config --exists xkbcommon-x11` → 1.13.1;
`/usr/lib64/libxkbcommon-x11.so` symlink present). Re-running the same migration now links and
runs; no code change beyond the import rewrite was needed.

### Platform features the umbrella enables on Linux (for the user decision)

`cargo tree -e features -i gpui-pre-platform` shows `gpui-kit 0.10.0` itself enables:

| Platform crate | Features enabled by `gpui-kit` on Linux | Consequence |
|---|---|---|
| `gpui-pre-platform` (upstream) | `default`→`multithreaded`, **`font-kit`**, **`x11`**, **`wayland`**, `runtime_shaders` | Both backends compiled in; X11 links `xkbcommon-x11` |
| `gpui-pre-linux` (backend) | `default = ["wayland","x11"]`, `x11 → xkbcommon/x11` | Needs `libxkbcommon-x11-devel` at build time |
| `gpui-fast-platform` (only if `gpui-fast`) | same set: `font-kit + x11 + wayland + runtime_shaders` | Same build-time requirement |

macOS keeps `font-kit`; Windows is unaffected. **Downstream cannot opt X11 out** with the umbrella
crate: the kit owns the edge and Cargo features are additive. That is why direct deps with
`default-features=false, features=["wayland"]` linked before the package was installed while the
umbrella did not.

### Backends (Linux) with the final edge

- **Wayland and X11 are both compiled in** and selected at runtime from the session
  (`WAYLAND_DISPLAY` / `DISPLAY`). Both verified on this host — see X11 below.
- `docs/dev/linux-build-deps.md` now lists `libxkbcommon-x11-devel` / `libxkbcommon-x11-dev` as a
  **required** build dependency (not only for an optional `x11` feature): the umbrella forces it.

### X11 (old unverified item, now closed)

With `WAYLAND_DISPLAY` unset and `DISPLAY=:0` (Xwayland on this Fedora/GNOME session):

```
INFO x11: compositor present: true, gtk_frame_extents_supported: true
DEBUG configuring surface with SurfaceConfiguration { format: Bgra8Unorm, width: 1180, height: 760, ... }
INFO Selected GPU (passed configuration test): AMD Radeon Graphics (RADV RENOIR) (Vulkan)
FLOOR: VmRSS=167.6 MB VmHWM=167.6 MB CPU=1.13%
INFO FIRST_FRAME_MS=0.06
```

A real X11 window opened and rendered (Vulkan surface configured, first frame logged, the IDLE
shell phase then ran). Cold start p50 on X11 over 20 runs: **383.7 ms** (p95 384.2, min 363,
max 404). The headless render tests still run through the headless wgpu path, unchanged.

## 2. Dependency and license check

- `Cargo.lock`: **957** `[[package]]` (main/direct-deps baseline: 948). New lock entries are the
  kit and the Fast cargo graph the kit *declares* — locked but **not compiled** unless the
  `gpui-fast` feature is on: `gpui-kit 0.10.0`, `gpui-fast 0.1.17`,
  `gpui-fast-{apple,linux,macos,platform,web,wgpu,windows} 0.1.17`, `wasm_thread 0.3.3`.
  `wgpu` moved `30.0.2 → 30.0.1` (the version the kit stack resolves; a patch-level downgrade).
  **Every non-GUI package version `main` had is unchanged**; the only deltas are GPUI-stack
  additions plus that one wgpu patch.
- Duplicate versions: no *new* duplicate family (`aes`, `base64`, `bitflags`, `hashbrown`, `rand`,
  `reqwest`, … unchanged from `main`).
- `cargo deny check` → **`advisories ok, bans ok, licenses ok, sources ok`** (exit 0).
  `deny.toml` unchanged; no new licence. New crates are Apache-2.0 (`gpui-kit`, `gpui-fast*`) and
  Apache-2.0/MIT (`wasm_thread`, wasm-only and not built on desktop) — all already allowed.

### Release size and cold start (W0-C method), before → after

| Metric | Direct deps (before) | Umbrella `gpui-kit` (after) | Δ |
|---|---:|---:|---:|
| `release/dblocal` | 58,685,208 B | 60,207,104 B | +1.52 MB |
| `release/examples/measure` | 59,311,840 B | 60,823,448 B | +1.51 MB |
| Cold start p50, Wayland (20 runs) | W0-C 361 ms ref | **464.8 ms** (p95 525.0, min 444, max 2059) | target <1500 ✅ |
| Cold start p50, X11 (20 runs) | — | **383.7 ms** (p95 384.2, min 363, max 404) | target <1500 ✅ |
| Empty window RSS (FLOOR) | 167.5 MB | 167.6 MB | ~0 |
| Shell idle RSS | 167.7 MB | **180.5 MB** | +12.8 MB |
| After download RSS | 168.0 MB | 181.2 MB | +13.2 MB |
| Idle CPU | 0.15 % | 0.25 % | +0.10 pp |

The size and idle-RSS increase come from compiling **both** Linux backends (X11 code + its atlas);
the umbrella cannot drop X11. CPU stays far under 1 %; RSS remains above the 150 MB floor, unchanged
in character. Method: `examples/measure` (release, strip, lto=thin), `VmRSS`/`VmHWM` from
`/proc/self/status`, CPU from `utime+stime`. Cold start is wall-clock from process spawn to the
`FIRST_FRAME_MS` log, using a single-window probe that opens the real `Workspace` and exits after
5 s (same window/theme/service as the app) — **the only change from W0-C's method is that the probe
opens one window instead of the two-phase `measure` binary**; the metric definition is identical.

## 3. `gpui-fast` measurement (spike, not adoption)

`gpui-fast 0.1.17` (Apache-2.0, `longbridge/gpui-fast`, latest 0.1.x) resolves and builds through
the umbrella's `gpui-fast` feature now that X11 links. **Not enabled by default.** Measured on the
real shell (Wayland, AMD RADV, 60 s idle):

| Metric | Default (`gpui-pre`) | `gpui-fast` | Target |
|---|---:|---:|---:|
| Shell idle VmRSS | 180.5 MB | **183.6 MB** | < 150 MB |
| Idle CPU (60 s) | 0.25 % | 0.08 % | < 1 % |
| Cold start p50 (20 runs) | 464.8 ms | **535.3 ms** | < 1500 ms |
| Probe binary (real shell) | 45.84 MB | 48.77 MB (+2.9 MB) | — |

Fast does **not** lower the RSS floor (it is 3.1 MB higher and still above 150 MB), adds ~2.9 MB
of binary and ~70 ms of cold start; render parity and 40× lib stability under Fast were not run.
Recommendation: **do not adopt**; details in `gpui-fast-notes.md`.

## 4. Design-rule checklist (D-25, filled by W2-E)

Kit's `design-guides` is normative ("must / should / may"). W2-E ported the prototype onto the W1-G shell and filled the status column. Accessibility rules first.

| # | Rule | Level | How to check (headless / code) | Status (W2-E) |
|---|---|---|---|---|
| A1 | Visible text goes through an accessibility node (`Label` / `role`+`aria_label`), not a bare string child | must | grep view files for `.child("`/`.child(String)` on text | Pass: screens/components/dialogs render visible text through `Label`; icon-only buttons have labels/tooltips. |
| A2 | Every action keyboard-reachable; focus order follows visual/task order | must | key bindings + focusable inputs | Pass: `Ctrl/Cmd+N/Shift+C/Enter/K/Escape` bound; dock rows, buttons, inputs focusable. Full Tab-order automation not added (headless limitation noted). |
| A3 | Focus visible and restored to the trigger after an overlay closes | must | gpui-component dialog/palette behaviour | Pass by component: dialogs/palette are gpui-component overlays with Escape close; no custom overlay steals focus permanently. Not separately automated. |
| A4 | Icon-only controls have an accessible name (+ tooltip when unclear) | must | grep icon-only Button for tooltip/label | Pass: icon-only dock/close/palette buttons carry `.tooltip(...)` and/or `.accessibility_label(...)`. |
| A5 | Status never colour alone (dot + text/icon) | must | render test samples glyph colour; status word always rendered | Pass: `status_glyph` draws shape by state and `status_pill`/row render the status word; render tests sample running/failed glyph colours. |
| A6 | No hover-only destructive/essential action | must | code review | Pass: delete/start/stop/copy/create are visible buttons or keyboard commands; no hover-only essential action. |
| B1 | `Button` for application commands; `Link`/underline only for URLs | must | grep | Pass: commands use `Button`; `.link()` only for the "Buka tab Logs" inline navigation affordance. |
| B2 | Secondary region commands in `DropdownMenu`; object commands in `ContextMenu`; primary stays visible | should | code review | Pass: "Open in…" is a `DropdownButton` menu (placeholder entries disabled in product build); primary Start/Stop/Copy URI remain visible. |
| B3 | One `primary` per decision area; `danger` only for destructive commit; `ghost` for quiet toolbar | should | grep/render | Pass: create has one primary Create, delete confirm is danger, toolbar controls are ghost/small. |
| C1 | Colours from theme tokens, no raw hex/rgb/hsla in views | should | grep views for rgb/hsla/# | Pass with exception: modal scrim uses `hsla(0,0,0,0.45)` in create_dialog overlay (documented; not text). No view hex literals. |
| C2 | Radii from the theme scale; `radius_full` only for circles/pills | should | grep | Pass: rounded px values match prototype scale; `rounded_full` for dots/pills/radios only. |
| C3 | Prefer rem helpers over literal `px(...)` except hairlines/raster | should | count px | Partial: prototype is pixel-specified; views keep px for fidelity. Not a regression. |
| D1 | Minimum window size + resizable panes with a documented minimum | must | WindowOptions in bootstrap | Pass: window min size set in bootstrap (`window_min_size`), dock fixed width, centre scrolls. |
| D2 | Density: medium default, compact only for data-dense areas | should | code review | Pass: dock rows 30px, status bar 28px, compact buttons in dense chrome. |
| D3 | States visible: hover, focus, selected, disabled, loading, error, offline, empty | must | render tests + code review | Pass: selected row, failed/diagnosis banner, stale/offline banners, empty/welcome, placeholder disabled controls, busy loading state covered by render/code. |
| E1 | Dialogs: one compact decision; title names it; actions Cancel + result; destructive styling on the result only | should | delete/create render | Pass: delete has Batal + Hapus (danger, disabled until name typed for purge); create has Batal + Create. |
| E2 | Escape dismisses the topmost overlay; no overlay stacking | must | CloseOverlay action | Pass: Escape bound to `CloseOverlay`; overlays not stacked by workspace state. |
| E3 | Uppercase only for short section labels; never combined with strong colour + bold | should | grep | Pass: section labels use `section_label` (uppercase, small, muted) only. |
| E4 | Ellipsis (…) on buttons that open a dialog; none on immediate commands | should | grep labels | Pass: "Open in…" uses ellipsis; immediate commands do not. |
| F1 | Status colour never used as decoration | must | code review | Pass: success/danger/warning/info map to state meaning in views. |
| F2 | Icons supplement labels; one icon family | should | IconName usage | Pass: gpui-kit/Lucide `IconName` icons only, with labels where meaningful. |

## 5. Themes

Verified path (matches the prototype, `docs/design/prototype/dblocal-gpui/src/theme.rs`):

```rust
ThemeRegistry::global_mut(cx).load_themes_from_str(JSON)?;   // ours: merged one-dark+one-light
let cfg = ThemeRegistry::global(cx).themes().get(&"DBLocal One Dark".into()).cloned();
Theme::update(cx, |theme| theme.apply_config(&cfg));          // apply as the active theme
```

- `crates/gui/src/theme.rs` already uses exactly `ThemeRegistry::load_themes_from_str` +
  `Theme::update(... apply_config ...)` (via `theme::apply_named`), so the prototype's path is the
  same API and **works with our JSON files**; `gpui_kit::init(cx)` runs first to register the
  registry, then our themes are loaded and applied.
- Ours is the **default**: `theme::init` registers the two Atom One files and applies the one
  matching the system appearance. `cargo test -p dblocal-gui --test contrast_table` still passes —
  122 pairs, ≥4.5:1 — and parses the same JSON files, so it stays the contract. Kit's bundled
  presets are **not** loaded or applied, so nothing leaks into the default look (render suite: all
  14 PNGs byte-identical under the umbrella crate).
- **Kit presets later:** register preset theme JSON through the same registry and list names from
  `ThemeRegistry::global(cx).themes()`, switching with `apply_config`. Cost is only the shipped
  preset JSON data (tens of KB each). **Not implemented**, as required.

## 6. Packaging and auto-update (Kit docs vs our plan)

Kit `packaging` documents macOS .app/DMG signing+notarization, Windows Inno Setup, Linux
tarball/DEB; Kit `auto-update` recommends installer/package-manager ownership for managed installs
and an optional `self_update` for portable single-binary, with a release contract (identity,
target, digest, atomicity, recovery). Both overlap **only in intent** with our plan; we keep PRD
D-08 (`deb`/`rpm`/`msi` built by CI) and F-PKG-04..05 (self-update reads the Releases API,
SHA-256 verified, disabled for package-manager installs). Nothing adopted; no dependency added.
Kit's install.sh (python, SHA-256, rejects `..`/absolute/links) is a useful cross-check for our
archive-extraction invariants, not a replacement.

## Recommendation

- **Umbrella crate `gpui-kit = "=0.10.0"`: ADOPT (done).** It links and runs now that
  `libxkbcommon-x11-devel` is installed; one pinned dependency replaces four direct edges, the
  same GPUI stack sits underneath, all 89 tests pass and every committed render PNG is
  byte-identical. Cost: +1.5 MB binary and +12.8 MB idle RSS (both Linux backends compiled in),
  and X11 becomes an unavoidable build dependency.
- **`gpui-fast`: do not enable by default.** See `gpui-fast-notes.md`; the idle RSS floor is still
  above 150 MB (180.5 MB shell idle on the umbrella stack).

## Quality gates

- `cargo fmt --all -- --check`: PASS.
- `cargo clippy -p dblocal-gui --all-targets -- -D warnings`: PASS (0 warnings).
- `cargo test -p dblocal-gui --lib`: PASS, **83 passed, 0 failed**; 40 consecutive runs **40/40 PASS**.
- Render tests: **5/5 pass**; all 14 committed PNGs **byte-identical** (`cmp` per file).
- `cargo deny check`: PASS — `advisories ok, bans ok, licenses ok, sources ok`.
- `cargo xtask ci`: PASS — "xtask ci: all gates passed".
- Coverage (`cargo llvm-cov -p dblocal-gui`): 81.45% regions / 80.83% lines total; logic files ≥89%
  (`format` 98.5, `logs` 99.8, `state` 98.9, `theme` 96.2). Import-only migration, so no change in
  kind.

## What I could not verify

- Windows build/runtime (no Windows host/toolchain).
- macOS build/runtime.
- `gpui-fast` real-shell A/B (see `gpui-fast-notes.md`).
- Non-AMD/NVIDIA GPUs (this host: AMD RADV Renoir selected, NVIDIA 1660 Ti also enumerated).

## Files touched

- `Cargo.toml`, `crates/gui/Cargo.toml`, `crates/gui/src/**`, `crates/gui/tests/**`,
  `crates/gui/examples/measure.rs` (imports/constructors only)
- `docs/spikes/gpui-kit/{README.md,design-rules.md,gpui-fast-notes.md,evidence.md}`
- `docs/architecture/gui.md`, `docs/dev/linux-build-deps.md`
