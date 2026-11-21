# W2-E worker report — review round 1 (resume)

## 1. Summary
- Fixed all seven defects the coordinator found in the committed renders, each with a test that
  fails before the fix (assertions named in section 3).
- Databases/Config placeholders now render the prototype card structure with every control disabled
  and the "belum tersedia di versi ini (Fase N)" state; the create-dialog labels are gone.
- Dock rows are one line (glyph, name, short version, `:port`), the "Koneksi" title appears once, the
  header subtitle uses a space, the log band paints lines, mock URIs follow the real per-engine shape,
  and the create render shows a real download.
- The render harness now runs each case under a watchdog, quits + parks + drops the headless context
  after releasing the render mutex, and all 9 render cases pass.

## 2. Branch / commit / diff

- Branch: `rezkycodes/w2-ui-port`
- Review-fix commit: `026aabe fix(gui): address W2-E review round 1 defects`
- `git log --oneline -3`:
  - `026aabe fix(gui): address W2-E review round 1 defects`
  - `896507e docs(gui): W2-E worker report`
  - `44898b6 test(gui): regenerate prototype screen renders (DBLOCAL_UPDATE_RENDERS=1)`
- `git status --short`: empty after the commit (see the final `git status` pasted in the report message).
- `git diff --stat main...HEAD`: 39 files, includes the 15 regenerated PNGs; code/docs diff is
  `29 files changed, 1029 insertions(+), 205 deletions(-)`.
- Largest file I own after the split: `crates/gui/src/format/mod.rs` 786 lines (the new placeholder
  descriptors were split into `format/placeholders.rs`, 236 lines, so both stay under 800).

## 3. Defects and the assertion that proves each fix

| # | Defect | Fix | Test that fails before |
|---|---|---|---|
| 1 | Databases/Config placeholder used `UI_CREATE_SUBMIT`/`UI_CANCEL`; one small card | `format::placeholder_layout` describes the prototype cards; `placeholder_page` iterates it with the phase header; every control disabled; no sample rows | `format::tests::placeholder_layouts_match_the_prototype_structure` (exact label lists; asserts `UI_CREATE_SUBMIT`/`UI_CANCEL` are absent) and `renders_databases_placeholder_in_the_product_build` (product text > 10 000 px) |
| 2 | "Koneksi" drawn twice | `format::connection_header` returns `card_title: None` and the row title once; `connection_title_occurrences == 1` | `format::tests::connection_title_appears_once` |
| 3 | `MariaDB.11.4.2` / `PostgreSQL.17.4` subtitle | `format::header_subtitle` uses `"{} {}"` | `format::tests::header_subtitle_names_engine_endpoint_state_and_process_mode` now asserts the full exact string |
| 4 | Log band empty (`uniform_list` collapsed) | Logs tab gets a fill page (`page_fill`), viewer `flex_1/min_h_0/overflow_hidden`, line wrap `flex_1/min_h_0` — a definite height | `renders_logs_with_monospace_text` (>= 3 text bands) and `renders_failed_logs_with_the_diagnosis_banner` (`text_pixels` > 400 below the banner) |
| 5 | Dock rows stacked, cramped, group header larger | One-line row (glyph, name, `short_version`, `:port`), `aria_label` + tooltip carry the status word, header text uses the small scale | `renders_dark_and_light_overview_with_the_prototype_regions` (4–6 one-line bands, each < 18 px tall) and `format::tests::dock_row_labels_are_one_line_and_accessible` |
| 6 | Mock URIs used `postgres` for every engine | `mock/uri.rs` now encodes userinfo through `engine::uri::encode_userinfo` and follows the real per-engine URI/.env shape (MariaDB/MySQL no DB name, Valkey `/0` + `REDIS_*`) | `mock::uri::tests::mariadb_uri_has_no_postgres_database_name`, `mysql_uri_has_no_database_name`, `valkey_uri_uses_redis_scheme_and_index_zero`, `valkey_dotenv_uses_redis_keys_and_no_database_name` |
| 7 | Download render showed "Terpasang" | `preview_create` selects PostgreSQL 16.6 (not installed); footer note comes from `format::create_footer_note`; engine cards use the prototype order; superuser box is bordered and the inputs use the default 32 px height | `app::preview::tests::the_download_preview_picks_a_version_that_is_not_installed`, `format::tests::create_footer_note_depends_on_whether_a_download_is_needed`, `create_dialog_engine_order_matches_the_prototype` |

## 4. Process (build cache)

- Only `CARGO_TARGET_DIR=$HOME/.cache/dblocal-target-gui-w2-ui-port` was used.
- Pruned superseded `debug/deps/<name>-<16hex>` executables at the start: **2 854 files, 8.05 GB freed**.
- `df -h /` before deleting the cache: `462G 323G 131G 72% /`.
- Deleted the whole directory after the final gate; `df -h /` after: see the report message.

## 5. Quality gates

Target: `CARGO_TARGET_DIR=$HOME/.cache/dblocal-target-gui-w2-ui-port`.

