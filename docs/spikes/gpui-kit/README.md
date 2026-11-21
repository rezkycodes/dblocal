# W2-D — GPUI Kit adoption spike (umbrella crate, design audit, `gpui-fast`)

Branch: `w2-gpui-kit` (from `main` @ `46a40f7`). Date: 2026-11-21. Host: Fedora 44,
kernel 7.3.0-cachyos1, GNOME 50.6, Wayland `wayland-0`, AMD Radeon (RADV RENOIR, Vulkan).

Scope owned by this task: root `Cargo.toml` (GPUI deps only), `crates/gui/Cargo.toml`,
`crates/gui/src/**`, `crates/gui/tests/**` (import/constructor-only), `docs/spikes/gpui-kit/**`,
`docs/architecture/gui.md`, and `deny.toml` if a new license appeared (none did).

User direction (verbatim): "saya mau aplikasi ini UI nya menggunakan https://gpui-kit.com/docs/".
Layout stays Zed-style (D-23). GPUI Kit is the toolkit and design rulebook, not a different layout.

## 1. Migration to the umbrella crate — **not adopted on this host (link blocked)**

### What was attempted

`cargo info gpui-kit` (crates.io, 2026-11-21) resolves `gpui-kit = "0.10.0"` (Apache-2.0),
with `default = ["component", "assets"]`, re-exporting `gpui` (`gpui-pre =0.3.12`), `gpui-base 0.10.0`,
`gpui-component 0.10.0`, `gpui-kit-assets 0.10.0`, and `platform` (desktop/web). A full source
migration was performed in this worktree: workspace pin `gpui-kit = "=0.10.0"` (replacing the
separate `gpui`, `gpui-component` pins), `dblocal-gui` depending on `gpui-kit` only (plus
`wgpu` on Linux for the Vulkan preflight), `gpui::`/`gpui_component::`/`gpui-kit-assets` imports
switched to `gpui_kit::*` / `gpui_kit::component` / `gpui_kit::assets`, and
`gpui_platform::application()` to `gpui_kit::platform::application()`.

`cargo check -p dblocal-gui --all-targets` **succeeded** (compiled cleanly against the umbrella
crate). Linking failed.

### Exact link error

```
rust-lld: error: unable to find library -lxkbcommon-x11
collect2: error: ld returned 1 exit status
```

