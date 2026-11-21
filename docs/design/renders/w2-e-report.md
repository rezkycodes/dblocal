# W2-E worker report — review round 2 (resume)

## 1. Summary

- **Logs tab now paints lines for real.** The cause was not `uniform_list` itself: `screen-fill`
  was a block `div()` wrapping `page_fill()`, so `page_fill`'s `flex_1` had no flex parent to grow
  in; the whole viewer collapsed to 16 logical px (only header/footer) and the virtualised list had
  no room. `screen-fill` is now a `v_flex()` with `flex_1/min_h_0`; the list also carries
  `size_full()` so it has a definite height.
- Both vacuous log tests were replaced by one layout-derived test that locates the two
  `table_row_border` rows of the viewer and counts text bands strictly between them.
- **`.env` masking is now secret-driven, not key-prefix guessing.** `mask_dotenv_with_secret`
  replaces the raw and percent-encoded secret from the URI the view holds, and masks any
  `scheme://user:pass@` userinfo and `*PASSWORD` keys. Verified on the real `engine::uri` builder
  output for PostgreSQL, MySQL, MariaDB and Valkey.
- Databases placeholder card 1 now has the prototype's header row (title + field + button) and the
  same card padding as the others; the Config placeholder renders and is pinned at view-model level.
- 20/20 default-thread render runs passed with no watchdog fire.

## 2. Branch / commit / diff

- Branch: `rezkycodes/w2-ui-port`
- Last commit: see the `worker_done` message (`git log --oneline -3` pasted there).
- `git status --short` after the commit: empty.
- Largest file I own: `crates/gui/tests/render.rs` at 787 lines (under the 800 hard maximum).
  The masking code was split into `crates/gui/src/format/masking.rs` (176) and its tests into
  `crates/gui/src/format/masking_tests.rs` (124); `format/mod.rs` is 743 and `format/tests.rs` 766.

## 3. Round-2 items

### 3.1 Logs tab paints no log lines

**Root cause (bisected, not guessed).** Giving the viewer `.h(px(240.))` or `screen-fill`
`.h(px(400.))` changed nothing; giving `page_fill` a concrete height did. That proved the collapse
was in the ancestor chain. `screen-fill` was `div()` (block). `page_fill`'s `flex_1` therefore had a
block parent with no free-space distribution and laid out at content height; the viewer's
`flex_1/min_h_0` then shrank the list to 16 logical px.

Measured on the round-1 PNG (`logs-diagnosis-dark.png`): header text band y 679..705, footer text
band y 781..806, header bottom border 726/727, footer top border 760/761 → body 32 device px.
After the fix header border is 726/727 and footer border 1492/1493 → body 766 device px.

**Failing-first proof.** Reverting `screen-fill` to `div()` makes the new test fail immediately:

```
RENDER .../renders/logs-diagnosis-dark.png (2560x1640, 3016 colours, target-only)
thread 'renders_logs_body_bands_from_layout' panicked at crates/gui/tests/render.rs:492:9:
could not locate log viewer borders; found borders at [727, 761]
```

The 727/761 pair is exactly the collapsed body the coordinator measured. The red output is also
recorded as a comment inside `renders_logs_body_bands_from_layout`.

**ink_bands evidence on the committed renders** (rect = between the two viewer borders):

```
$ python3 -I /tmp/claude-1000/ink_bands.py docs/design/renders/logs-diagnosis-dark.png 680 728 2440 1492
image (2560, 1640) rect (680, 728, 2440, 1492) background (21, 22, 29)
  band y 754..771  height 18
  band y 790..811  height 22
  band y 826..846  height 21
  band y 862..879  height 18
  band y 898..915  height 18
  band y 934..951  height 18
  band y 967..990  height 24
  band y 1004..1028  height 25
8 ink band(s)

$ python3 -I /tmp/claude-1000/ink_bands.py docs/design/renders/logs-light.png 624 468 2480 1490
image (2560, 1640) rect (624, 468, 2480, 1490) background (248, 248, 251)
  band y 494..516  height 23
  band y 530..547  height 18
2 ink band(s)

$ python3 -I /tmp/claude-1000/ink_bands.py docs/design/renders/logs-short-light.png 624 468 2480 1490
  band y 494..511  height 18
1 ink band(s)

$ python3 -I /tmp/claude-1000/ink_bands.py docs/design/renders/logs-long-light.png 624 468 2480 1490
  ... 28 ink band(s)  (body 511 logical px / 18 px rows = 28 visible rows)
```

