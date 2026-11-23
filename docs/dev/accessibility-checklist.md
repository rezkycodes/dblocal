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
| An engine card's focus handling removed (`apply_focus_ring` stops calling `track_focus`) | `keyboard_flow_tests::keyboard_only_first_session` | `FAILED` — "Enter on the focused engine card must select that engine" |
| The palette's start command enabled on a running instance (`if true` instead of `can_start`) | `format::tests::palette_disables_commands_with_a_reason_when_they_do_not_apply`, `palette_a_busy_instance_cannot_start_stop_or_copy`, `keyboard_flow_tests::palette_disables_inapplicable_commands_with_a_reason` | `FAILED` — "start on a running instance is disabled" |
| A token pair darkened below 4.5:1 (dark `muted.foreground` `#9da5b4 → #3a3d47`) | `contrast::tests::every_rendered_text_pair_meets_wcag_aa`, `derived_surfaces_keep_text_legible_too`, `tinted_status_badges_keep_their_text_readable` | `FAILED` — "muted.foreground on background: #3a3d47 on #15161d = 1.66" |
| A status variant without a label (`status_label(Running) → ""`) | `format::tests::every_status_has_a_glyph_and_a_nonempty_label` | `FAILED` — `Running` |
| Escape not returning focus (`restore_dialog_focus_after_close` made a no-op) | `keyboard_flow_tests::escape_returns_focus_to_the_opener` | `FAILED` — "Escape must return focus to the opener" |
| **F1**: palette confirm without the focus restore (`run_palette_action` drops the `restore_dialog_focus_after_close` call) | `keyboard_flow_tests::palette_starts_an_instance_with_the_keyboard` | `FAILED` — "ctrl-k must reopen the palette after a confirm (no dead keyboard)" |
| **F1**: `submit_create` success without the restore (drop `finish_create_overlay()`) | `keyboard_flow_tests::create_confirm_keeps_the_keyboard_alive` | `FAILED` — "ctrl-n must reopen the create dialog after a confirm (no dead keyboard)" |
| **F1**: no root focus on an empty frame (remove the `window.focused().is_none()` root focus in `render`) | `keyboard_flow_tests::a_fresh_window_opens_the_palette_without_a_click` | `FAILED` — "the workspace must take focus on the first frame (F1 startup)" |
| **F2**: Tab containment removed (`overlay()` no longer calls `focus_trap`) | `keyboard_flow_tests::tab_is_contained_in_the_create_dialog` | `FAILED` — "the create dialog must register a focus trap" |
| **F3**: the ring removed from a control kind (`apply_focus_ring` drops `focus_ring_style`) | `render::focus_ring::a_focused_engine_card_draws_the_theme_ring` | `FAILED` — "new ring pixels=0" (vs 2028 unmutated) |
| **F3**: `tab_stop(true) → false` on the rows | `keyboard_flow_tests::tab_reaches_every_engine_card_and_version_row` | `FAILED` — "Tab never reached a engine card ... visited 5 stops" |
| **F3**: the palette Start arm does nothing | `keyboard_flow_tests::full_first_session_creates_starts_and_copies` | `FAILED` — "the palette start command must start the stopped instance, got []" |
| **F4**: the dock instance-row focus removed | `keyboard_flow_tests::tab_and_enter_reaches_a_sidebar_row_and_opens_it` | `FAILED` — "Tab + Enter ... must reach a sidebar instance row and open it" |
| `submit_create` made a no-op | `keyboard_flow_tests::create_confirm_keeps_the_keyboard_alive`, `full_first_session_creates_starts_and_copies` | `FAILED` (the created-specs assertion) |
| **G1**: `StartSelected`/`StopSelected` handler body emptied | `keyboard_flow_tests::keyboard_fix2_tests::ctrl_shift_s_x_start_and_stop_the_selected_instance` | `FAILED` — "Ctrl+Shift+S must start exactly the selected stopped instance" |
| **G1**: `start_selected` without the `can_start` status check | `keyboard_flow_tests::keyboard_fix2_tests::ctrl_shift_s_x_are_noops_for_wrong_status_or_no_selection` | `FAILED` — "Ctrl+Shift+S on a running instance must be a no-op" |
| **G2**: version-row ring drawn only on the top edge | `render::focus_ring::a_focused_version_row_draws_a_full_outline` | `FAILED` — "bbox height 3 < 60 (top-edge-only mutation)" |
| **G3a**: `confirm_delete` success without the restore | `keyboard_flow_tests::keyboard_fix2_tests::delete_confirm_restores_focus_and_keeps_the_keyboard_alive` | `FAILED` — "ctrl-k must open after a delete confirm" |
| **G3b**: `restore_focus_if_quit_closed` made a no-op | `keyboard_flow_tests::keyboard_fix2_tests::quit_cancel_restores_focus_and_keeps_the_keyboard_alive` | `FAILED` — "cancel must return focus to the exact opener child" |
| **G3c**: `.focus_trap(...)` removed from the quit dialog | `keyboard_flow_tests::keyboard_fix2_tests::tab_is_contained_in_the_quit_dialog` | `FAILED` — "the quit dialog must register a focus trap (G3c)" |
| **G3d**: the group header's `apply_inset_focus_ring` removed (drops its `track_focus`) | `keyboard_flow_tests::keyboard_fix2_tests::tab_reaches_the_group_header_and_enter_toggles_it` | `FAILED` — "Tab + Enter must reach the group header and toggle it (G3d)" |
| **G3e**: the palette `CopyUri` arm does nothing | `keyboard_flow_tests::keyboard_fix2_tests::palette_copy_uri_puts_the_uri_on_the_clipboard` | `FAILED` — "must copy the URI, got \"\"" |

