# Accessibility checklist (`F-PLH-01`, `F-PLH-02`)

The PRD NFR "Aksesibilitas" (PRD §10, line ~945) asks for a **manual per-release
checklist** covering the first session by keyboard, contrast ≥ 4.5:1, and status
never by colour alone. This file is that checklist. It is split into what the
test suite already enforces here (Linux, headless) and what a human must still
verify on Windows and macOS.

Related: `docs/architecture/gui.md` (focus model, theme override, contrast test),
`docs/design/UI-DIRECTION.md` (the visual language and keyboard map).

## Mutation checks (red first, then restored, clean tree)

A test is not evidence until it fails without the code it tests. Each rule below
was reverted in turn, the named test observed red, then restored; the tree is
clean at `git status --short`.

| Mutation | Red test | Observed |
|---|---|---|
| The engine card's focus handling removed (`apply_focus_ring` stops calling `track_focus`/`focus_ring_style`) | `keyboard_flow_tests::keyboard_only_first_session` | `FAILED` — "Enter on the focused engine card must select that engine" |
| The palette's start command enabled on a running instance (`if true` instead of `can_start`) | `format::tests::palette_disables_commands_with_a_reason_when_they_do_not_apply`, `palette_a_busy_instance_cannot_start_stop_or_copy`, `keyboard_flow_tests::palette_disables_inapplicable_commands_with_a_reason` | `FAILED` — "start on a running instance is disabled" |
| A token pair darkened below 4.5:1 (dark `muted.foreground` `#9da5b4 → #3a3d47`) | `contrast::tests::every_rendered_text_pair_meets_wcag_aa`, `derived_surfaces_keep_text_legible_too`, `tinted_status_badges_keep_their_text_readable` | `FAILED` — "muted.foreground on background: #3a3d47 on #15161d = 1.66" |
| A status variant without a label (`status_label(Running) → ""`) | `format::tests::every_status_has_a_glyph_and_a_nonempty_label` | `FAILED` — `Running` |
| Escape not returning focus (`restore_dialog_focus_after_close` made a no-op) | `keyboard_flow_tests::escape_returns_focus_to_the_opener` | `FAILED` — "Escape must return focus to the opener" |

The focus-ring *pixels* themselves are not sampled headlessly: gpui-component
draws its ring inside the dependency, where `cfg!(test)` is false, so its
`focus-ring` debug selector records nothing from this crate's tests. The ring is
verified instead by (a) the `focus_ring_pairs` contrast test proving `theme.ring`
clears 3:1 on every surface it is drawn on, and (b) the `keyboard_only_first_session`
mutation above proving the control is focusable and draws the theme ring only when
`apply_focus_ring` runs. A pixel check of the ring is a manual step (below).

## What "good" means (acceptance, verbatim intent)

- **F-PLH-01 AC1** — with only the keyboard the whole first session (choose
  engine → create → start → copy URI) can be completed, **with the focus always
  visible**.
- **F-PLH-01 AC2** — the command palette (`Ctrl/Cmd+K`) finds an instance and
  runs start / stop / copy URI.
- **F-PLH-02 AC1** — the app follows the system light or dark theme and **all
  text meets 4.5:1**.
- **F-PLH-02 AC2** — instance status is never conveyed by colour alone (a text
  label or an accompanying icon always exists).

## Keyboard contract

| Keys | Action | Where |
|---|---|---|
| `Tab` / `Shift+Tab` | move focus in visual order, no trap | every screen and dialog |
| `Enter` / `Space` | activate the focused control | buttons, checkboxes, switches, list rows |
| `Escape` | close the top overlay and return focus where it came from | every dialog and the palette |
| `Ctrl/Cmd+N` | open the create dialog | root |
| `Ctrl/Cmd+Shift+C` | copy the selected instance URI | root |
| `Ctrl/Cmd+Enter` | start/stop the selected instance | root |
| `Ctrl/Cmd+Shift+S` / `Ctrl/Cmd+Shift+X` | start / stop the selected instance | root |
| `Ctrl/Cmd+K`, `Ctrl/Cmd+Shift+P` | open the command palette | root |
| `Ctrl/Cmd+,` | open Settings | root |
| `Up`/`Down`/`Enter` | move and confirm in the palette | palette |
| `Escape` (empty query) | close the palette and return focus | palette |

The focus indicator is **a ring/outline**, not a background tint: gpui-component
draws `theme.ring` at its edge (or a contrasting inside line on filled
controls). `apply_focus_ring` (the helpers documented in
`docs/architecture/gui.md`) calls `focus_ring_style` on a focused row, and the
`focus_ring_pairs` contrast test proves `theme.ring` clears 3:1 on every surface
it is drawn on.

## Automated here (Linux, headless, CI)

Every row below is a test that ships in `crates/gui`. A row is only "automated"
when the test **fails without the code it tests** (see the mutation checks above).

