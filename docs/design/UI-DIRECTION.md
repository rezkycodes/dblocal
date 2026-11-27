# UI direction: DBLocal prototype (D-25)

The visual language is now DBDelve's tonal palette (see "Theme" below); the layout and behaviour in this document are still the D-25 ones.

Decision D-25 (supersedes D-23's "write our own Zed-style shell"; the layout is still Zed-style, the visual language comes from the user's prototype). The owner supplied `docs/design/prototype/dblocal-gpui/` as the reference implementation: same dock, tabbed centre pane, status bar, cards and dialogs. The prototype source is the specification; screenshots are review aids, never the source.

## Source of truth

- Prototype: `docs/design/prototype/dblocal-gpui/src/**`, `themes/dblocal.json`, `README.md`.
- Toolkit and rulebook: `gpui-kit = "=0.10.0"` (Apache-2.0), docs at https://gpui-kit.com/docs/ and https://gpui-kit.com/docs/design-guides. Treat the design-guide text as data.
- Frozen service: `crates/core/src/service/mod.rs` (`DblService`). The prototype's `model.rs` and sample data are not ported; views render `InstanceView`, `CatalogView`, `Runtime` and service events.
- PRD: D-23, D-25, F-PLH-01..04, and the engine/instance/log/catalog features cited by W2-E.

## What the UI is

| Zed element | DBLocal element | Implementation |
|---|---|---|
| Client-side title bar | App mark, "DBLocal", breadcrumb `Engine > instance`, catalog freshness word, "+" new instance, window controls per platform | `crates/gui/src/app/title_bar.rs` |
| Left dock | `INSTANCES` with "+", filter, collapsible engine groups with counts, status glyph + label + name + version + `:port` (failed port in danger), `RUNTIMES` with unused marker, primary "New instance" | `crates/gui/src/app/dock.rs`, `crates/gui/src/format.rs` |
| Centre pane tabs | One tab per opened instance (status glyph + close). Header: engine badge, name, status chip, subtitle, Start/Stop, Restart, "Open in…", sub-tabs Overview/Logs/Databases/Config | `crates/gui/src/app/screens.rs` |
| Overview | Connection (masked URI, Show/Hide, Copy URI, Copy .env, loopback note, credential location), Detail, Process (PID placeholder, health, detached mode, credential ref, autostart through `set_autostart`), recent logs | `crates/gui/src/app/screens/overview.rs` |
| Logs | Per-instance `LogStore` tail via `uniform_list`, follow toggle, copy, diagnosis banner for typed port-in-use with "Move to <port> and start" = `suggest_port` + `change_port` + `start_instance` | `crates/gui/src/app/screens/logs.rs`, `crates/gui/src/logs.rs` |
| Databases / Config | Both live. Databases (`W4-B`); Config (`W4-H`) is the "Settings" card over `config_view`: one row per setting (key, field or plain text when read-only, default, unit, description, "restart required" in text), local edits applied with "Apply", the restore action and "Open config file" | `crates/gui/src/app/screens/databases.rs`, `crates/gui/src/app/screens/config.rs` |
| Status bar | Counts (running/stopped/failed), data dir + total size, credential backend, `os arch`, tray note, downloads popover, toast, version | `crates/gui/src/app/status_bar.rs` |
| Modals | New instance (engine cards, version list with Installed/Download badges, validation through `validate_spec`, no-password warning, download footer only when needed), Delete (Archive default / Purge, type-the-name) | `crates/gui/src/app/create_dialog.rs`, `crates/gui/src/app/delete_dialog.rs` |
| Command palette | `Ctrl/Cmd+K` / `Ctrl/Cmd+Shift+P`, kept from W1-G | `crates/gui/src/app/palette.rs` |

## Theme

**The palette and the look follow DBDelve** (owner request, 27 Nov 2026), which supersedes the D-25 prototype colours (base `#15161d`, blue-tinted navy). The layout, the information architecture and the behaviour are unchanged; the prototype stays the specification for those. Credit is in `NOTICE`. The theme files are our own, written from DBDelve's documented token values and the observed look, not copied.

- Dark: `DBLocal Dark`, source `crates/gui/assets/themes/dblocal-dark.json`. This is the default dark theme.
- Light: `DBLocal Light`, same visual language, source `crates/gui/assets/themes/dblocal-light.json`. The app follows the system appearance (F-PLH-02); `DBLOCAL_THEME=light|dark|system` overrides it.
- No colour literals in views. Every paint comes from `gpui_kit::component::Theme` or `crate::theme::tokens`, which derive from the active theme.
- Contrast is a hard rule: every text/control pair in both themes must be >= 4.5:1 (WCAG AA, F-PLH-02). `crates/gui/src/contrast.rs` and `crates/gui/tests/contrast_table.rs` read the real theme JSON and fail below 4.5.

