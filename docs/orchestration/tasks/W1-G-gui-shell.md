# W1-G — GUI shell on `crates/gui` with an in-memory service

Branch: `w1-gui-shell`. Read first: AGENTS.md, `docs/design/UI-DIRECTION.md` (decision D-23, Zed-style), `docs/architecture/CONTRACT.md`, **`docs/spikes/gpui/README.md`** (the W0-C findings you build on),
PRD §6 `F-INS-01`, `F-INS-03`, `F-CRD-02`, `F-CAT-01..02`, `F-LOG-01..02`, `F-RTM-01`, decisions D-18, D-23, and open item OPEN-08.
Never open an image with the Read tool (the Command Code protocol rejects image input). Check renders numerically, as W0-C did.

## Goal

The real GUI crate: a Zed-style workspace that talks only to `dyn DblService`, running today against an in-memory mock so the whole Phase 1 flow can be exercised and reviewed before the real service exists (W2-A).
The prototype in `spikes/gpui/` is **reference code**, not something to paste: split it into small modules (the spike's `app.rs` is 1107 lines; keep every file < 800 and aim for 200-400).

## You own

`crates/gui/**`, `crates/core/src/service/mock.rs`, `crates/core/src/strings.rs` (you are the only editor of `strings.rs` in this wave), `docs/architecture/gui.md`, `docs/dev/linux-build-deps.md`. Do not edit `crates/cli/**` (it calls `dblocal_gui::run()`).
Tray wiring, quit dialogs and the real service are Wave 2; do not build them, but keep a seam for them.

## Deliverables

1. **Mock service** (`service/mock.rs`, `mock` feature): in-memory `DblService` with a seedable clock and random source, simulated operations (create: provisioning, download with `DownloadProgress` and ETA, initializing, stopped; start/stop; delete with both `DeleteMode`s;
   catalog refresh that can report Fresh, Stale and Offline), logs, and scripted failures (port in use, download failed, start timeout) that surface a `UserFacingError` with a `DBL-NNNN` code. Events go out over the `broadcast` channel exactly as the real service will. Fully unit-tested, >= 80%.
2. **Shell** per UI-DIRECTION.md: client-side title bar; left Instances dock grouped by engine (status dot **plus** text label, name, version, port); centre pane with a tab per opened instance (Overview, Logs); status bar (running count, catalog freshness, downloads popover with progress and ETA, version);
   command palette `Ctrl/Cmd+K` using `gpui_component::command::Command` (the spike's hand-made overlay is a placeholder); toasts ("URI copied"); modals.
3. **Flows**: create instance (engine, version list from `CatalogView` with unavailable combinations shown disabled and explained, name validation through `validate_spec`, suggested port, password or no-password mode, real form fields on `InputState`);
   start/stop; copy connection URI (`UriFormat::Uri` and `DotEnv`, password masked until revealed); delete dialog with the two options and type-the-name confirmation (D-18); failure state showing the diagnosis text and code (F-LOG-02); empty state; stale and offline banners (F-CAT-01).
4. **Logs tab**: virtualised over a ring buffer (`uniform_list`/`VirtualList`), live follow, copy lines; never block the UI thread.
5. **All user-facing strings in Indonesian in `strings.rs`**, exactly as the PRD words them. No literal UI sentence elsewhere in `crates/gui`.
6. **Vulkan pre-flight** ported from the spike (exit code 78 with install instructions, no panic, no black window) with its tests.

## Fixes required from my review of W0-C (these are bugs in the spike, not options)

- **Contrast must be tested against the theme that is rendered.** The spike's `contrast.rs` checks hand-typed hex constants, not `THEME_SET_JSON`, so the test passed while the light theme still failed. Move the themes to JSON files under `crates/gui/assets/themes/` loaded with `include_str!`,
  and make the test parse **those same files** and check every `X.foreground` on `X.background` and `X`, plus `foreground`, `muted.foreground` and the status colours against `background` and `sidebar.background`, in both themes, failing below 4.5:1.
  I measured the current light theme: `primary.foreground #fafafa` on `primary.background #4078f2` = **3.88**, `base.blue #4078f2` on `#fafafa` = 3.88 and on `#f0f0f0` = 3.55, `info.*` and `sidebar.primary.*` the same, and `muted.foreground #696c77` on `muted.background #eaeaeb` = **4.35**. Fix the tokens, not the test. The dark theme passed all 32 pairs.
- **No polling.** The spike's 50 ms pump tick costs about 3.2% CPU at idle. The PRD requires idle CPU < 1% and RSS < 150 MB (OPEN-08: spike measured 176 MB). Bridge service events with an async channel that wakes the UI only when something arrives; no periodic timer while idle.
  Measure and report three numbers: an **empty GPUI window** (the floor), your app idle, and your app after the mock download finishes. Do not hack around the floor; if the floor alone is above 150 MB, say so with the evidence and I will take it to the user.
- **Fonts.** The spike claims the system font "resolves to Segoe UI on this host", which is a Windows font on a Fedora machine. Find out which family GPUI really resolves on Linux (log it), set an explicit sensible fallback stack, and keep the system font by default (W0-C's recommendation). Any bundled font must ship its licence.
- Document the Fedora and Debian build packages (including `libxkbcommon-x11-devel`, which blocked the X11 build in the spike) in `docs/dev/linux-build-deps.md`.

## Structure and tests

Keep rendering thin and logic in pure view-models that are unit-tested: tree grouping, status-to-label mapping, size, speed and ETA formatting, URI masking, palette filtering, form validation. Coverage >= 80% on every non-layout file; report layout files separately.
Add headless render tests (as in the spike's `tests/render_screenshot.rs`) for dark, light, palette, logs, create modal and a failure state, asserting on sampled pixels and distinct-colour counts. `tokio` runs on its own thread and never on the GPUI executor (AGENTS.md).

## Done when

`cargo xtask ci` green (including building `crates/gui`), the coverage table, the three idle numbers, the contrast test output from the real theme files, and a list of anything unverified (Windows, macOS and X11 are not runnable here; say so). Commit early and often.