| Screen / state | What is checked | How | Result | Test |
|---|---|---|---|---|
| Shell, all overlays | no duplicate a11y node ids in a rendered frame | headless a11y tree | pass (27 cases) | `tests/a11y_tree.rs` |
| Sidebar tree + filter | filter input is a real `Input` tab stop; rows are mouse-only (reach an instance by **palette → Buka**) | code audit | rows not tab stops (by design; palette covers) | manual |
| Instance tabs | each tab's close `Button` is a tab stop; selecting a tab is mouse-only; palette "Buka" selects | code audit | selection via palette | `keyboard_flow_tests` |
| Overview (every card + failure banner) | status text + glyph; copy/reveal are `Button`s (tab stops, Enter/Space) | code audit + render | pass | `tests/render.rs` |
| Logs | follow/copy are `Button`s; list scrolls by wheel/keyboard | code audit | pass | `tests/render.rs` |
| Databases (tables, drop/secret/reset dialogs) | drop and reset dialogs take focus on open and return it (W4-B `dialog_focus`) | unit | pass | `app/admin_fix_tests.rs` |
| Config (W4-H) | fields are `Input`s; Apply/Restore/Open are `Button`s; "perlu restart" is text | code audit + render | pass | `tests/render.rs` |
| Create dialog | focus on open; engine cards + version rows are tab stops that activate on Enter/Space; Batal/Buat are tab stops | headless key events | pass | `keyboard_flow_tests.rs` |
| Create dialog → first session | open (Ctrl+N) → pick engine → pick version → create, by key only, no pointer; then start and copy URI are the root commands / palette on the opened instance | headless key events | pass | `keyboard_flow_tests.rs` |
| Delete dialog | takes focus on open; type-the-name; Batal/Hapus are tab stops | unit + code audit | pass | `app/instance_actions_tests.rs` |
| Quit dialog | three named `Button`s, primary default; Escape cancels | code audit + render | pass | `tests/render.rs` |
| Settings dialog | Check/Pasang/Close `Button`s, login `Switch` (tab stop, Enter/Space) | code audit + render | pass | `tests/render.rs`, `app/settings_tests.rs` |
| Open in… menu | `DropdownButton` builds on `PopupMenuItem`s (menu items are keyboard-navigable) | code audit | pass | `app/open_in_tests.rs` |
| Command palette | Ctrl/Cmd+K opens + focuses query; typing filters by name/engine/version/port; Up/Down/Enter run; disabled rows carry a text reason | headless key events + unit | pass | `keyboard_flow_tests.rs`, `format/tests.rs` |
| Toasts | a `Label` in the status bar (text, not colour) | code audit | pass | — |
| Status bar | every counter is text + glyph; pending/tray notes are text | code audit | pass | `tests/render.rs` |
| Tray menu labels | `TrayMenuModel` rows carry shape+word status | unit | pass | `tray::tests` |
| Every screen, both themes | renders without layout crash; light frame lighter than dark | headless wgpu | pass (9 screens ×2) | `tests/render.rs::every_screen_renders_in_both_themes` |
| Theme following | system appearance selects light/dark; `DBLOCAL_THEME` forces one | unit | pass | `theme::tests` |
| Contrast | every text/background pair incl. tinted badges, link, active tab, danger; both themes ≥ 4.5:1 | parse theme JSON | pass | `contrast.rs`, `tests/contrast_table.rs` |
| Focus ring visibility | `theme.ring` ≥ 3:1 on every surface | parse theme JSON | pass | `contrast::tests` |
| Status never colour alone | every status → non-empty label + a distinct glyph family | unit | pass | `format/tests.rs` |

## Manual, per release (Windows, macOS; also a human pass on Linux)

These cannot be believed from a headless test or from this Linux host. Run them
on each release build for every OS in the matrix.

1. **Focus ring per OS.** Tab through the shell on Windows and macOS and confirm
   the ring is drawn and visible (contrast ≥ 3:1 against both the control and the
   surface). The ring rendering path is `gpui-component`'s `focus_style`; the
   *appearance* on each native platform is unverified here.
2. **System theme following.** Flip the OS appearance (Windows Settings → Colour,
   macOS Appearance) while the app runs and confirm every screen repaints in the
   new theme **without a restart**. (The change-while-running subscription is
   exercised headless; the native signal is not.)
3. **Screen reader.** With Orca (Linux), NVDA (Windows) or VoiceOver (macOS),
   confirm the first session is announceable: sidebar rows read "name — status",
   dialog titles and buttons are named, the palette announces its rows.
4. **High-contrast / forced-colours modes.** Confirm the app is legible under the
   OS forced-colour mode (Windows High Contrast, macOS Increase Contrast).
5. **Non-Latin input.** Type an instance name with an IME (Japanese, Chinese,
   Arabic) into the create field and confirm composition and validation behave.
6. **Reduce motion.** With motion reduce on, confirm no essential feedback relies
   on animation.