The focus-ring *pixels* are sampled by `tests/render/focus_ring.rs`: for an engine
card it counts pixels the theme `ring` colour in the focused frame but not at the
same point unfocused (2028 at scale 2); for a **version row** and a **group
header** it asserts the bounding box of the newly-added ring pixels spans the
row's full width **and** height (version row 1351×81 device px, header 509×59),
which a one-edge line (height ≈ 3) fails. The `focus_ring_pairs` contrast test
still proves `theme.ring` clears 3:1 on every surface it is drawn on.

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
| `Ctrl/Cmd+Shift+S` / `Ctrl/Cmd+Shift+X` | start / stop the **selected** instance, gated on its status (start only a stopped/failed instance, stop only a running one; otherwise a no-op) | root |
| `Ctrl/Cmd+K`, `Ctrl/Cmd+Shift+P` | open the command palette | root |
| `Ctrl/Cmd+,` | open Settings | root |
| `Up`/`Down`/`Enter` | move and confirm in the palette | palette |
| `Escape` (empty query) | close the palette and return focus | palette |

The focus indicator is **a ring/outline**, not a background tint. The default is
gpui-component's `focus_ring_style` (`apply_focus_ring`: a `theme.ring` border
plus the outside ring). Where the row sits in a **clipped** container — the
create dialog's version list has `overflow_hidden`, so an outside ring is cut off
to a single edge line — the row and the dock's engine-group header use
`apply_inset_focus_ring` instead: a 2px `theme.ring` border painted on **all four
sides inside** the row bounds, which cannot be clipped. Both are verified by
`tests/render/focus_ring.rs` (the version-row and group-header tests assert the
new ring's bounding box spans the row's full width **and** height, so a
one-edge line fails). `focus_ring_pairs` proves `theme.ring` clears 3:1 on every
surface it is drawn on.

## Automated here (Linux, headless, CI)

Every row below is a test that ships in `crates/gui`. A row is only "automated"
when the test **fails without the code it tests** (see the mutation checks above).