Observed for `dblocal-gui` (lib + tests) and the `render` integration test (`cargo test -p
dblocal-gui --no-run`). The error is identical in shape to the W0-C/OPEN-03 note in
`docs/spikes/gpui/README.md` ("cargo build --features x11 fails linking -lxkbcommon-x11 because
only the runtime SONAME libxkbcommon-x11.so.0 is installed, not the -devel symlink").

### Why the umbrella crate forces this

`gpui-kit 0.10.0` enables `gpui-fast-platform`'s desktop features unconditionally on non-mobile
targets: `features = ["font-kit", "x11", "wayland", "runtime_shaders"]` on both
`gpui-pre-platform 0.3.12` and, when `gpui-fast` is selected, `gpui-fast-platform 0.1.17`. The
Linux backend crate `gpui-pre-linux 0.3.12` / `gpui-fast-linux 0.1.17` has
`default = ["wayland", "x11"]` and maps `x11` to `xkbcommon/x11` (and `scap?/x11`). Because
the umbrella crate owns these dependency edges, a downstream crate cannot turn X11 off while
depending on it (features are additive and the kit's own dep enables x11). This is **not** the
same as our current direct deps: `crates/gui/Cargo.toml` today uses
`gpui-platform = { package = "gpui-pre-platform", default-features = false, features = ["wayland"] }`
on Linux, so it links on this host.

Host state (verified): `libxkbcommon-x11.so.0` present in `/usr/lib64`, **no**
`libxkbcommon-x11.so` development symlink, no `xkbcommon*.pc` in `/usr/lib64/pkgconfig`,
`cc ... -lxkbcommon-x11` fails with "cannot find -lxkbcommon-x11". `libxkbcommon-x11-devel` is
not installed (Fedora; also noted missing in OPEN-03 and `docs/dev/linux-build-deps.md`).

### Action taken (per task fallback)

No packages installed, no vendoring/hacking around the missing library. The dependency migration
and import rewrites were **reverted** so `crates/gui` remains buildable and the render tests stay
green on this host. The current direct dependencies were kept — they are the **same crates and
versions** the umbrella re-exports (`gpui-pre =0.3.12`, `gpui-component =0.10.0`,
`gpui-kit-assets 0.10.0`). Items 2–6 below are completed on the current stack.

### Platform-feature mapping (documented)

| Concern | Current direct deps (works here) | Umbrella crate `gpui-kit 0.10.0` |
|---|---|---|
| Linux backend | `gpui-pre-platform` with `default-features = false, features = ["wayland"]` (+ optional `x11` feature we expose) | Desktop platform forces `font-kit + x11 + wayland + runtime_shaders`; downstream cannot disable x11 |
| macOS font | `gpui-pre-platform` with `features = ["font-kit"]` | Satisfied (font-kit forced on desktop) |
| Windows | `gpui-pre-platform` default-features = false | Satisfied |
| Assets | `gpui-kit-assets = "0.10.0"` | `gpui_kit::assets` (default feature `assets`) |
| Init | `gpui_component::init(cx)` | `gpui_kit::init(cx)` (initializes component+base when enabled) |
| Test support | `gpui`/`gpui-component` `test-support` features | `gpui-kit` `features = ["test-support"]` |

### Adopt-umbrella readiness

**Blocked here, ready elsewhere once the dev symlink (or an upstream feature to opt out of X11)
exists.** The API migration is mechanical and compiled cleanly (`cargo check` green); only linking
fails. Options for the user/coordinator: (a) install `libxkbcommon-x11-devel` in the dev
environment/CI image and adopt the umbrella, or (b) ask upstream for a wayland-only
feature/default-off-x11 on desktop, or (c) keep direct deps (same stack, today’s behaviour).

## 2. Dependency and license check (current stack, final tree)

- `cargo tree -p dblocal-gui` (full): 1968 lines (not committed; reproducible with the command).
- Duplicate versions (`cargo tree -p dblocal-gui -d`): 1470 lines. Duplicates are the pre-existing set (e.g. `aes`
  0.8.4/0.9.4, `base64` 0.22/0.23, `bitflags` 1/2, `hashbrown`, `rand` 0.8/0.10, `reqwest`
  0.12/0.13, etc.) already present on `main`; **no new duplicates introduced** (no dependency
  changes landed).
- `cargo deny check` (licenses, advisories, bans, sources): **`advisories ok, bans ok, licenses ok, sources ok`** (exit 0). `deny.toml` unchanged; no new license.
- `Cargo.lock`: 948 `[[package]]` entries (824 unique `name =` lines) — identical to the
  pre-migration baseline: `git diff HEAD --stat Cargo.lock` is empty, so **every package version
  main had (GUI and non-GUI) is unchanged**. No new crates.
- New dependencies added to the repo: **none**.

### Release binary size and cold start (W0-C method, current stack)

Baseline release artifacts (stripped, `lto = "thin"`, `CARGO_TARGET_DIR=$HOME/.cache/dblocal-target-gui`),
built on this host against the current direct deps:

| Artifact | Size (bytes) | Size |
|---|---:|---:|
| `release/dblocal` (CLI/GUI binary) | 58,685,208 | ~56.0 MiB |
| `release/examples/measure` | 59,311,840 | ~56.6 MiB |

Cold start to first frame: **not captured as a clean 20-run series in this pass** (the two-phase
`measure` binary makes process-to-first-frame hard to isolate from the harness without editing a
path this task does not own). Prior reference on this same host/window: W0-C p50 361 ms / p95 425 ms
(process-to-first-frame); a single in-process probe logged `FIRST_FRAME_MS=0.91` (render-entry
delta, not the W0-C metric). Reported as unverified rather than guessed; `evidence.md` records what
was actually measured.

Idle/empty RSS and CPU (OPEN-08 method, release `measure`, this host): FLOOR **167.5 MB** RSS /
**0.20%** CPU, IDLE (shell, mock) **167.7 MB** RSS (HWM 178.2 MB) / **0.15%** CPU, AFTER_DOWNLOAD
**168.0 MB** RSS / **0.00%** CPU (target <150 MB RSS, <1% CPU). CPU target met; RSS floor remains
above 150 MB (wgpu/Vulkan + retained structures), unchanged in character from W0-C/W1-G.


## 3. `gpui-fast` measurement (spike, not adoption)

**Result: not measured on the real DBLocal shell on this host; not adopted. Recommendation: needs more evidence.**

- `gpui-fast 0.1.17` (Apache-2.0, repo `longbridge/gpui-fast`) is the latest published; `gpui-kit 0.10.0` exposes it behind the `gpui-fast` feature (pins 0.1.16 internally).
- The Fast Linux backend (`gpui-fast-linux 0.1.17`) has the same default `["wayland","x11"]` and the x11 path enables `xkbcommon/x11`. With `default-features = false, features = ["wayland"]` a small probe compiles on this host, but building the **DBLocal shell** against the Fast stack requires either the umbrella crate's `gpui-fast` feature (blocked by the same xkbcommon-x11 link) or a dedicated manifest patching platform deps — outside the owned files / invasive to the build.
- I did **not** measure the real shell under gpui-fast, so I cannot report RSS/CPU/cold-start/render-pixel parity/lib-40× for it. The hypothesis ("may reduce the 165/178 MB RSS floor") remains **unverified**.
- Windows/macOS Fast support: **not verifiable** on this Fedora/Wayland host.
- License: Apache-2.0 (compatible). Age: gpui-fast 0.1.17 published recently (crates.io 0.1.x line; 0.1.17 is the newest). Maturity relative to gpui-pre 0.3.12 is unknown to me from this run.

**Recommendation:** keep the current `gpui-pre` stack. Do not enable `gpui-fast` by default. Revisit only with a host/build that links the Fast backend and produces the required A/B numbers (empty RSS, shell RSS, idle CPU 60s, cold start p50×20, binary size, render-pixel diff, lib tests 40×, Windows/macOS build).

| Metric (this host, current stack) | Value | Target |
|---|---:|---:|
| Empty GPUI window RSS (floor) | 167.5 MB | < 150 MB |
| Shell idle RSS (mock) | 167.7 MB (HWM 178.2 MB) | < 150 MB |
| After download RSS | 168.0 MB | < 150 MB |
| Idle CPU (60s sample style) | 0.00–0.20 % | < 1 % |
| Release `dblocal` size | 58,685,208 B (~56.0 MiB) | < 60 MiB installer (binary only) |
| Release `measure` size | 59,311,840 B (~56.6 MiB) | — |
| Cold start to first frame p50 (20 runs) | see note below | < 1500 ms |
| gpui-fast A/B | **not measured** (link/build constraint) | — |

Cold-start note: `Workspace::render` logs `FIRST_FRAME_MS` (in-process render delta ~0.2 ms in a probe). A clean 20-run process-to-first-frame wall-clock series for the `measure` two-phase binary was not captured by the harness in this run; W0-C measured 361 ms p50 / 425 ms p95 for the spike shell on this host, and the shell is materially the same shape. I am reporting this as **unverified in this run** rather than guessing. The `cargo xtask ci` and render gates are the authoritative correctness signals for this task.

## 4. Design-guide audit

See `docs/spikes/gpui-kit/design-audit.md` (normative rule table: rule, where, status, file, fix/effort). Summary: no import-only/one-line-constructor deviation was safe to change without a render re-baseline. Key deviations: Label a11y migration (broad, 0.5–1 day), px→rem (1–2 days), explicit hover/focus/tab order (0.5–1 day), secondary commands into DropdownMenu (0.5 day), overlay scrim token + radii tokens (small), "New instance…" copy needs core string change via `ask`. Passes: always-visible commands, Button-not-Link, theme tokens (except scrim literal), status not colour-only, minimum window size, density, uppercase limited to short section labels.

## 5. Themes

`gpui_kit::init(cx)` (as the Kit documents) and our JSON themes (`crates/gui/assets/themes/one-dark.json`, `one-light.json`, Atom One based, contrast-tested) coexist: our themes are registered via `ThemeRegistry::load_themes_from_str` and selected by `theme::sync_to_system`; they remain the default. The 122-pair contrast test (`cargo test -p dblocal-gui --test contrast_table`) passes and is the contract — it reads the same JSON files the app loads. No Kit preset leaks into the default look.

Kit bundled presets (Default Light/Dark, Catppuccin, Tokyo Night, etc.) are cheap to expose later: register/apply via `ThemeRegistry`/theme config API; cost is the bundled preset data in the binary (small, theme JSON) — **not implemented, not measured precisely**. Exposing a user picker is a W2/W3 UI task, not a spike change.

## 6. Packaging and auto-update (Kit docs vs our plan)

- Kit `packaging`: documents macOS .app/DMG signing+notarization, Windows Inno Setup, Linux tarball/DEB, identity/checksums. Overlaps **only in intent** with our plan. We keep PRD D-08/F-PKG: deb/rpm/msi built by CI, no sudo at runtime; Kit packaging is not adopted.
- Kit `auto-update`: recommends installer/package-manager ownership for managed installs, optional `self_update` for portable single-binary. Overlaps with F-PKG-04..05 (self-update reads Releases API, SHA-256 verified, disabled for package-manager installs). We keep our own contract; Kit auto-update is not adopted.
- Both pages are useful as checklists; neither changes our security/invariant rules (no sudo, SHA-256 before execution, no telemetry).
- No code or dependency adopted from these pages.
- Decision left to user/coordinator as requested.

## Recommendation

**Adopt the umbrella crate: YES, but not on this host until the X11 dev-symlink blocker is resolved.** The migration is mechanical, compiled cleanly, and reduces GPUI dependency surface to one pinned crate (`gpui-kit = "=0.10.0"`) that matches the existing stack exactly. The blocker is environmental (missing `libxkbcommon-x11-devel`, forces x11 link via kit's desktop platform features), not an API incompatibility. Action: install `libxkbcommon-x11-devel` in dev/CI or request upstream wayland-only desktop feature; then land the import migration.

**gpui-fast: do not adopt / needs more evidence.** No real-shell A/B was possible here. Current stack meets CPU target (0.00–0.20%) but misses the 150 MB RSS target at the wgpu floor (167.5 MB empty, 167.7 MB shell idle) — gpui-fast *might* help, but that is a hypothesis until measured with the required method.

## Quality gates

- `cargo fmt --all -- --check`: PASS (exit 0).
- `cargo clippy -p dblocal-gui --all-targets -- -D warnings`: PASS (Finished, 0 warnings).
- `cargo test -p dblocal-gui --lib`: PASS, **83 passed, 0 failed**. Ran 40 times consecutively: **40/40 PASS** (`ALL_40_OK`).
- `cargo deny check`: PASS — `advisories ok, bans ok, licenses ok, sources ok`.
- `cargo xtask ci`: **PASS — "xtask ci: all gates passed"** (fmt check, workspace clippy `-D warnings`, workspace tests: dblocal-core 46 + gui lib 83 + integration tests + 0 doc-tests, all ok).
- Render tests: PASS inside `cargo llvm-cov -p dblocal-gui` (`render` suite: 5 passed in 43.29 s) and inside `xtask ci`. PNG sha256 list under `docs/design/renders/` **identical before and after** (`RENDERS_UNCHANGED`); no code change landed.
- Coverage (`cargo llvm-cov -p dblocal-gui --summary-only`): **TOTAL 81.45% regions / 80.83% lines**. Logic files: `format` 98.5%, `logs` 99.8%, `state` 98.9%, `theme` 96.2%, `service_bridge` 92.6%, `contrast` 89.6%, `vulkan` 89.9%. Layout/entry files below 80% (`bootstrap` 20.8%, `workspace_api` 30.4%, `status_bar` 45.7%, `app` 44.1%) are the same files `docs/architecture/gui.md` already lists as layout/entry-only.

## What I could not verify

- Windows build/runtime (no Windows host/toolchain).
- macOS build/runtime.
- X11 backend runtime and link (missing `libxkbcommon-x11-devel`).
- gpui-fast real-shell A/B (RSS/CPU/cold start/render pixel diff/lib 40×) on a host that can link the Fast backend.
- Non-AMD GPUs / other drivers (only AMD RADV Renoir tested).
- A clean 20-run process-to-first-frame wall-clock cold-start series (harness captured in-process frame delta; W0-C 361 ms p50 is the prior reference).

## Files I own / touched

- `docs/spikes/gpui-kit/README.md` (this report)
- `docs/spikes/gpui-kit/design-audit.md`
- `docs/spikes/gpui-kit/gpui-fast-notes.md`
- `docs/spikes/gpui-kit/evidence.md` (commands, link error, RSS, gate output)
- `docs/architecture/gui.md` (GPUI Kit section added)

No code, Cargo.toml, deny.toml, or render PNG changed.
