# Design-guide audit against GPUI Kit `design-guides` (normative: must / should / may)

Source read: https://gpui-kit.com/docs/design-guides (2026-11-21), plus `accessibility`, `assets`,
`fonts`, `getting-started`. Rules are quoted/paraphrased; status is **pass / deviation / n-a**
against the **current shell** (`crates/gui/src/**`). "Fix if import/one-liner" marks only what I
changed; everything else is listed with an effort estimate.

| # | Rule (Kit `design-guides` / `accessibility`) | Where it applies | Status | File | Fix / effort |
|---|---|---|---|---|---|
| 1 | Text exposed to accessibility: use `Label`, not a bare string child | Every visible string (title bar, sidebar groups, rows, status bar, overview, failure panel, welcome, dialogs, palette) | **deviation (broad)** | `app/title_bar.rs`, `app/views.rs`, `app/status_bar.rs`, `app/create_dialog.rs`, `app/delete_dialog.rs` | Bare `div().child(String)` and `.child(&str)` are used throughout. Converting every text node to `gpui_component::label::Label` is a broad mechanical change (~60-90 call sites) with render-pixel risk (Label uses theme metrics), so **not changed** here. Est: 0.5–1 day + render re-baseline. |
| 2 | Hover must not be the only path to a destructive/essential action | Overview actions (Start/Stop/Copy/Delete), sidebar rows | **pass** | `app/views.rs` | All commands are always-visible `Button`s; no hover-only action exists. |
| 3 | Secondary/region commands behind `DropdownMenu`; object commands in `ContextMenu`; keep the primary visible | Overview command row, sidebar rows | **deviation (design)** | `app/views.rs` | Overview renders 5 always-visible buttons (Start/Stop/Copy URI/Copy .env/Delete). Kit says the primary stays visible and secondary moves into a `DropdownMenu`. This is a **layout/behaviour change**, out of scope for import-only fixes. Est: 0.5 day + palette/action wiring review. |
| 4 | `Button` for commands; `Link` only for URLs | All action controls | **pass** | `app/views.rs`, `app/title_bar.rs`, `app/status_bar.rs` | Commands use `Button`; no Link-styled command. (URI text is display-only, not a link.) |
| 5 | No raw hex/rgb/hsla in application UI; resolve colours from `cx.theme()` | All views | **pass with one exception** | `app/palette.rs` | Palette backdrop uses `gpui::hsla(0.0,0.0,0.0,0.45)` and `app/create_dialog.rs` uses the same literal. It is an overlay scrim (no semantic token exists). Kit says define a product token if no role exists. **fix-later**; est: 0.5 h (add `overlay` token to both JSON themes + contrast table). |
| 6 | State must be visible: hover / focus / selected / disabled / loading / error / offline / empty | Shell-wide | **partial** | `app/views.rs`, `app/status_bar.rs` | Selected row (sidebar accent) ✅, disabled buttons ✅ (`.disabled`), error (failure panel) ✅, offline/stale banner ✅, empty (welcome + empty logs) ✅. **Hover state on sidebar rows and palette rows is not explicitly styled** (relies on component defaults where used; custom `div` rows have no `.hover`). Dev: hover styling on custom rows. Est: 2 h. |
| 7 | Explicit focus ownership + tab order | Root, palette, modals, logs | **partial** | `app.rs` (`track_focus`, key context), `app/palette.rs`, `app/create_dialog.rs`, `app/delete_dialog.rs` | Root has `track_focus` + key context and Escape closes overlays; palette uses `CommandState` keyboard nav. **No explicit `tab_index`/`tab_stop` ordering on the custom rows and dialogs**, and focus is not explicitly returned to the trigger after overlay dismissal. Dev. Est: 0.5–1 day (needs `focus_visible` audit + tests). |
| 8 | Minimum window size + resizable panes with a documented minimum | Window | **pass** | `app/bootstrap.rs` | `window_min_size: 900x600`; sidebar fixed 260, centre `flex_1 + min_w_0`. |
| 9 | Density: medium default, compact only for dense data | Rows/tabs/status bar | **pass** | `app/views.rs`, `app/status_bar.rs` | 26 px status bar, 24–28 px rows, `xsmall`/`compact` buttons in toolbars — within Kit's small/compact guidance for data-dense shell. |
| 10 | Tokens before values; prefer rem scale helpers over `px(...)` | All views | **deviation (broad)** | `app/views.rs`, `app/status_bar.rs`, `app/palette.rs`, `app/title_bar.rs` | Many literal `px(..)` for heights, paddings, widths (e.g. `h(px(26.))`, `w(px(260.))`, `rounded(px(6.))`). Kit says app layout "should not call px(...) directly" except physical/raster boundaries. The shell predates the rule. Not import-only. Est: 1–2 days + visual regression. |
| 11 | Radii from a finite theme scale; `radius_full` only for circles/pills | Rows (4px), panels (6px), palette (8px) | **deviation (minor)** | `app/views.rs`, `app/palette.rs` | Uses literal radii rather than theme tiers. Est: 0.5 h if theme exposes tiers; verify token names. |
| 12 | Status not communicated by colour alone | Sidebar rows, tabs, overview | **pass** | `app/views.rs` | Dot **plus** status text label (`format::status_label`) on rows and overview; tabs carry a dot whose meaning is repeated in the row text. |
| 13 | Badges only for short state/count, mostly neutral | "no auth" badge | **pass** | `app/views.rs` | Single semantic `warning` badge for the no-auth state; not a field of badges. |
| 14 | Icons supplement labels; icon-only buttons need an accessible name + tooltip | — | **n-a** | — | The shell uses labelled buttons; no icon-only control yet. |
| 15 | Uppercase only for short section labels, never combined with strong colour + bold | Sidebar headers, group labels | **pass** | `app/views.rs` | Uppercase used only on short section labels, muted colour, semibold (not strong color). |
| 16 | Ellipsis (…) on buttons that open a dialog; no ellipsis on immediate commands | "New instance" buttons | **deviation (copy)** | `app/title_bar.rs`, `app/views.rs` | "New instance" opens a dialog and per Kit should be "New instance…". Indonesian strings live in `crates/core/src/strings.rs` (not owned). Est: needs a core change via `ask` (1 h). |
| 17 | Dialogs form one compact decision: title names the decision, actions Cancel + result | Delete dialog, create dialog | **pass (delete) / partial (create)** | `app/delete_dialog.rs`, `app/create_dialog.rs` | Delete names the instance and the two D-18 actions, destructive result labelled. Create is a form; acceptable. |
| 18 | Avoid stacking overlays; Escape dismisses topmost, focus returns to trigger | Palette + modals | **partial** | `app.rs` (`close_modals` closes all), `app/palette.rs` | Escape closes overlays (root binding) but closes **all** at once and does not restore focus to the trigger. Dev (same item as #7). |

## Fixes applied in this task

None. Every deviation above needs either a broad mechanical change (Label migration, px→rem,
hover/focus system) or a behaviour/layout change (DropdownMenu regrouping), which the task
excludes ("Fix only deviations that need nothing but an import or a one-line constructor change").
No import-only or one-line-constructor deviation was found that is safe to change without a
render re-baseline.

## Effort summary (for the coordinator)

- Label migration (a11y text): **0.5–1 day** + render re-baseline.
- px→rem scale pass: **1–2 days** + visual regression.
- Hover + explicit focus/tab order: **0.5–1 day** + tests.
- Secondary commands → DropdownMenu: **0.5 day** + palette wiring.
- Overlay scrim token, radii tiers, "New instance…" copy: **≤2 h** each (copy needs a core string change via `ask`).
