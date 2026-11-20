# W0-C — GPUI and tray spike

Branch: `w0-gpui-spike`. Read first: AGENTS.md, then PRD §6 `F-SPK-01`, `F-SPK-04`, `F-TRY`, §9 architecture rules 1-2,
decisions D-04, D-06, **D-23** and open questions OPEN-02, OPEN-03. The reference look is the **Zed IDE** (see
`docs/design/UI-DIRECTION.md`, which you must read first): docks, a tabbed centre pane, a status bar, a command palette, dark and light themes.
The reference for how to build it is https://github.com/zed-industries/zed/tree/main/crates/gpui/examples. **Do not copy Zed or DBDelve
source files**; reading how they use GPUI is fine.

## Goal

Decide, with a working prototype, whether GPUI plus a system tray can carry the product, and hand the GUI worker a
proven starting point. This answers OPEN-02 (tray) and OPEN-03 (GPUI maturity).

## You own

`spikes/gpui/**` and `docs/spikes/gpui/**`. Nothing else. The spike is a **standalone** cargo project with its own
`[workspace]` table so the root workspace ignores it.

## Rules

- `export CARGO_TARGET_DIR="$HOME/.cache/dblocal-target-gui"` for every cargo command (gpui compiles for a long time;
  a separate target dir keeps it from blocking other workers).
- Pins: `gpui = { package = "gpui-pre", version = "=0.3.12" }`, `gpui-component = "=0.10.0"`. Confirm they resolve together.
- No sudo. If a system library is missing, record the exact error and the package a user would need, then continue as far as you can.
- Tokio never runs on GPUI's executor (PRD §9 rule 2). Any async work runs on a separate thread and talks to the UI through a channel.

## Deliverables

### 1. Window prototype (`spikes/gpui/`)

A window using `gpui-component` that shows three fake instances with status dots (green running, grey stopped, red failed),
Start/Stop buttons that toggle state, a "Copy URI" button that writes to the clipboard, a download progress bar fed from a
background thread, and a dark teal theme in the spirit of DBDelve. It must run on this Wayland machine
(`WAYLAND_DISPLAY=wayland-0`; X11 on `DISPLAY=:0` is also available). Capture a screenshot if a tool exists (`grim`, `spectacle`,
`gnome-screenshot`); otherwise prove it runs with `timeout 20 cargo run` and exit status.

### 2. Missing Vulkan behaviour (`F-SPK-01` AC2)

Simulate no Vulkan (for example `VK_DRIVER_FILES=/nonexistent`, `VK_ICD_FILENAMES=/nonexistent`). Document exactly what GPUI does:
panic, error text, exit code, or a black window. Then implement a **pre-flight check** in the prototype that detects the failure
and exits with an actionable message (what is missing, which package to install, a docs link) and a non-zero exit code,
never a panic or a black window.

### 3. Tray (`F-SPK-04`) — the highest-uncertainty item

GPUI owns the event loop and does not treat a tray as a first-class citizen. Investigate and prototype:

- Linux: `tray-icon` needs GTK and libappindicator; GPUI does not use GTK. Evaluate `ksni` (StatusNotifierItem over D-Bus, no GTK)
  and any other pure-Rust option. State which desktop environment this machine runs, whether a StatusNotifier host is present,
  and what GNOME users need (AppIndicator extension).
- Windows and macOS: `tray-icon` 0.26. Reason about the event-loop requirements from its source and docs (macOS must be the main
  thread inside the NSApplication loop; Windows needs a message loop on the creating thread).
- Prototype a tray menu listing the fake instances with start/stop, whose actions reach the GPUI state through a channel and update
  the open window within 1 second. A "Quit" item and "close window keeps the app alive in the tray" (`F-TRY-02`).
- If no tray is available, the app must still work through the window (`F-TRY-01` AC3). Prove the fallback.
- Windows and macOS cannot be run here. Say so plainly. Use `cargo check --target <triple>` only if it works; otherwise mark as **untested**.

### 4. Measurements (`docs/spikes/gpui/measurements.md`)

Cold start to first frame (target p95 under 1.5 s), idle RSS (target under 150 MB), release binary size after `strip` (target
installer under 60 MB), first clean build time, target dir size, and the exact system libraries and Cargo features needed to
build and run on Linux.

### 5. Findings (`docs/spikes/gpui/README.md`)

A go / no-go with evidence for GPUI (OPEN-03) and for the tray (OPEN-02), the recommended tray crate per OS, the exact dependency
set and features, pitfalls you hit, and what the GUI worker should copy from your prototype. If the tray is a no-go on any OS, say
what retention fallback you recommend (PRD D-04 depends on the tray).

## Evidence required in your report

Build output, run output and exit statuses, the screenshot path if any, the Vulkan-failure transcript before and after your
pre-flight check, the measurements table, and the go / no-go verdicts first.
