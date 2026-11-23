# W4-B fix 1 — the Databases tab shows a false empty list after a start, and four smaller defects (reviewer ran the real window)

Same branch `rezkycodes/w4-gui-admin-open-settings`, same worktree. Rules `docs/orchestration/tasks/_COMMON-RULES.md` incl. 11-15 (rule 15: only your own worktree, stage explicit paths). FIRST `git merge main` (main moved: W2-G, W4-A2, W4-C and docs; the reviewer's trial merge of your head onto main is conflict-free). Do not touch `dblocal-gpui-kit-source/`.

## What the reviewer verified and accepts (do not redo)

On the trial merge (main 0ed3469 + your a1b407e): fmt, deny, clippy workspace (also `--features demo`), GUI lib x3 (295), a11y_tree 26, render 15, core lib x3 (mock, 1452). Real release binary under Xwayland, isolated home, real PostgreSQL 17.12 (a pixel click in `delivery_mode:"foreground"` DOES select the instance row and tabs; background clicks do not, and typing works in foreground too): create database `appdb` from the tab (psql: exists, UTF8, owner postgres, list refreshed, toast); drop dialog with a wrong name leaves the confirm button disabled and Enter does nothing (psql: `appdb` still there), with the exact name Enter drops it (psql: gone); `postgres` row has no actionable drop control; user `alice` Read-only on `shop` (psql: SELECT ok, INSERT `permission denied for table t`, CREATE TABLE denied), the generated password is shown once with Salin / "Sudah saya simpan" and is gone from the accessibility tree after the dialog closes; Reset password: confirmation, new URI dialog, psql: old password `authentication failed`, new password works, `dblocal uri pg1` equals the shown URI; stopped instance shows the empty state with "Nyalakan instance"; Settings dialog from the gear: version, "Periksa sekarang" -> `DBL-8011 — Sumber rilis pembaruan tidak ditemukan atau tidak publik`, login switch with the real unit path; "Open in…" menu lists a detected GUI client and "Perbarui daftar client". Engines, GUI and the home were removed afterwards.

## F1 (HIGH, blocks the merge) — after a start, the Databases tab keeps showing "Database · 0 / Belum ada database" for a server that has databases

Reproduction (real binary): `dblocal create` two PostgreSQL instances, start only `pg1`, open the GUI, select `pg2` (stopped), tab Databases -> "Jalankan instance untuk melihat dan mengelola database." + "Nyalakan instance"; click it; the instance becomes `berjalan` and the tab shows `Database · 0`, `Belum ada database.` and, in the user card, `Belum ada database.` although the server has `postgres`. Leaving the tab (Overview) and coming back shows the same. Only an action that emits `AdminChanged` (create database) refreshes it.
Cause (read it, confirm it): `reload_admin_bg` (`admin_actions.rs`) stores `Loadable::Ready(AdminData::default())` for a non-running instance, and `enter_databases_tab` reads only for `Idle` or a failed read, so a stale `Ready(empty)` is never re-read; nothing resets the admin state when the instance status changes to running.
Do, red first: a headless test with the scripted service where an instance starts `Stopped`, the tab is entered, the instance becomes `Running` (the `InstanceChanged` path), and the tab must re-read and show the service's rows; also the reverse (running -> stopped must not keep showing rows as if they were live: show the empty state again). Do not store a fake `Ready(empty)` for a stopped instance (a stopped state is its own variant or simply "not read"). Paste the red output on the current head, then green. Also re-check the same transition for an instance started by the CLI while the GUI is open (the status arrives as an event, not through the button).

## F2 (MEDIUM) — the login switch shows OFF after a failed disable of an enabled login item

`set_login_item` replaces the state with `LoginState::Failed(err)` and `login_enabled()` reads only `Known`, so after `systemctl --user disable` fails for an enabled item the switch is rendered off while the unit is still enabled (the comment on `login_enabled` says "last confirmed state", it is not); during `Busy` the switch also flickers to off. Fix: keep the last confirmed `LoginStatus` through `Busy` and `Failed` (or re-read the status after a failed write) and render that; the typed error stays visible. Tests (red first): enabled -> disable fails -> switch still on + error; disabled -> enable fails -> switch off + error; busy keeps the previous value.

## F3 (MEDIUM) — the drop dialog does not take keyboard focus

Opening the drop dialog leaves the focus in the covered "Buat database" field: typing `x` appended to the field behind the overlay and the dialog's own field stayed empty (reviewer screenshot sequence). A keyboard user can neither type the name nor use "Enter confirms" without clicking into the field first. Do: focus the dialog's input when the dialog opens (`InputState::focus` / window focus handle, whatever GPUI Kit offers) and restore the previous focus on close. Same check for the reset confirmation and the secret dialog (the Copy / "Sudah saya simpan" buttons should be reachable by Tab, Escape closes). A GPUI test that asserts which entity holds focus after open and after close. Say plainly what you could not verify with the real window.

## F4 (LOW) — visible duplicate text in the drop dialog

The prompt "Database dan seluruh isinya dihapus permanen. Ketik nama database untuk mengonfirmasi." is rendered twice (`-prompt` muted and `-type-name` in red) and the heading repeats the title ("Hapus database" / "Hapus database — appdb"). Render the sentence once (the danger line may be a short distinct warning, from `strings`), no duplicate ids.

## F5 (LOW) — wrong stable code on a dead path

`create_user` builds `UserFacingError::new("DBL-8106", strings::UI_DATABASES_USER_DATABASE)` when no database is chosen: `DBL-8106` is "database in use" and the message is the label "Database". The button is disabled in that state so it is unreachable today, but a stray code must not exist: remove the branch (make the button the only gate) or give it a message from `strings` and no `DBL-` code of the core registry.

## F6 (LOW) — stale feedback

After a successful create the name field keeps its text (a second click gives "already exists"); clear it on success (database and user). After a successful drop no toast is shown and the status bar keeps "Database appdb dibuat (UTF-8)" from the earlier create; show a drop confirmation toast ("Database <name> dihapus") from `strings`.

## F7 (MEDIUM) — three mutations stay green (the spec asked for these guarantees)

Reviewer mutations on the trial merge, each leaves all 295 GUI lib tests green:
- `confirm_drop` calls `service.drop_database(&iid, &name, true)` instead of `false` (the GUI would silently kill other sessions): no test pins `force == false`. Test through a scripted service that records the arguments of `drop_database`.
- the `UiEvent::AdminChanged` arm in `app.rs` no longer sets another instance's rows back to `Idle` (`if false {`): the test only covers the visible instance, which `apply_event_with_cx` re-reads anyway. Test the non-visible instance: its rows change, `AdminChanged` arrives, entering its tab issues a fresh read.
- the "user created" toast built as `format!("{user} {password}")` (the one-time password in a toast): the spec says "never in a toast, never logged". Add a canary test: after `create_user` and after `confirm_reset` with a known secret from the scripted service, neither the toast, the status-bar text, the card error, nor any `Debug`/`Display` of the view state contains the secret while the dialog is open (only the dialog may), and nothing contains it after the dialog closes.
Paste each mutation red on your head after the test is added, restored, clean `git status --short`. The other six reviewer mutations (drop confirm accepts any text, row ids ignore the instance, secret dialog kept after close, login switch shows enabled after a failed call, confirm_drop skips its re-check, reset failure flag never set) were red: keep those tests.

## Bar

fmt, clippy workspace (also `--features demo`), deny, GUI lib x20 from a private copy of the test binary (check with `--list` that your new tests are inside it), a11y_tree and render tests, `cargo xtask ci` under the heavy lock on the final head (`CARGO_BUILD_JOBS=8`, MemAvailable above 8000 MB), real-window evidence for F1 (the two-instance reproduction above, after the fix: `pg2` shows `postgres` right after the start) and F3 (type into the dialog without clicking first) using `delivery_mode:"foreground"` clicks and typing under Xwayland, isolated `DBLOCAL_HOME`, engines and home removed afterwards (`pgrep -x postgres` must be 0), never leave `~/.config/systemd/user/dblocal.service` behind. `worker_done` only with the red and green outputs pasted.
