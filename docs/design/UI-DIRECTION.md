# UI direction: Zed-style

Decision D-23 (supersedes the DBDelve reference in D-06). The owner wants DBLocal to look and feel like the **Zed IDE**: dense,
dark-first, flat, keyboard-driven, built from GPUI primitives. Reference for *how to build it*:
https://github.com/zed-industries/zed/tree/main/crates/gpui/examples

## What "Zed-style" means here

Zed is a workspace of **docks and a tabbed centre pane**, not a window of pages. Map that onto DBLocal:

| Zed element | DBLocal element |
|---|---|
| Client-side title bar with window controls | Custom title bar: app name, "New instance" button, window controls (Linux/Windows draw their own; macOS uses the native traffic lights) |
| Left dock, project panel (tree) | **Instances panel**: tree grouped by engine (PostgreSQL, MySQL, MariaDB, Valkey), then instances. Each row: status dot + status text, name, version, port. Footer: "New instance" |
| Centre pane with a tab bar | One tab per opened instance (status dot in the tab). Tabs inside: **Overview**, **Logs**, **Databases** (Phase 2), **Config** (Phase 3). Empty state: a welcome tab with the "create your first instance" call to action |
| Bottom status bar | Running-instance count, catalog freshness (`Fresh` / `Stale` / `Offline` banner, F-CAT-01), active downloads (click for a popover with progress and ETA), tray availability, app version and update badge |
| Command palette | `Ctrl/Cmd+K` (alias `Ctrl/Cmd+Shift+P`): find an instance, start, stop, copy URI, new instance (F-PLH-01) |
| Toasts | Bottom-right: "URI copied", failures with the stable `DBL-xxxx` code |
| Modals | Centred with backdrop: New instance, Delete (two actions, type-the-name), Port in use (one-click "Move to <port>") |
| Tooltips with key hints | Every icon button shows its shortcut |

Density: 24-28 px rows, 1 px hairline borders, small radii, no gradients or shadows except on floating surfaces (popover, modal, toast).
Typography: a proportional UI font and a monospace font for logs, URIs and ports. IBM Plex Sans (SIL OFL 1.1) and the system monospace are the
default proposal; W0-C measures the binary-size cost of bundling a font and recommends. Any bundled font must ship its licence file.

## Palette

Start from the Atom **One Dark** and **One Light** palettes (MIT). Do not copy Zed's theme JSON or any Zed source file.
Dark base: editor `#282c34`, panels `#21252b`, selection `#3e4451`, text `#abb2bf`, blue `#61afef`, green `#98c379`, yellow `#e5c07b`, red `#e06c75`.
A light theme is required: the app follows the system theme (F-PLH-02).

### Contrast is a hard rule, and Zed's own greys fail it

PRD F-PLH-02 requires text contrast of at least **4.5:1**. Measured against editor `#282c34`:

| Token | Ratio | Result |
|---|---|---|
| body `#abb2bf` | 6.57 | pass |
| comment/muted `#5c6370` | 2.32 | **fail** |
| gutter `#4b5263` | 1.79 | **fail** |
| `#7f848e` | 3.73 | **fail** |
| `#9da5b4` | 5.65 | pass (use this for secondary text) |
| red `#e06c75` | 4.38 | **fail** on the editor background (4.82 on the panel) |
| blue `#61afef`, green `#98c379`, yellow `#e5c07b` | 5.92 / 6.94 / 8.10 | pass |

Rule: keep the Zed look, but lighten any token that fails. Add a unit test that computes the WCAG ratio for every text-on-background pair
in both themes and fails the build below 4.5. Disabled controls are exempt. Status is never colour alone: a dot **plus** a text label or icon
(F-PLH-02 AC2).

## Building blocks

- `gpui-component` 0.10.0 is the component library. It ships a JSON theme system (21 presets, none of them One Dark) and a dock/tab/sidebar
  toolkit. W0-C reports which of its widgets (`Sidebar`, `Tabs`, dock area, `Table`, `List`, `Input`, `Popover`, notifications) cover the table above,
  and which pieces we must build ourselves on raw GPUI.
- Use the Zed GPUI examples as an **API reference**: `window_shadow` and `window_movable` (client-side title bar), `on_window_close_quit`
  (close window keeps the app alive, F-TRY-02), `tree` and `uniform_list` (instances panel, virtualised logs), `data_table`, `input` (forms),
  `popover`, `set_menus` (macOS menu bar), `system_notifications`, `a11y`, `focus_visible` and `tab_stop` (keyboard navigation, F-PLH-01).
- **Licensing.** The `gpui` crate and its examples are Apache-2.0, compatible with our GPL-3.0-only. Zed's own `ui` and `theme` crates are
  GPL-3.0-or-later and live inside the Zed workspace; they are not on crates.io (the crates.io names `ui` and `theme` belong to unrelated
  projects), so they cannot be a dependency. Write our own code. If you adapt a snippet from an example, keep it small and put an attribution
  comment with the source path and the Apache-2.0 notice at the top of that file.

## Out of scope

Pixel-perfect cloning, an editor widget, themes beyond dark and light, and any Zed branding or logo.