The test asserts 8 / 2 / 1 and, for the 200-line follow case, exactly
`floor(body_height / 18)` bands with the last band within 24 logical px of the body bottom.

### 3.2 `.env` showed the password in clear (new security defect)

- `format::mask_dotenv_with_secret(block, uri)` takes the URI the view holds (now kept inside
  `UriView` as a `SecretString`, redacted by `Debug`), extracts the password, and replaces the raw
  **and** percent-encoded forms everywhere. It also masks any value with `scheme://…:…@` userinfo
  and any `*PASSWORD` key. `masked_env_rows` uses it in the hidden path.
- Tests use the real `dblocal_core::engine::uri::{build_uri, build_dotenv}` builder with the secret
  `p@ss:w/rd%` for PostgreSQL, MySQL, MariaDB and Valkey; they assert the raw and encoded forms never
  appear, that Show reveals and Hide clears, and that the tab-close path (`forget_uri`) drops it.
- Mutation proof: restoring the round-1 `mask_dotenv` (only `DB_PASSWORD=`/`REDIS_PASSWORD=` prefixes)
  reproduces the coordinator's finding:

```
Postgresql hidden .env leaked the percent-encoded secret:
DATABASE_URL=postgresql://root:p%40ss%3Aw%2Frd%25@127.0.0.1:5432/postgres
DB_HOST=127.0.0.1
...
DB_PASSWORD=••••••
```

With the fix the same line reads `DATABASE_URL=postgresql://root:••••••@127.0.0.1:5432/postgres`.
The demo MariaDB case now renders `DATABASE_URL=mysql://root:••••••@127.0.0.1:3307/` (it previously
printed `pwd-ec621e61`).

**Audit of every path where URI/env text can reach a Label, clipboard, toast, tracing or Debug:**

| Path | Secret can reach? | Why safe |
|---|---|---|
| `overview.rs` URI `Label` via `uri_text()` | masked unless revealed | `UriView::text()` returns `masked` or the revealed secret |
| `overview.rs` `.env` rows via `dotenv_rows()` | masked unless revealed | goes through `masked_env_rows` → `mask_dotenv_with_secret` |
| `app.rs::copy_uri` → clipboard | yes, explicit Copy URI/Copy .env | the task allows the explicit copy buttons; nothing else |
| `app.rs::copy_logs` → clipboard | no | log lines never contain the credential |
| `app.rs::report_error` → toast | no | `UserFacingError` code + message; no secret |
| `tracing` calls in `crates/gui` | no | no URI/env/secret is logged (`grep` for `tracing::` + `uri/password/secret` is empty) |
| `UriView` `Debug` | no | stores `SecretString`, which prints `SecretString(***)` |
| `preview.rs` / `state.rs` test `expose()` | test-only | not in the product build |

Only `copy_uri` writes the plaintext, and only from the two explicit copy buttons and the
`Ctrl/Cmd+Shift+C` action.

### 3.3 Small items

- **3a padding.** `databases-product-light.png` card 1 is now one header row (title `Database`,
  `nama_database` field, `Buat database`) followed by the message box, all inside `card()`'s 18 px
  padding (`placeholder_card` renders controls before `empty`). Measured on the old vs new PNG at
  x = 1000 device px:

  ```
  OLD  card 1 bottom border 639; last content run 635..639  (text on the border)
  NEW  card 1 bottom border 735; white padding 698..733, message box 591..697
  ```

  So the content that used to end on the card's bottom border now ends 18 logical px above it. The
  4 px blue strip at y 368..371 is the active Databases sub-tab underline, not a control (it is
  present in the old render too).