| Screen / state | What is checked | How | Result | Test |
|---|---|---|---|---|
| Shell, all overlays | no duplicate a11y node ids in a rendered frame | headless a11y tree | pass (27 cases) | `tests/a11y_tree.rs` |
| Sidebar tree + filter | engine-group headers and instance rows are tab stops with the ring; Enter/Space expands a group or opens an instance | headless key events | pass (F4/G3d) | `keyboard_flow_tests::tab_and_enter_reaches_a_sidebar_row_and_opens_it`, `keyboard_fix2_tests::tab_reaches_the_group_header_and_enter_toggles_it` |
| Instance tabs | each tab is a tab stop with the ring; Enter/Space selects it; close `Button` is its own stop | headless key events | pass (F4) | `keyboard_flow_tests::tab_and_enter_selects_an_instance_tab` |
| Overview (every card + failure banner) | status text + glyph; copy/reveal are `Button`s (tab stops, Enter/Space) | code audit + render | pass | `tests/render.rs` |
| Logs | follow/copy are `Button`s; list scrolls by wheel/keyboard | code audit | pass | `tests/render.rs` |
| Databases (tables, drop/secret/reset dialogs) | drop and reset dialogs take focus on open, return it on close, and trap Tab inside | unit + key events | pass (F1/F2) | `app/admin_fix_tests.rs`, `keyboard_flow_tests.rs` |
| Config (W4-H) | fields are `Input`s; Apply/Restore/Open are `Button`s; "perlu restart" is text | code audit + render | pass | `tests/render.rs` |
| Create dialog | focused on open; Tab/Shift+Tab **contained** inside the modal and wrap; engine cards + version rows are tab stops; **version-row and engine-card rings** drawn; activate on Enter/Space; Batal/Buat are tab stops | headless key events + render pixels | pass (F2/F3/G2) | `keyboard_flow_tests.rs`, `tests/render/focus_ring.rs` |
| Create dialog → first session | full session by key only, no pointer: choose engine+version → create (exact spec asserted) → copy URI (clipboard text + service call) → start (service call) | headless key events | pass (F3) | `keyboard_flow_tests::full_first_session_creates_starts_and_copies` |
| Delete dialog | takes focus on open, traps Tab inside, returns focus on confirm; type-the-name; Batal/Hapus are tab stops | unit + key events | pass (F1/F2/G3a) | `keyboard_fix2_tests::delete_confirm_restores_focus_and_keeps_the_keyboard_alive` |
| Quit dialog | three named `Button`s, primary default; takes focus on open, **traps Tab and wraps**, cancel returns focus to the exact opener child | headless key events | pass (G3b/G3c) | `keyboard_fix2_tests::tab_is_contained_in_the_quit_dialog`, `quit_cancel_restores_focus_and_keeps_the_keyboard_alive` |
| Settings dialog | Check/Pasang/Close `Button`s, login `Switch`; takes focus on open, traps Tab inside, close returns focus | key events + render | pass (F1/F2) | `keyboard_flow_tests.rs`, `app/settings_tests.rs` |
| Open in… menu | `DropdownButton` builds on `PopupMenuItem`s (menu items are keyboard-navigable) | code audit | pass | `app/open_in_tests.rs` |
| Command palette | Ctrl/Cmd+K opens + focuses query (stale query cleared); typing filters by name/engine/version/port (keywords); Up/Down/Enter run; disabled rows carry a text reason and Enter on one does nothing; **Copy URI** copies via keys; confirm/Enter returns focus so ctrl-k works again | headless key events + unit | pass (F1/F3/G3e) | `keyboard_flow_tests.rs`, `keyboard_fix2_tests::palette_copy_uri_puts_the_uri_on_the_clipboard`, `format/tests.rs` |
| Toasts | a `Label` in the status bar (text, not colour) | code audit | pass | — |
| Status bar | every counter is text + glyph; pending/tray notes are text | code audit | pass | `tests/render.rs` |
| Tray menu labels | `TrayMenuModel` rows carry shape+word status | unit | pass | `tray::tests` |
| Every screen, both themes | renders without layout crash; light frame lighter than dark | headless wgpu | pass (9 screens ×2) | `tests/render.rs::every_screen_renders_in_both_themes` |
| Theme following | system appearance selects light/dark; `DBLOCAL_THEME` forces one. Live following of a runtime OS change cannot be triggered from the gpui test platform (`simulate_appearance_change` is not exposed), so it is **manual** below | unit (selection) + manual (live) | partial | `theme::tests` |
| Contrast | every text/background pair incl. tinted badges, link, active tab, danger; both themes ≥ 4.5:1 | parse theme JSON | pass | `contrast.rs`, `tests/contrast_table.rs` |
| Focus ring visibility | `theme.ring` ≥ 3:1 on every surface; a focused control adds new ring pixels in a real frame | parse theme JSON + render pixels | pass (F3) | `contrast::tests`, `tests/render/focus_ring.rs` |
| Status never colour alone | every status → non-empty label + a distinct glyph family | unit | pass | `format/tests.rs` |

## Real-window run on this host (W4-G fix 1, Xvfb `:77`)

