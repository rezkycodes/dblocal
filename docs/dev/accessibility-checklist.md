# Accessibility checklist (`F-PLH-01`, `F-PLH-02`)

The PRD NFR "Aksesibilitas" (PRD §10, line ~945) asks for a **manual per-release
checklist** covering the first session by keyboard, contrast ≥ 4.5:1, and status
never by colour alone. This file is that checklist. It is split into what the
test suite already enforces here (Linux, headless) and what a human must still
verify on Windows and macOS.

Related: `docs/architecture/gui.md` (focus model, theme override, contrast test),
`docs/design/UI-DIRECTION.md` (the visual language and keyboard map).

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
controls), and a DELIBERATE test asserts the focused element renders it.

## Automated here (Linux, headless, CI)

Every row below is a test that ships in `crates/gui`. A row is only "automated"
when the test **fails without the code it tests** (see the mutation checks in the
worker report).

| Screen / state | What is checked | Test |
|---|---|---|
| Shell, all overlays | no duplicate a11y node ids in a rendered frame | `tests/a11y_tree.rs` |
| Palette | `Ctrl/Cmd+K` opens, typing filters, Up/Down/Enter run a command | `app/palette_*` (see report) |
| Palette | start/stop/copy entries appear for the matching instance and are disabled with a reason when not applicable | `app/palette_*` |
| Palette / dialogs | `Escape` closes the top overlay and returns focus to the opener | `app/palette_*`, `app/dialog_focus.rs` |
| Create dialog | one end-to-end key-only first session: open → pick engine/version → name → create → start → copy URI | `app/keyboard_flow_tests.rs` |
| First-session focus | the focused control renders a focus indicator the theme guarantees | `app/focus_indicator_tests.rs` |
| Theme/contrast | every text/background pair the screens use, both themes, ≥ 4.5:1 | `contrast.rs` + `tests/contrast_table.rs` |
| Theme following | the system appearance selects light or dark; `DBLOCAL_THEME` forces one | `theme::tests`, `tests/render.rs` |
| Status | every status variant maps to a non-empty label **and** a distinct glyph | `format/tests.rs` |
| Every screen, both themes | renders without layout crash, sampled against tokens | `tests/render.rs` |

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