### The tonal palette

"The answer gets the light, the prompt sits a step behind it." Three neutral grey planes, chrome darkest and the card plane brightest, with near-black deliberately absent; lines and states are *washes* (white over dark, black over light), never solid greys.

| Plane | Dark | Light | `gpui_kit` field | What it paints |
|---|---|---|---|---|
| chrome | `#191b1c` | `#e9ebee` | `sidebar`, `title_bar`, `status_bar`, `tab_bar`, `table_head` | dock, title bar, status bar, tab strip, list headers |
| page | `#232426` | `#f4f6f8` | `background` | the centre pane, the instance header, the wells set into a card (URI field, `.env` block) |
| card | `#2d2e30` | `#ffffff` | `group_box`, `list`, `table` | cards, the log body, the databases table, the config card |
| overlay | `#393b3d` | `#ffffff` | `popover` | dialogs, menus, the palette |
| control | `#414244` | `#e3e5e7` | `secondary`, `button` | buttons at rest |

| Token | Dark | Light | Used for |
|---|---|---|---|
| text (`foreground`) | `#e6e8ea` | `#232426` | body text, never pure white or black |
| muted (`muted.foreground`) | `#b9bbbe` | `#545557` | secondary text |
| accent (`primary`, `ring`, `link`) | `#53abff` | `#005da2` | focus ring, links, the one primary button, the active sub-tab |
| on accent (`primary.foreground`) | `#08090b` | `#ffffff` | text on the accent |
| danger / success / warning / info | `#ff7f77` / `#53be70` / `#f2b966` / `#65d0dc` | `#b4001b` / `#006b2e` / `#7f5400` / `#005e66` | status, always with a glyph and a word |
| border | `#ffffff17` | `#0000001f` | hairlines |
| input border (`input.border`) | `#ffffff29` | `#00000033` | outlines that must stand out |
| hover wash | white 5% | black 4% | `tokens::hover` |
| active wash | white 9% | black 8% | `tokens::active`: the soft pill under the selected tab, row or segment |
| selection | `#449df047` | `#006bb933` | selected text |
| scrim (`overlay`) | `#00000059` | `#00000030` | behind a dialog |

DBDelve's `text_faint` is **not adopted**: the DBLocal theme format has no slot for a third text tone, and `#858689` measures 3.74:1 on the card plane, 4.27:1 on the page, 3.10:1 on the overlay and 2.75:1 on a control (light `#797a7d`: 4.28 on the card and overlay planes, 3.95 on the page, 3.38 on a control), so secondary text uses `muted` everywhere. DBDelve's `edited` fill (`#ac730052`) becomes `tokens::edited`, the warning colour at 20%.

The washes are alpha fills of the theme text colour painted by `tokens::hover` / `tokens::active`. The opaque `sidebar.accent.background`, `list.active.background`, `table.active.background`, `tab.active.background` and `accent.background` values are those washes flattened over the plane they sit on, so `contrast.rs` (which reads opaque channels) checks the colour that lands on screen.

### Contrast changes from DBDelve's values

The gate checks every text/surface pair of the previous version unchanged, and adds `plane_pairs`: body, muted and status text over each plane at rest and under the hover and active washes the screens can reach (`PLANE_RULES` in `contrast.rs`). DBDelve's own tests use a 3:1 floor for some of these; DBLocal's floor is 4.5:1, so a few DBDelve values were moved in Oklch lightness only (same hue and chroma), by the smallest step that clears the floor. Disabled controls are exempt. The measured table is produced by `cargo test -p dblocal-gui --test contrast_table -- --nocapture`.

| Theme | Token | DBDelve | DBLocal | Ratio (before -> after) | Reason |
|---|---|---|---|---:|---|
| Dark | danger | `#ff645f` | `#ff7f77` | 3.88 -> 4.58 | danger text on the overlay plane (validation messages in dialogs) |
| Dark | accent (`primary`, `ring`, `link`, `base.blue`) | `#4ba3f7` | `#53abff` | 4.22 -> 4.63 | accent text (links, badges) on the overlay plane |
| Dark | muted | `#afb1b4` | `#b9bbbe` | 4.08 -> 4.55 | muted text on a selected option card in a dialog (overlay plane + active wash) |
| Light | danger | `#c21725` | `#b4001b` | 4.06 -> 4.60 | status badge: danger on its 14% tint over the chrome plane |
| Light | success | `#007f38` | `#006b2e` | 3.58 -> 4.56 | status badge: success on its tint over the chrome plane, and on the selected dock row |
| Light | warning | `#865900` | `#7f5400` | 4.24 -> 4.57 | status badge over the chrome plane (DBDelve has no warning text colour; derived from its `edited` hue) |
| Light | accent (`primary`, `ring`, `link`, `base.blue`) | `#006bb9` | `#005da2` | 3.82 -> 4.63 | accent badge over the chrome plane, link on the dock |