The desktop's Xwayland (`:0`) does not deliver synthetic XTEST input to a gpui
window, so the previous round's real-window keyboard check was inconclusive. A
**private Xvfb** display does deliver it; `env -u WAYLAND_DISPLAY` is mandatory
(without it gpui opens on the user's real Wayland desktop). Recipe:

```
Xvfb :77 -screen 0 1440x900x24 -nolisten tcp &
env -u WAYLAND_DISPLAY DISPLAY=:77 DBLOCAL_HOME=/tmp/dbl-x DBLOCAL_THEME=light setsid nohup <release dblocal> &
# no window manager: find the 1280x820 window and `xdotool windowfocus` it. F1
# makes a click unnecessary — the shell focuses its root on the first frame, so
# ctrl-k / ctrl-n work straight away (verified below with no click).
```

A note on reading the result: rule 3 forbids opening images. The ring is detected
by code — the bounding box of what changed between two consecutive frames
(`convert a.png b.png -compose difference -composite -threshold 0 -format %@ info:`)
— not by eye.

### Transcript (this host, Xvfb `:77`, release `dblocal`, light theme, fresh home)

Window is the 1280×820 one; focused with `xdotool windowfocus` and **never
clicked** before the first key. "diff" is the change bounding box between two
captures; a change confined to `x≥298` (right of the sidebar's 272 px) and below
the title bar proves the modal, not the page.

| Key | Observed |
|---|---|
| `ctrl+k` (no click yet) | frame changed 1271×758; the centre shows a white palette panel with rows — the palette **opened with no click** (F1 startup) |
| type `instance`, `Return` | frame changed 676×665 → the "Instance baru" confirm opened the create dialog |
| `Escape` then `ctrl+k` | frame changed 1271×758 → the palette **reopened right after a confirm** (F1: no dead keyboard) |
| `ctrl+n` (no click) | frame changed 676×476, create-panel ink present → create dialog opened by key |
| `Tab` ×1 in the create dialog | change `532×38+298+530` — one control row inside the dialog |
| `Tab` ×19 in the create dialog | change `512×370+298+198` — **entirely inside the dialog panel**; sidebar (`x<272`), title bar (`y<34`) and status bar unchanged → Tab did **not** leave the modal (F2) |
| `Escape`, then `Tab` on the main screen, 12× | changes cycle through the header (`+240+1`), the sidebar top (`+5+39`), the sidebar list (`+5+69`, `+5+721`), then the centre pane — focus reaches the sidebar controls in order and wraps (F4) |

### Fix-2 transcript (this host, Xvfb `:78`, release `dblocal`, light theme)

The PostgreSQL 18.7 tarball was pre-placed in `downloads/`; the instance was
created by key (`ctrl+n`, `Tab`×4 to "Buat", `Return`), giving `postgres-18`.

| Key | Observed |
|---|---|
| palette `postgres-18`, `Down`, `Return` ("Buka") | palette closed (change 1271×758); the instance tab opened and became the selected instance |
| `ctrl+shift+s` | `pgrep -x postgres`: **0 → 9** — the selected stopped instance started (G1; this key was dead before) |
| `ctrl+shift+x` | `pgrep -x postgres`: **9 → 0** — the running instance stopped (G1); frame changed 1220×766 |
| `ctrl+n`, `Tab` ×10..11 in the create dialog | change **676×83** over the version row — a band spanning the row's full width and height (a full inset outline), not a 2px top line (G2) |
| `Escape`×3, `Tab`×4 on the main screen | change **261×60+5+39** over the PostgreSQL group header — full header width and height (full inset outline), not a one-edge line (G2) |

Cleanup after the run: `pgrep -x dblocal`=0, `pgrep -x Xvfb`=0,
`pgrep -x postgres`=0, the temp home removed, no
`~/.config/systemd/user/dblocal.service`.

## Manual, per release (Windows, macOS; also a human pass on Linux)

These cannot be believed from a headless test or from this Linux host. Run them
on each release build for every OS in the matrix.

1. **Keyboard-only first session (real window).** With only the keyboard —
   `Ctrl/Cmd+N`, Tab/Shift+Tab, Enter/Space, `Ctrl/Cmd+K`, `Ctrl/Cmd+Shift+C`,
   `Ctrl/Cmd+Shift+S`/`X` — complete choose engine → create → start → copy URI
   and confirm the clipboard holds the URI. This host's Xvfb run exercises the
   same path (the desktop's Xwayland drops synthetic XTEST input, so use a
   private Xvfb); Windows/macOS confirm native key delivery.
2. **Focus ring per OS.** Tab through the shell on Windows and macOS and confirm
   the ring is drawn and visible (contrast ≥ 3:1 against both the control and the
   surface). The ring rendering path is `gpui-component`'s `focus_style`; the
   *appearance* on each native platform is unverified here.
3. **System theme following.** Flip the OS appearance (Windows Settings → Colour,
   macOS Appearance) while the app runs and confirm every screen repaints in the
   new theme **without a restart**. (The change-while-running subscription is
   exercised headless; the native signal is not.)
4. **Screen reader.** With Orca (Linux), NVDA (Windows) or VoiceOver (macOS),
   confirm the first session is announceable: sidebar rows read "name — status",
   dialog titles and buttons are named, the palette announces its rows.
5. **High-contrast / forced-colours modes.** Confirm the app is legible under the
   OS forced-colour mode (Windows High Contrast, macOS Increase Contrast).
6. **Non-Latin input.** Type an instance name with an IME (Japanese, Chinese,
   Arabic) into the create field and confirm composition and validation behave.
7. **Reduce motion.** With motion reduce on, confirm no essential feedback relies
   on animation.