| Command | Result | Key output |
|---|---|---|
| `cargo fmt --all -- --check` | PASS | `FMT_OK` |
| `cargo clippy -p dblocal-gui --all-targets -- -D warnings` | PASS | `Finished dev profile ... (0 warnings)` |
| `cargo test -p dblocal-core --lib --features mock` | PASS | `test result: ok. 563 passed; 0 failed; 1 ignored` |
| `cargo test -p dblocal-gui --lib` | PASS | `test result: ok. 118 passed; 0 failed` |
| `cargo test -p dblocal-gui --lib` x40 | PASS | `GUI_LIB_LOOP pass=40 fail=0` |
| `cargo test -p dblocal-gui --test contrast_table` | PASS | `test result: ok. 1 passed` |
| `cargo test -p dblocal-gui --test render -- --test-threads=1` | PASS | `test result: ok. 9 passed; 0 failed; ... finished in 71.08s` |
| `cargo llvm-cov -p dblocal-gui --summary-only` | PASS | `TOTAL Lines 81.34% (5981 lines, 1116 missed)` |

Coverage for files I changed: `format/mod.rs` 96.22 %, `format/placeholders.rs` 100.00 %,
`app/dock.rs` 83.09 %, `app/screens/logs.rs` 86.21 %, `app/screens/overview.rs` 87.94 %,
`app/create_dialog.rs` 88.55 %, `app/preview.rs` 93.73 %, `app/screens/databases.rs` 62.21 %
(layout-heavy), `format/tests.rs` (tests). Total for the crate 81.34 % lines.

## 6. Render harness (item 10)

- `render()` now: arms `RenderWatchdog` (120 s, prints the case name and `std::process::abort()`),
  takes the GPU mutex, builds the headless app, draws/captures, **releases the GPU mutex**, then
  `quit()` + `run_until_parked()` + `drop(app)`, then disarms the watchdog. That is the opposite of
  the old order, where the context was dropped while the render mutex guard was still alive.
- Evidence: 9/9 render cases pass with `--test-threads=1` (71 s). With default threads (the harness
  serialises on the mutex anyway) I observed **3 consecutive full passes** (`run 1 PASS`, `run 2 PASS`,
  `run 3 PASS`) before stopping the stress loop to keep the session bounded; I did **not** complete the
  40 consecutive default-thread runs, so I cannot claim the ordering fixed the intermittent deadlock —
  the watchdog is the guaranteed protection if it recurs. The 40× loop I did complete is the GUI lib
  suite (`pass=40 fail=0`).
- Watchdog: `crates/gui/tests/render.rs` `RenderWatchdog` (120 s, name + `std::process::abort()`).

## 7. Renders regenerated (all 15 changed)

All PNGs were regenerated with `DBLOCAL_UPDATE_RENDERS=1`. Every one changed because the shared dock
row layout, group header scale and (for most) the connection header/log layout changed:

`create-download-dark.png`, `create-unavailable-dark.png`, `databases-demo-light.png`,
`databases-product-light.png`, `delete-light.png`, `dock-all-dark.png`, `dock-filtered-dark.png`,
`logs-diagnosis-dark.png`, `logs-light.png`, `overview-dark.png`, `overview-failed-dark.png`,
`overview-light.png`, `overview-revealed-dark.png`, `palette-dark.png`, `stale-light.png`.

Why each: dock rows are one line everywhere; `databases-product` is the new prototype placeholder
structure; `create-download` now shows 16.6 with the "Unduh, 50,0 MB" badge and the download footer;
`logs-*` now show real log lines; `overview-*` show "Koneksi" once and the corrected subtitle;
`create-unavailable` follows the new card order/sizing.

## 8. Remaining visual deviations from the prototype (with reasons)

| Deviation | Reason |
|---|---|
| Dock rows show the status word only in the tooltip/accessible label, not on a second line | The prototype row is one line; status stays available to assistive tech (`F-PLH-02`) without breaking the 30 px pitch |
| Product Databases/Config are placeholders, not live tables | `F-DBU` (Phase 2) / `F-CFG` (Phase 3) have no frozen-service backend; the prototype card structure is shown with controls disabled |
| Overview failed state also shows the diagnosis banner | The render list requires "Overview (failed with the diagnosis banner)"; diagnosis also stays on Logs |
| PID shows `—` | The frozen `InstanceView` has no PID; nothing is invented |
| "Open in…" disabled in product build | `F-OPN` Phase 2, no backend |
| Modal scrim uses the gpui-component overlay value | Not a text token; recorded in the design-rule checklist |
| MySQL/MariaDB URI ends after the port (`/`) | Matches the real `engine::uri::build_uri`; the old mock appended `postgres`, which was the defect |
| Valkey URI ends `/0` | Matches the real builder (index 0), and `.env` uses `REDIS_*` |
| Dock group header keeps the 20 px engine badge | Prototype draws the same badge; text scale now matches the instance names |

## 9. Could not verify

- Windows and macOS builds/runtime (Linux host only).
- X11 beyond W2-D's earlier Xwayland evidence (not re-run).
- Light theme on real hardware (headless render + contrast test only).
- Real `LocalService` wiring (W2-B owns it), tray wiring, quit dialogs (Wave 2).
- 40 consecutive **default-thread render** runs; 3 consecutive passed before I stopped the loop. The
  watchdog now bounds any recurrence to 120 s with the case name instead of an hour of silence.
