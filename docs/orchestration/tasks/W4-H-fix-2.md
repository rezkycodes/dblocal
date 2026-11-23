# W4-H fix 2 — one real-window defect left, two nits

Same branch and worktree. `git merge main` first. Rules incl. 3 (never open an image) and 11-14. Small round: do not touch anything else.

## What the reviewer verified and accepts on `ed5c6b3` (do not redo)

fmt, deny, clippy (also `--features demo`), GUI lib 385 x3, a11y_tree 27, render 15, core lib x3 (one `ports::allocate::tests::real_checker_sees_a_listening_port` failure in one of three runs under foreign load from other desktop apps, 60/60 alone, no core code touched: not yours), release build. My two probes (failed restore must not start; failed stop must not leave a restart pending) are green. Real window (release binary, isolated home, real PostgreSQL 18.7 and Valkey): the grid is now aligned (all fields at one x, default/unit/reason cells inside the card, read-only values on the same columns); `dblocal config set pg-app max_connections=300` from a shell shows up when the tab is left and re-entered; with a typed `work_mem=16MB` pending, a second shell change `max_connections=350` and a return to the window (focus away and back) shows 350, keeps the typed 16MB highlighted and shows "File berubah di luar aplikasi". The reviewer's 20-mutation pass on the final head is still running; any gap it finds comes as an addendum on main.

## F8 (MEDIUM-LOW) — returning to the window clears a field's validation error but keeps the bad text, and Terapkan turns blue

Reproduction (real window): type `abc` into `work_mem` (`DBL-8601` appears, Terapkan disabled), move focus to another window and back. The re-read runs `rebase`, which does `self.errors.clear()` without validating again: the field still says `abc`, the error is gone and "Terapkan" is enabled. Pressing it only shows the error again, but the F5 invariant (no blue Terapkan while a field is invalid) is broken on this path, and the same happens after any `ConfigChanged` re-read and after tab re-entry (the form is read every time now).
Do: `rebase` (or its callers) must re-validate the surviving edits against the new view, so `errors` always matches `edits`; the typed text is kept. Tests (state level and handler level): invalid edit + re-read keeps the error and `can_apply() == false`; a valid edit + re-read has no error; an edit that the new file value makes equal disappears together with its error.

## F9 (LOW) — "satuan: bytes" next to a value written in MB

Every size row shows `128MB` / `4MB` with "satuan: bytes" under the default. The core `unit` is the base unit (`bytes`, `seconds`); the user types `16MB`, so the label reads as a contradiction. Render a unit text that matches what the field accepts, e.g. "satuan: byte (boleh KB, MB, GB)" for `bytes` and "satuan: detik" for `seconds`, in `strings`; keep the id and the fixed cell. Test the string function for both units and the empty unit.

## F10 (LOW) — a re-read erases the card error

`begin_refresh` sets `error = None` on every read, so a failed apply or a failed open shows its `DBL-86xx` banner only until the window is re-activated or the tab re-entered. Keep the card error through a re-read that succeeds when it belongs to a failed apply/restore/open (the next user action or "Muat ulang" clears it); only a failed READ error is replaced by the next read. Test both.

## Mutation gaps (addendum, reviewer's 20 mutations on `ed5c6b3`: 18 red, 2 green)

All 14 behaviours of the first table and the fix behaviours F1-F7 are pinned (restore-after-failed-restore, pending restart vs another op / another instance, banner retirement, `can_apply`, focus after Confirm, re-entry re-read, activation re-read, refresh not blanking, same-file banner, grid `min_w(0)` and cell width). Two stay green; pin each, red against the mutation then green (paste both):

| # | Mutation | Behaviour to pin |
|---|---|---|
| N13 | `open_config_raw` without its `reload_config_bg` after the launcher returns | after "Buka file konfigurasi" returns, the form shows what the service now reads (script `config_view` to change between the open and its completion, assert the new value is in the state) even when no window activation happens |
| N14 | `stop_instance_tracked` Err branch without `clear_pending_restart_for` | a stop that cannot even be requested (`stop_instance` returns `Err`, script it) leaves no pending restart, reports the typed error, and a later manual stop does not start the instance |

## Bar

fmt, clippy (also `--features demo`), deny, new tests x20 from a private copy of the lib binary, a11y_tree and render, `cargo xtask ci` under the heavy lock on the final head, and a real-window check for F8 only (the reproduction above; a second window can be a `python3 -c "import tkinter ..."` window). Clean up engines, GUI and `/tmp/dblocal-w4h*`. `worker_done` with the red/green output pasted.
