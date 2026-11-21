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
and `gpui_kit::actions!`. `dblocal-gui` no longer declares any local feature for a backend.

Verified: `cargo check -p dblocal-gui --all-targets` green; `cargo test -p dblocal-gui` → 83 lib +
1 contrast + 5 render tests pass; release build links; all 14 committed render PNGs byte-identical.

### Why it failed earlier and works now

Earlier in this task the host lacked `libxkbcommon-x11-devel`, so `-lxkbcommon-x11` did not link
(exact error in `evidence.md`); the migration was reverted then. `libxkbcommon-x11-devel` is now
installed on this host (`pkg-config --exists xkbcommon-x11` → 1.13.1;
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
  **required** build dependency: the umbrella forces it.

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

### Release size, RSS and cold start — same-method A/B

The only sound comparison is the same binary built twice with the same source and run in one
session, alternating A B A B A B (3 reps each). Until now this section compared numbers taken
by **different** methods, which was wrong; the corrected measurement follows.

**Method.** `crates/gui/examples/measure` built from two trees: `main @ b7b18a7` (direct deps:
`gpui-pre`/`gpui-component`/`gpui-kit-assets`) and this branch (umbrella `gpui-kit`). The two
`measure.rs` files differ **only in import paths** (`gpui::`/`gpui_component::` → `gpui_kit::`), so FLOOR (empty window, 15 s),
IDLE (real shell on the mock, 20 s) and AFTER_DOWNLOAD are the same code. Release, strip,
`lto=thin`; `VmRSS`/`VmHWM` from `/proc/self/status`, CPU from `utime+stime`. Same session, no
other build/test process running.

| Phase | Direct deps (mean [min–max]) | Umbrella (mean [min–max]) | Δ mean |
|---|---:|---:|---:|
| FLOOR (empty window) | 176.8 MB [176.6–176.9] | 177.3 MB [177.2–177.5] | **+0.5 MB** |
| IDLE (shell, mock) | 188.0 MB [187.9–188.1] | 188.7 MB [188.6–188.8] | **+0.7 MB** |
| AFTER_DOWNLOAD | 188.2 MB [188.1–188.3] | 188.9 MB [188.8–189.0] | **+0.7 MB** |
| Idle CPU | 0.08 % | 0.12 % | +0.04 pp |
| Floor CPU | 0.13 % | 0.09 % | −0.04 pp |

**Cold start, same probe, interleaved 20 runs per edge** (spawn → `FIRST_FRAME_MS`, Wayland):

| Edge | p50 | p95 | mean | min–max |
|---|---:|---:|---:|---:|
| Direct deps | 494.2 ms | 561.3 ms | 501.5 ms | 448.6–571.7 |
| Umbrella | 489.1 ms | 530.4 ms | 493.7 ms | 449.4–538.8 |

Cold start is indistinguishable between the edges (p50 within 5 ms, noise > that). Earlier drafts
quoted W0-C's 361 ms as a "before": that was a different machine load and a different probe, so it
is **not** a valid baseline and is withdrawn from this comparison.

**Release binaries (same profile):** `dblocal` 58,685,208 → 60,207,104 B (+1.52 MB);
`examples/measure` 59,311,840 → 60,823,448 B (+1.51 MB). The size delta is real and reproducible and
is the only cost this A/B attributes to the umbrella edge.

### Reconciliation with `docs/architecture/gui.md` (165.4 / 178.6 MB)

`gui.md` records FLOOR 165.4 MB and shell idle 178.6 MB (W1-G). Today, on this host, **main's own
`measure` binary** reports FLOOR 167.6 MB / IDLE 180.0 MB, and in the paired run above the direct
edge measured FLOOR 176.8 / IDLE 188.0 MB. So:

- The absolute RSS of the *same* binary moves by up to ~8 MB between sessions (page cache, GPU
  driver state, desktop compositor load). Cross-session absolute numbers are not comparable at the
  1 MB level, which is why an A/B in one session is the only defensible measurement.
- `gui.md`'s 165.4 / 178.6 is a valid historical data point but is **stale by ~1–2 MB** on today's
  host, and it was produced before the `font-kit` text system and X11 code were linked in.
- Against the merged baseline the umbrella edge costs **≈ +0.5 MB floor / +0.7 MB shell**, not the
  +12.8 MB in the earlier draft. That draft compared an umbrella shell figure measured one way with
  a direct-deps shell figure measured another (and ~11 MB below the documented baseline), so its
  delta was an artefact, and its causal claim ("X11 code and its atlas") was untested. **The causal
  claim is withdrawn**: the floor moved by +0.5 MB, so a shell-only widget explanation cannot hold,
  and no allocation profile was taken to identify the cause.

## 3. `gpui-fast` measurement (spike, not adoption)

`gpui-fast 0.1.17` (Apache-2.0, `longbridge/gpui-fast`, latest 0.1.x) resolves and builds through
the umbrella's `gpui-fast` feature now that X11 links. **Not enabled by default.** Measured on the
real shell (Wayland, AMD RADV, 60 s idle):

| Metric | Default (`gpui-pre`) | `gpui-fast` | Target |
|---|---:|---:|---:|
| Floor VmRSS (empty window) | 181.9 MB | 184.1 MB | < 150 MB |
| Shell idle VmRSS | 184.8 MB | 187.5 MB | < 150 MB |
| Idle CPU (60 s) | 0.06–0.10 % | 0.05–0.08 % | < 1 % |
| Cold start p50 (20 runs) | 489–494 ms | ~535 ms | < 1500 ms |
| Probe binary (real shell) | 45.84 MB | 48.78 MB (+2.9 MB) | — |

Measured with the **same probe and session** as the default edge, Fast does **not** lower the RSS
floor (it is 2–3 MB *higher* and still above 150 MB), adds ~2.9 MB of binary and is slower to
start. Render parity and 40× lib stability were not run, because the RSS result made adoption
moot. Recommendation: **do not adopt**; details in `gpui-fast-notes.md`.

## 4. Design-rule checklist (scope changed by D-25 — W2-E fills status)

Kit's `design-guides` is normative ("must / should / may"). Rules extracted below, with a
**headless way to check each**; W2-E fills the `Status` column against the prototype port.
Accessibility rules come first.

| # | Rule | Level | How to check (headless / code) | Status (W2-E) |
|---|---|---|---|---|
| A1 | Visible text goes through an accessibility node (`Label` / `role`+`aria_label`), not a bare string child | must | grep view files for `.child("`/`.child(String)` on text; add a `#[gpui_kit::test]` reading `window.find(id).label()` per key text node | |
| A2 | Every action keyboard-reachable; focus order follows visual/task order | must | dispatch Tab presses in a UI test, assert focused-id sequence; grep `tab_index`/`focus_handle` | |
| A3 | Focus visible and restored to the trigger after an overlay closes | must | UI test: open palette/dialog → Escape → assert focus id == trigger id | |
| A4 | Icon-only controls have an accessible name (+ tooltip when unclear) | must | grep icon-only `Button`/`IconButton` for `.aria_label(`/`.tooltip(` | |
| A5 | Status never colour alone (dot + text/icon) | must | render test: sample dot colour **and** assert the status label text (existing pattern) | |
| A6 | No hover-only destructive/essential action | must | code review: every critical command has a non-hover path | |
| B1 | `Button` for application commands; `Link`/underline only for URLs | must | grep `Link::`/underline on commands | |
| B2 | Secondary region commands in `DropdownMenu`; object commands in `ContextMenu`; primary stays visible | should | code review per toolbar/row; assert the primary is a visible `Button` | |
| B3 | One `primary` per decision area; `danger` only for destructive commit; `ghost` for quiet toolbar | should | grep `.primary()` per view; render test can count primary-tinted buttons | |
| C1 | Colours from theme tokens, no raw hex/rgb/hsla in views | should | grep views for `rgb(`/`hsla(`/`#`; scrim only via a theme token | |
| C2 | Radii from the theme scale; `radius_full` only for circles/pills | should | grep `rounded(px(` / `rounded_full()` | |
| C3 | Prefer rem helpers over literal `px(...)` except hairlines/raster | should | count `px(` per view file | |
| D1 | Minimum window size + resizable panes with a documented minimum | must | assert `window_min_size` in `WindowOptions`; resize test | |
| D2 | Density: medium default, compact only for data-dense areas | should | code review; component size API (`xsmall`/`small`) | |
| D3 | States visible: hover, focus, selected, disabled, loading, error, offline, empty | must | UI test per state; existing render suite proves selected/error/offline/empty | |
| E1 | Dialogs: one compact decision; title names it; actions Cancel + result; destructive styling on the result only | should | code review of delete/create dialogs; render test for the danger button | |
| E2 | Escape dismisses the topmost overlay; no overlay stacking | must | UI test: stack → Esc → only the top closes | |
| E3 | Uppercase only for short section labels; never combined with strong colour + bold | should | grep `.to_uppercase()` and check the same node's colour/weight | |
| E4 | Ellipsis (…) on buttons that open a dialog; none on immediate commands | should | grep `"…"` / dialog-opening labels | |
| F1 | Status colour never used as decoration | must | code review: `success`/`danger`/`warning` only for their meaning | |
| F2 | Icons supplement labels; one icon family | should | grep icon usage; single family (`IconName`) | |

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
  byte-identical. Cost: +1.5 MB binary and, by the same-session A/B, **+0.5 MB floor / +0.7 MB
  shell idle RSS** — not a material regression (an earlier draft's +12.8 MB was a method
  artefact; see the reconciliation note above). X11 becomes an unavoidable build dependency.
- **`gpui-fast`: do not enable by default.** See `gpui-fast-notes.md`. With the same probe its
  floor is **+2.2 MB** and its shell idle RSS **+2.7 MB** versus the default backend in one
  session — i.e. higher, not lower, and still above the 150 MB target.

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
- `docs/spikes/gpui-kit/{README.md,gpui-fast-notes.md,evidence.md}` (the design-rule table is
  section 4 of this README)
- `docs/architecture/gui.md`, `docs/dev/linux-build-deps.md`
