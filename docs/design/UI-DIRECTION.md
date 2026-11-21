# UI direction: DBLocal prototype (D-25)

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
| Left dock | `INSTANCES` with "+", filter, collapsible engine groups with counts, status glyph + label + name + version + `:port` (failed port in danger), `RUNTIMES` with unused marker, primary "Instance baru" | `crates/gui/src/app/dock.rs`, `crates/gui/src/format.rs` |
| Centre pane tabs | One tab per opened instance (status glyph + close). Header: engine badge, name, status chip, subtitle, Start/Stop, Restart, "Open in…" (placeholder), sub-tabs Overview/Logs/Databases/Config | `crates/gui/src/app/screens.rs` |
| Overview | Connection (masked URI, Show/Hide, Copy URI, Copy .env, loopback note, credential location), Detail, Process (PID placeholder, health, detached mode, credential ref, autostart through `set_autostart`), recent logs | `crates/gui/src/app/screens/overview.rs` |
| Logs | Per-instance `LogStore` tail via `uniform_list`, follow toggle, copy, diagnosis banner for typed port-in-use with "Pindah ke port berikutnya & start" = `suggest_port` + `change_port` + `start_instance` | `crates/gui/src/app/screens/logs.rs`, `crates/gui/src/logs.rs` |
| Databases / Config | Honest placeholders in the product build: "belum tersedia di versi ini" + disabled controls, no sample rows. Demo build renders the prototype layout for review only | `crates/gui/src/app/screens/databases.rs` |
| Status bar | Counts (running/stopped/failed), data dir + total size, credential backend, `os arch`, tray note, downloads popover, toast, version | `crates/gui/src/app/status_bar.rs` |
| Modals | New instance (engine cards, version list with Terpasang/Unduh badges, validation through `validate_spec`, no-password warning, download footer only when needed), Delete (Archive default / Purge, type-the-name) | `crates/gui/src/app/create_dialog.rs`, `crates/gui/src/app/delete_dialog.rs` |
| Command palette | `Ctrl/Cmd+K` / `Ctrl/Cmd+Shift+P`, kept from W1-G | `crates/gui/src/app/palette.rs` |

## Theme

- Dark: `DBLocal Dark`, base `#15161d`, source `crates/gui/assets/themes/dblocal-dark.json` (from the prototype `themes/dblocal.json`). This is the default dark theme.
- Light: `DBLocal Light`, same visual language, source `crates/gui/assets/themes/dblocal-light.json`. The app follows the system appearance (F-PLH-02).
- No colour literals in views. Every paint comes from `gpui_kit::component::Theme` or `crate::theme::tokens`, which derive from the active theme.
- Contrast is a hard rule: every text/control pair in both themes must be >= 4.5:1 (WCAG AA, F-PLH-02). `crates/gui/src/contrast.rs` and `crates/gui/tests/contrast_table.rs` read the real theme JSON and fail below 4.5.

### Contrast changes from the prototype / generated light theme

The prototype dark tokens were kept as-is (they pass). The light theme was authored in the same language and adjusted where needed for AA. The current measured table is produced by `cargo test -p dblocal-gui --test contrast_table -- --nocapture`.

| Theme | Pair | Before | After | Ratio | Reason |
|---|---|---:|---:|---:|---|
| DBLocal Light | muted.foreground on muted.background | `#6b7280` on `#eaebf0` | `#545866` on `#eaebf0` | 5.95 | Prototype-like muted grey failed AA on the elevated surface |
| DBLocal Light | muted.foreground on sidebar.accent.background | `#6b7280` on `#dadbe4` | `#545866` on `#dadbe4` | 5.14 | Selection rows need readable secondary text |
| DBLocal Light | primary/info/link on surfaces | `#4078f2` family | `#1a609f` family | >= 4.73 on derived surfaces, 6.16 on base | W1-G showed #4078f2 was 3.88 on white; kept hue, darkened for AA |
| DBLocal Light | success/danger/warning/info semantic tokens | prototype-light approximations | `#256a2b/#a03129/#735300/#085e76` | >= 4.80 on derived surfaces | Semantic colours must be readable as text, not only as fills |

No hue was changed; saturation/value were adjusted minimally until every checked pair passed 4.5:1. Disabled controls are exempt. Status is never colour alone: glyph shape plus text label (F-PLH-02 AC2).

## Keyboard and accessibility

- `Ctrl/Cmd+N` new instance, `Ctrl/Cmd+Shift+C` copy URI, `Ctrl/Cmd+Enter` start/stop, `Ctrl/Cmd+K`/`Ctrl/Cmd+Shift+P` command palette, `Escape` closes the top overlay.
- Text through `Label` (gpui-component) so it is exposed to accessibility. No bare string children for visible UI text.
- Focus order follows visual/task order; focus is visible; overlay close restores focus via gpui-component dialog/palette behaviour.
- Status is never colour alone: running filled circle, stopped hollow circle, failed triangle, busy half circle, always accompanied by the status word.
- No hover-only destructive/essential action. Primary actions are visible buttons; secondary/overflow actions use menus.

## Honest placeholders (no backend in the frozen service)

| Surface | Phase | Product build | Demo build |
|---|---|---|---|
| Databases tab (`F-DBU`) | 2 | Prototype card structure (Database, "User & hak akses", "Password superuser", boundary footer with disabled "Open in…"); every control disabled; empty-state copy "belum tersedia di versi ini (Fase 2)"; no sample rows | Prototype layout with sample rows, review only |
| Config tab (`F-CFG`) | 3 | Prototype card structure (restart-required banner, "Setting penting" with column header, footer actions); every control disabled; empty-state copy "belum tersedia di versi ini (Fase 3)"; no sample rows | Prototype layout with sample config rows, review only |
| "Open in…" (`F-OPN`) | 2 | Menu item disabled with placeholder reason | Menu entries visible but disabled |

## Rules

- No `DblService` call on the GPUI thread is introduced by this task. Existing synchronous calls remain as W1-G left them; W2-B moves them off-thread.
- No idle timer (OPEN-08). The event pump parks on a flume channel.
- No `unwrap/expect/panic/todo/unimplemented` outside tests.
- Files < 800 lines, functions < 50 lines except pure layout.
- All user-facing strings live in `crates/core/src/strings.rs`. Indonesian strings are exactly as the PRD/prototype write them.
