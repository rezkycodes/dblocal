# W4-G fix 2 — Ctrl+Shift+S/X are dead, the version-row ring is a thin line, five mutants still green

Same branch `rezkycodes/w4-gui-keyboard-contrast`, same worktree. Rules `docs/orchestration/tasks/_COMMON-RULES.md` incl. 3 (never open an image), 11-15. FIRST `git merge main` (this spec arrives on main). Do not touch `dblocal-gpui-kit-source/`. Small round: three items, no refactors.

## What the reviewer verified on `e61a90e` and accepts (do not redo)

Gates on a private copy: fmt, deny, clippy workspace and `--features demo`, GUI lib 419 x3, a11y_tree 27, render 17, contrast_table 1, core lib 1483 x3, release build. Real window on a private Xvfb (`:78`), key-only, read by me:
- No click: `ctrl+k` opens the palette, `ctrl+n` opens the create dialog with the caret in Nama (F1 startup is fixed).
- Create dialog: Tab cycles Nama -> Port -> checkbox -> Batal -> Create -> PostgreSQL -> Valkey -> MySQL -> MariaDB -> 18.7.0 -> 17.12.0 -> 16.16.0 -> Nama, never leaves the dialog (F2 fixed); Space on a version row selects it (name becomes `postgres-16`); Enter on Create creates the instance; `ctrl+k` opens right after the confirm (F1 fixed).
- Palette: `mulai` + Enter starts postgres (9 processes), `ctrl+k` reopens; `salin` + Enter copies the URI (clipboard text correct), `ctrl+k` reopens. Tab inside the palette keeps focus in the query field and nothing behind it changes.
- Settings (`ctrl+,`): Tab cycles Tutup -> Periksa sekarang -> switch -> Tutup; Escape then `ctrl+k` works.
- Main screen Tab walk: gear -> header "+" -> sidebar "+" -> Filter -> PostgreSQL group header -> postgres-18 row -> "Instance baru" -> status item -> wraps; Enter on the row opens the instance; then instance tab -> its close button -> Hentikan -> Restart -> Open in... -> chevron -> inner tab bar (one stop; Right arrow moves to Logs, Enter selects it) -> Tampilkan sandi -> Salin URI -> Salin .env -> Open in... -> chevron -> checkbox. Every stop shows a ring.
- Mutations on the lib, 22 of 28 red: palette confirm / create confirm / settings / secret / drop / config-restore / palette close without the focus restore, root fallback removed, root focus on first frame removed, focus trap removed from `overlay()` and from the palette, `tab_stop(false)`, ring removed from the sidebar row (red via reachability) and from `apply_focus_ring` (red in `render focus_ring`), palette query focus, stale query, keywords, disabled row, Start enabled on a running instance, Start arm, create-dialog focus, status label.

## G1 (MEDIUM) — `Ctrl/Cmd+Shift+S` and `Ctrl/Cmd+Shift+X` are bound and do nothing

`actions.rs` declares `StartSelected` / `StopSelected` and binds `ctrl-shift-s` / `ctrl-shift-x`, but **no `on_action` handler exists anywhere** (since commit 6794238). Real window: with an instance selected and running, `ctrl+shift+c` copied the URI, `ctrl+enter` stopped and started it, `ctrl+shift+x` did nothing. Your `docs/dev/accessibility-checklist.md` keyboard contract lists both as working: that row is wrong today.
Do: handlers in the root's `on_action` list: `StartSelected` starts the selected instance only when the palette's own start predicate allows it (stopped/failed), `StopSelected` stops it only when running; no selection or wrong status = no call, no panic, no dialog. Same path as the palette arms (one function per verb, not a copy). Key-driven tests (`press(cx, "ctrl-shift-s")`) asserting the SPECIFIC recorded call and its instance id in `QuietService`, plus the no-selection and wrong-status no-ops. Also assert both keys work while the keyboard focus is on a child control (a button inside the page), not only on the root. Red mutation: handler body empty; handler without the status check.

## G2 (MEDIUM) — the focus ring on a version row is a 2px line on its top edge

In the real window a focused version row shows only a thin blue line along the TOP edge of the row (the list container clips the rest of the ring); between two rows it reads as a separator and the radio shows no focus, so the user cannot tell which row holds focus. The engine cards, buttons, inputs, checkbox and sidebar row show a full ring. The sidebar group header ring was also only partly visible in my frame (a line along one edge).
Do: make the ring of a version row and of a group header an unmistakable full outline INSIDE the row bounds (an inset ring/border on all four sides, or an inset ring plus the existing tint; say which and why). Extend `tests/render/focus_ring.rs` so that a focused version row and a focused group header add ring-coloured pixels on ALL FOUR edges of their bounds (assert per edge, not a total count), and an unfocused one adds none. Red mutation: draw the ring only on the top edge (or drop `focus_ring_style` for version rows only).

## G3 (MEDIUM) — five mutants of mine are still green on the lib

Each needs a test that fails, or an honest line in the checklist why it cannot (not for the first three):
- (a) `confirm_delete` success path without `restore_dialog_focus_after_close` (`instance_actions.rs`, the `Ok(_)` arm): green. Drive it by keys (`open_delete_dialog` is `cfg(test)`, use it to open), confirm, let the background result land, then `ctrl-k` must open and `window.focused` is `Some`.
- (b) `restore_focus_if_quit_closed` made a no-op (quit dialog dismissed without returning focus): green. Test through the real quit path (dialog open, Escape/cancel, then `ctrl-k`).
- (c) `.focus_trap(...)` removed from `render_quit_dialog`: green. The F2 walk covers create/delete/settings/palette but not the quit dialog; add it to the Tab-containment set.
- (d) the group header's `apply_focus_ring` removed (it also drops its `track_focus`, so the header stops being a tab stop): green. The reachability test must visit the group header by Tab (and Enter/Space must collapse/expand it, assert the group state).
- (e) the palette `CopyUri` arm doing nothing: green, because your first-session test copies with `ctrl-shift-c`, not through the palette. Add the palette path: query `salin`, Enter, assert the clipboard text and the recorded `connection_uri` call for the right instance.
`E5` (live system appearance) stays a documented manual item; keep that line.

## Docs

`docs/dev/accessibility-checklist.md`: keyboard contract rows true after G1; the version-row ring row; the mutation table gets the new red results; record your own Xvfb run of G1/G2 (key -> what changed, detected by code). `docs/architecture/gui.md` only if the focus model text changes.

## Bar

fmt, clippy workspace and `--features demo`, deny, new tests x20 from a private copy of the GUI lib test binary (`--list` first), a11y_tree, render, contrast_table, `cargo xtask ci` under the heavy lock on the final head (`CARGO_BUILD_JOBS=8`, MemAvailable above 8000 MB), files < 800 lines (`keyboard_flow_tests.rs` is at 735: put the new tests in a new file), then the REAL WINDOW on Xvfb (same recipe as fix 1: private display, `env -u WAYLAND_DISPLAY`, no image opened, ring/edge detection by code): `ctrl+shift+x` stops the postgres processes and `ctrl+shift+s` starts them again, with the pgrep counts pasted; a Tab walk to a version row and a group header with the per-edge change pasted. Mutations (paste red, restored, clean tree): G1 empty handler, G1 no status check, G2 top-edge-only ring, G3 a-e. `worker_done` only with the red/green output pasted. Clean up: engines (`pgrep -x postgres` = 0), GUI (`pkill -x dblocal`, NEVER `pkill -f`), Xvfb, `/tmp` dirs, no `~/.config/systemd/user/dblocal.service`.