- **3a config render.** New `config-product-light.png` from `Workspace::preview_product_config`,
  rendered and asserted (`renders_config_placeholder_in_the_product_build`). The Config control labels
  and their disabled state are pinned by
  `format::tests::placeholder_layouts_match_the_prototype_structure` (`UI_CONFIG_LATER`,
  `UI_CONFIG_RESTART_NOW`, `UI_CONFIG_OPEN_FILE`, `UI_CONFIG_REVERT`, `UI_CONFIG_DISCARD`,
  `UI_CONFIG_SAVE`, all `disabled`).
- **3b default-thread render loop.** `RENDER_LOOP pass=20 fail=0`; the watchdog never fired. Each run
  executes all 9 render cases on default test threads.

## 4. Quality gates

Target: `CARGO_TARGET_DIR=$HOME/.cache/dblocal-target-gui-w2-ui-port`.

| Command | Result | Key output |
|---|---|---|
| `cargo fmt --all -- --check` | PASS | `FMT_OK` |
| `cargo clippy -p dblocal-gui --all-targets -- -D warnings` | PASS | `Finished dev profile ... (0 warnings)` |
| `cargo test -p dblocal-core --lib --features mock` | PASS | `563 passed; 0 failed; 1 ignored` |
| `cargo test -p dblocal-gui --lib` | PASS | `123 passed; 0 failed` |
| `cargo test -p dblocal-gui --lib` ×40 | PASS | `GUI_LIB_LOOP pass=40 fail=0` |
| `cargo test -p dblocal-gui --test contrast_table` | PASS | `1 passed` |
| `cargo test -p dblocal-gui --test render -- --test-threads=1` | PASS | `9 passed; 0 failed` |
| render binary, default threads ×20 | PASS | `RENDER_LOOP pass=20 fail=0`, no watchdog |
| `cargo llvm-cov -p dblocal-gui --summary-only` | PASS | `TOTAL Lines 82.42%` |
| `cargo deny check licenses bans` | PASS | `bans ok, licenses ok` |

Coverage for the files I touched: `format/mod.rs` 95.56 %, `format/placeholders.rs` 100 %,
`app/state.rs` 99.00 % (lines), `logs.rs` 99.54 %, `app/screens/logs.rs` 86.27 %,
`app/preview.rs` 94.24 %, `app/screens/overview.rs` 87.94 %,
`app/screens/databases.rs` 70.16 % (layout-heavy), `app/screens.rs` 78.04 % (layout).

## 5. Renders regenerated (DBLOCAL_UPDATE_RENDERS=1)

Changed: `databases-product-light.png` (card-1 header row/padding), `logs-diagnosis-dark.png`,
`logs-light.png`, `overview-dark.png`, `overview-light.png` (masked `.env`), `overview-failed-dark.png`,
`stale-light.png`, `dock-all-dark.png`, `dock-filtered-dark.png` (shared chrome re-render).
New: `config-product-light.png`, `logs-short-light.png`, `logs-long-light.png`.

## 6. Remaining visual deviations from the prototype

| Deviation | Leave? | Why |
|---|---|---|
| Dock engine order | no longer deviates | round 1 fixed it; the coordinator confirmed |
| Data dir truncated with an ellipsis | leave | the full path does not fit the Detail card column at 1280×820; changing the card width would deviate further |
| Title bar shows "Terbaru" instead of "Katalog diperbarui …" | leave | the prototype's phrase needs a relative clock ("3 jam lalu"); the catalog read model carries a freshness state, not an age. "Terbaru" is the accurate string from `strings.rs` |
| PID shows `—` | leave | the frozen `InstanceView` has no PID; inventing one would break the "nothing invented" rule |
| Dock status word only in tooltip/accessible label | leave | the prototype row is one line; the name/version/port row matches it and the word stays available to assistive tech |
| MySQL/MariaDB URI ends after the port | leave | matches the real `engine::uri::build_uri` |
| Product Databases/Config are placeholders | leave | `F-DBU` Phase 2 / `F-CFG` Phase 3 have no frozen-service backend |

## 7. Could not verify

- Windows and macOS builds/runtime (Linux host only).
- Light theme on real hardware (headless render + contrast test only).
- The real `LocalService` (W2-B owns it); these renders use the mock, so the `.env` masking is proven
  against the real `engine::uri` builders in unit tests, not against a live instance.