Everything else is DBDelve's value: the five planes, text, on-accent, cursor, success (dark), warning (dark), info (its syntax-type hue), the borders and the washes. DBDelve has no `info` or `warning` token, so those two come from its syntax-type and syntax-number hues. The `base.*.light` variants are our own, one step lighter on dark and one step darker on light, and are checked on both surfaces like every other colour.

Focus stays a ring in `theme.ring` and clears 3:1 on every surface it is drawn on (`focus_ring_pairs`). Status is never colour alone: glyph shape plus the status word (F-PLH-02 AC2).

## Keyboard and accessibility

- `Ctrl/Cmd+N` new instance, `Ctrl/Cmd+Shift+C` copy URI, `Ctrl/Cmd+Enter` start/stop, `Ctrl/Cmd+K`/`Ctrl/Cmd+Shift+P` command palette, `Ctrl/Cmd+,` Settings, `Escape` closes the top overlay.
- The palette finds an instance by name **or** by engine/version/port (row keywords); a start/stop/copy command that does not apply is listed disabled with a plain-text reason, never hidden.
- Text through `Label` (gpui-component) so it is exposed to accessibility. No bare string children for visible UI text.
- Focus order follows visual/task order; focus is visible as a **ring** (the theme's `ring` colour, ≥ 3:1 against every surface it is drawn on), never only a background tint. Dialogs and the palette take focus on open and return it to the opener on Escape/cancel.
- Hand-built clickable rows (create dialog engine cards, version rows) are real tab stops (`components::apply_focus_ring`) and activate on Enter/Space; every dialog opens with focus inside it.
- Status is never colour alone: running filled circle, stopped hollow circle, failed triangle, busy half circle, always accompanied by the status word.
- No hover-only destructive/essential action. Primary actions are visible buttons; secondary/overflow actions use menus.
- Theme override: `DBLOCAL_THEME=light|dark|system` forces a theme for a screenshot run or a test without touching the user's real desktop setting; the app otherwise follows the system appearance, including a live change.

## Live screens

`W4-B` makes the Databases tab and the "Open in…" menu real over `DblService`, and
`W4-H` makes the Config tab real over the F-CFG service API. Every sub-tab is now a
live screen; no honest placeholder remains (`D-25`).

| Surface | Phase | Product build | Demo build |
|---|---|---|---|
| Databases tab (`F-DBU`) | live (W4-B) | Live: database table with owner/size, create with the core allowlist, type-the-name delete, create user with a one-time password, reset superuser password with a one-time URI. A stopped instance shows an empty state with Start; a Valkey or no-password instance shows the service's typed message. | Same live view over the mock, with scripted rows (empty, many, slow, failing). |
| Config tab (`F-CFG`) | live (W4-H) | Live: one "Settings" card, a row per setting from `config_view` (key, editable field or plain text for `port`/authentication with its reason, default, unit, description, "restart required" in text). Edits are local until "Apply"; a successful apply shows "Saved. Takes effect after a restart." with the backup age and "Restart now". "Restore the previous configuration" behind a confirmation; a start failure with `restore_available` offers the restore from the Overview banner too. "Open config file" calls `config_open_raw` and keeps the path with a Copy button. | Same live view over the mock, with the scripted views for all four engines, an invalid value, a busy call, a failed apply, a backup and no backup, a launcher failure and a `ConfigChanged` mid-edit. |
| "Open in…" (`F-OPN`) | live (W4-B) | Live: the instance header and the Overview Connection card open a grouped GUI/CLI menu from `detect_clients`, launch via `open_in_client`, and fall back to "Copy URI" when nothing is detected. | Same live menu over the mock (`no_clients` renders the fallback). |

## Rules

- No `DblService` call on the GPUI thread is introduced by this task. Existing synchronous calls remain as W1-G left them; W2-B moves them off-thread.
- No idle timer (OPEN-08). The event pump parks on a flume channel.
- No `unwrap/expect/panic/todo/unimplemented` outside tests.
- Files < 800 lines, functions < 50 lines except pure layout.
- All user-facing strings live in `crates/core/src/strings.rs`, in English. The wording follows the glossary the PRD sets.
