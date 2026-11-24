# Windows verification checklist (WIN)

This document is the checklist the owner follows after rebooting this dual-boot machine into
Windows. It is a verification guide, not a claim that Windows works.

**Status:** unverified. Nothing in this file claims a result. Record actual outcomes in the
"Outcome" column when you run it.

## Scope

Verify DBLocal on Windows with real user-visible behavior: build, run, tray, data directory
behavior, engine lifecycle, MSI install/uninstall, and SmartScreen first-run. This covers the
Windows-specific risks that cannot be validated on Linux.

## Prerequisites

Install these before cloning/building:

| Tool | Notes | Expected |
|---|---|---|
| Windows 10/11 x64 | Dual-boot target machine. | OS boots normally. |
| Git for Windows | Needed to clone repo. | `git --version` works. |
| Rustup + stable Rust | Install from https://rustup.rs. Use stable toolchain (repo pins via `rust-toolchain.toml`). | `rustup show` reports stable; `cargo --version` works. |
| Visual Studio Build Tools / MSVC | "Desktop development with C++" workload, including MSVC, Windows SDK. Required by Rust MSVC target and native deps. | `cl.exe` available in Developer shell, or cargo can build. |
| Vulkan-capable GPU driver | GPUI renders through Vulkan. Install vendor driver (NVIDIA/AMD/Intel) or ensure Vulkan runtime is present. | `vulkaninfo --summary` works if Vulkan SDK/tools installed; otherwise a Vulkan app can start. |
| PowerShell 7+ (optional) | Useful for checksums and scripted steps. Windows PowerShell 5.1 also works for most commands. | `pwsh --version` or `powershell -Command $PSVersionTable.PSVersion`. |

Optional for MSI verification:

| Tool | Notes |
|---|---|
| WiX Toolset v3 | Only needed to build MSI locally. CI builds MSI on `windows-latest`. |
| `gh` CLI | Optional for attestation/checksum convenience. |

## Get the code

```powershell
git clone https://github.com/rezkycodes/dblocal.git
cd dblocal
git checkout <tag-or-commit-to-verify>
git status --short
```

Expected: clean working tree, correct tag/commit.

## Build and automated checks

Run from a Developer PowerShell/Command Prompt with Rust on PATH:

```powershell
cargo xtask ci
```

Expected: format, clippy, tests, and CI gates pass. If a gate fails, record the full command and
key output; do not mark the check as passed.

Run the CLI directly:

```powershell
cargo run -p dblocal-cli -- --help
```

Expected: CLI help prints. Then run the GUI app path:

```powershell
cargo run -p dblocal-cli
```

Expected: DBLocal window opens (without subcommand, CLI launches GUI). Record any Vulkan/driver
error; if Vulkan is missing, the app should show an actionable message rather than crashing.

## Manual verification matrix

Record every row. Use `Pass`, `Fail`, or `Blocked` with notes.

Rows depending on the tray and quit flow assume the GUI-wiring task (W2-B) has been merged into the commit under test; the owner must record the commit in the "Built version" row.

| Area | Steps | Expected result | Outcome | Notes |
|---|---|---|---|---|
| Built version | Run `dblocal --version` (or `cargo run -p dblocal-cli -- --version`) in the tested checkout. | Owner records the reported version and the exact commit SHA under test. | | |
| Window opens | `cargo run -p dblocal-cli` | Main DBLocal window appears; no crash; UI readable. | | |
| Vulkan missing/driver issue | Run on machine/session with broken Vulkan driver if reproducible. | Actionable error, not silent crash/black window. | | |
| Tray icon present | Start app, inspect system tray/notification area. | DBLocal tray icon appears. | | |
| Tray menu actions | Open tray menu; exercise show/open/quit actions as available. | Actions work; no duplicate app instances; no crash. | | |
| Close-to-tray | Click window close button. | Window hides to tray if that is the configured behavior; app keeps running. | | |
| Quit dialog / quit action | Quit from tray/app menu. | Quit confirmation behaves as designed; app exits cleanly; engine processes stop as expected. | | |
| Close while an engine is running | Start a real engine instance, then quit the app. | Quit dialog lists running engines; choose Stop all; engines stop and no orphan processes remain. | | |
| Data dir creation | Start app and create/use default config. | `%USERPROFILE%\.dblocal` created as needed; no writes outside user profile except package install paths. | | |
| Credential Manager create | Create instance/secret that stores credentials. | Credential entry appears in Windows Credential Manager with expected name/scope. | | |
| Credential Manager remove | Delete instance/credential through app. | Corresponding Credential Manager entry removed; no orphan secret remains. | | |
| Rename behavior | Rename instance/data as supported by app flow. | Rename succeeds; old name not left active; paths updated. | | |
| Symlink behavior | If symlinks/junctions are exercised by engine/runtime install, verify behavior. | No path escape outside allowed dirs; operations follow Windows permissions. | | |
| ACL / permissions | Inspect `%USERPROFILE%\.dblocal` ACLs for sensitive dirs/files. | Private data not world-readable beyond normal user profile defaults; secrets not written to plain files unexpectedly. | | |
| PostgreSQL start/stop | Install/start a real PostgreSQL instance on a non-default high port if needed. | Instance starts, reports healthy/running, accepts local connection, stops cleanly. | | |
| PostgreSQL port binding | Check listening port. | Binds localhost only (`127.0.0.1`/`::1`), not `0.0.0.0`. | | |
| Valkey start/stop | Install/start a real Valkey instance (if Windows artifact/support is available). | Instance starts/stops cleanly on localhost port. If unsupported on Windows, record as Blocked with reason. | | |
| Open in... launcher (psql/mysql in a terminal) | Needs the GUI wiring of "Open in..." (until then the library `dblocal_core::clients` is covered only by Linux unit tests that check the generated text, never `cmd.exe`). Install a client such as `psql.exe`, create a password instance, run Open in... psql from a build that wires it. Use a password containing `%`, `^`, `&` and `!` if the instance allows setting one. | A terminal opens (Windows Terminal if `wt.exe` exists, otherwise the fallback) and `psql` connects without prompting for a password. The password is NOT visible in the terminal's command line (check with Process Explorer or `wmic process get commandline`). Nothing named `dblocal-open-*.cmd` remains in `%LOCALAPPDATA%\dblocal\run` once the terminal has started; a leftover older than 10 minutes is swept at the next launch. | | |
| Open in... launcher directory ACL | After the row above, inspect `%LOCALAPPDATA%\dblocal\run` (`icacls`). | Only the current user (plus SYSTEM/Administrators by inheritance) can read it. The code does not verify Windows ACLs (its owner and mode checks are Unix-only), so a different result here is a defect to report. | | |
| MSI install | Install `dblocal-<version>-windows-x86_64.msi` (per-user default). | Installs without admin; Start Menu shortcut created; app launches. | | |
| MSI uninstall keeps data | Uninstall MSI. | Package files removed; `%USERPROFILE%\.dblocal` remains. | | |
| SmartScreen first run | Download/run unsigned artifact from release. | SmartScreen "Windows protected your PC" / Unknown publisher may appear; verify checksum before proceeding; "More info" -> "Run anyway" works. | | |
| High DPI | Run on 125%/150%/200% scaling if available. | Layout readable; no clipped controls; tray/window icons crisp enough. | | |
| Clean shutdown/no leftovers | Quit app after starting/stopping engines. | No orphan dblocal/engine processes; ports released. | | |
| Autostart: enable writes the Run value (W4-D) | Run `dblocal.exe autostart enable`, then `reg query "HKCU\Software\Microsoft\Windows\CurrentVersion\Run" /v DBLocal`. | The value is present and quotes the installed exe path (`"C:\Program Files\DBLocal\dblocal.exe" --autostart`), including under `Program Files` (a space in the path). `dblocal.exe autostart status` prints `autostart: aktif`. | | |
| Autostart: disable removes the Run value (W4-D) | Run `dblocal.exe autostart disable`, then `reg query` the same key. | The `DBLocal` value is gone; `autostart status` prints `autostart: nonaktif`; running `disable` twice does not error. | | |
| Autostart: login opens to the tray (W4-D) | With the Run value enabled, sign out and back in (or run `dblocal.exe --autostart`). | No window appears; the app runs in the tray (W2-G now provides a real Windows tray). If the tray could not be created (restricted session), the process opens the window instead, or with a flagged instance keeps running headless; record which behaviour you saw and whether the status bar showed "Tray tidak tersedia". | | |
| Autostart: flagged instance starts at login (W4-D) | Flag one instance (`autostart`), sign out and in. | The instance reaches `running`; the tray dot turns green. If its port is taken by another process, it ends `failed` with `[DBL-1005]` and the other flagged instances still start. | | |
| Tray: icon appears in the notification area (W2-G) | Start the app normally, look at the notification area / overflow. | A DBLocal tray icon appears (a status dot: grey with nothing running, green with a running instance, red with a failed one). Hovering shows the "DBLocal" tooltip. | | |
| Tray: menu lists instances with the right dots (W2-G) | Right-click (or left-click) the tray icon. | The menu lists every instance grouped by engine, each with a coloured status dot (green running, grey stopped, red failed, amber busy); `Starting`/`Stopping` rows are greyed out. "Buka DBLocal" and "Keluar" are present. | | |
| Tray: Start/Stop from the menu starts/stops the engine (W2-G) | Create a stopped instance; choose it / its "Mulai" row from the tray menu; then stop it the same way. | The engine starts (status turns green within ~1 s, log shows startup) and stops again; the main window, if open, reflects the change in ≤ 1 s (F-TRY-01 AC2). | | |
| Tray: "Buka DBLocal" opens a window after the window was closed (W2-G) | Close the main window (tray stays), then choose "Buka DBLocal" from the tray. | A window opens attached to the same service (no second app instance); with the window already open it is focused instead. | | |
| Tray: "Keluar" with a running instance shows the quit dialog (W2-G) | Start one instance, then choose "Keluar" from the tray. | The quit dialog appears ("Biarkan tetap berjalan" is the default); cancelling leaves both the app and the engine running. Choosing "Hentikan semua lalu keluar" stops the engine and exits. | | |
| Tray: closing the window keeps the process (W2-G) | Click the window close button while the tray is present. | The window closes; the process, tray and any running engines stay alive (F-TRY-02 AC1). | | |
| Tray: quit removes the icon, no ghost icon (W2-G) | Quit the app from the tray, then inspect the notification area (including the overflow pane, and after hovering). | The tray icon disappears immediately and does not linger as a ghost that only clears on hover. No `dblocal.exe` process remains (`tasklist | findstr dblocal`). | | |
| Tray: no tray host degrades to AC3 (W2-G) | If you can reproduce a session with no shell tray area, start the app. | The status bar shows "Tray tidak tersedia" once with its note; closing the window goes through the quit dialog; nothing panics. (Otherwise record Blocked with the reason.) | | |
| Config: show and change a PostgreSQL setting (W4-C) | Create a PostgreSQL instance; `dblocal.exe config show <name>`, then `config set <name> max_connections=57 work_mem=8MB`, then stop/start the instance and `psql -c "show max_connections"`. | The file `data\postgresql.conf` keeps its comments and CRLF/LF endings, only the edited key changes (the new key is appended), the backup `%USERPROFILE%\.dblocal\dblocal-backups\<name>\postgresql.conf.<ts>.bak` exists, `show max_connections` says 57. The line editor is only proven on Linux; CRLF files and Windows paths are unverified. | | |
| Config: restore after a bad value (W4-C) | `config set <name> shared_buffers=99999GB` (the validator may accept what the engine refuses), start the instance, then `config restore <name>`. | The start fails with a log tail and the failure offers the restore; after `config restore` the instance starts again. | | |
| Config: open the raw file (W4-C) | `dblocal.exe config open <name>`. | The file opens in the default handler for `.conf`/`.cnf`. The launcher is `rundll32.exe url.dll,FileProtocolHandler <path>`; for an unknown extension Windows may show the "Open with" dialog (record which). If nothing opens, the CLI prints the path and `DBL-8608`. **Never run on Windows.** | | |
| MySQL database admin (W4-A3) | Create a MySQL 8.4.11 instance from the Windows artifact with a password. Then `dblocal.exe db create <name> app`, `db list <name>`, `user create <name> u1 --db app --level ro`, `user create <name> u2 --db app --level rw`, connect with each user's password and try `SELECT` (both) / `INSERT` (rw only) / `CREATE DATABASE` (neither), `password reset <name>`, then `db drop <name> app --confirm app`. | Every command works and admin dials loopback TCP on `127.0.0.1:<port>` (a unix socket does not exist on Windows; `mysql_async` would return `NamedPipesDisabled` for a socket path). A failure before W4-A3 was `[DBL-8100]`. A failure now would look like `[DBL-8100]` (port not bound: engine not running, or `bind-address` not `127.0.0.1`), `[DBL-8116]` (wrong stored password), or `[DBL-8110]` (the RSA exchange or a statement failed). Record the first failing step. **No Windows run exists.** | | |
| MariaDB database admin (W4-A3) | Same steps as the MySQL row, against a MariaDB 11.8.10 Windows artifact. The bootstrap sets the password with a plain `ALTER USER`, so the account may keep the default plugin; `mysql_async` accepts either it or a `mysql_native_password` switch. | Same expectations and failure codes as MySQL. If MariaDB's `root@localhost` account rejects a TCP login (`root@127.0.0.1` missing), the failure is `[DBL-8116]`; the adapters create the `127.0.0.1` account, so that would be a defect to report. **No Windows run exists.** | | |
| MySQL/MariaDB admin port binding (W4-A3) | With a MySQL and a MariaDB instance running on Windows, inspect the listening sockets with `netstat -ano | findstr <port>` (or `Get-NetTCPConnection -State Listen -LocalPort <port>` in PowerShell). | Each engine listens on `127.0.0.1:<port>` only, not `0.0.0.0` and not `::`; the adapters render `bind-address=127.0.0.1`. If it listens wider, record it as a security defect. **No Windows run exists.** | | |
| GUI: Databases tab (W4-B) | Create a PostgreSQL instance with a password, start it, open it in the GUI, tab Databases. Create `appdb`, create user `u1` Read-only on it, drop `appdb` by typing its name, reset the superuser password. | The list shows `postgres` right after the start; `appdb` appears, the drop dialog takes the keyboard (typing goes into its field) and Enter drops only when the name matches; the generated password is shown once and is gone after the dialog closes; after the reset the new URI works and the old password does not. Verified only on Linux/Xwayland; Windows rendering, focus and the Credential Manager are unverified. | | |
| GUI: Open in… menu (W4-B) | Header and Overview "Open in…" with and without a database client installed (for example DBeaver, HeidiSQL, psql in `PATH`). | Detected clients are listed (GUI and CLI groups) and open the instance; with none detected the menu offers "Salin URI". The Windows launchers were never run. | | |
| GUI: Settings, login item (W4-B) | Gear in the title bar, "Jalankan saat login", toggle on, sign out and in, toggle off. | The switch follows the real state (the Registry Run key); a failed enable shows the typed error and stays off, a failed disable stays on. Linux unit path only was seen on screen; Registry and LaunchAgent are unverified. | | |
| GUI: Config tab (W4-H) | Create a PostgreSQL instance, open it, tab Config. Change one value (for example `max_connections`), press Terapkan, then "Restart sekarang"; change another to a bad value and apply; use "Buka file konfigurasi"; resize the window to 900 and 1280 px wide. | The grid fits at both widths; a valid change shows "restart required" and the restart applies it; a bad value shows the engine's refusal and the previous file is restored (backup kept); "Buka file konfigurasi" opens `postgresql.conf` in the default `.conf` handler. Linux only was seen on screen; the Windows editor launch, the real restart and MySQL/MariaDB flows were never run there. | | |
| GUI: keyboard-only first session (W4-G) | Without touching the mouse: start the app, Ctrl+N, choose engine and version with Tab and Space/Enter, Tab to Create and press Enter, then Ctrl+K, type `mulai`, Enter, Ctrl+K, type `salin`, Enter, paste somewhere. Repeat with Ctrl+Shift+S, Ctrl+Shift+X and Ctrl+Enter on a selected instance. | Every step works with no click, including the very first Ctrl+K/Ctrl+N on a fresh window; the clipboard holds the connection URI; the keyboard still answers right after every dialog closes. Linux (private Xvfb, no window manager) only; a real Windows window manager and focus-on-activate were never exercised. | | |
| GUI: focus ring on every control (W4-G) | Tab through the main screen (header, sidebar filter, group header, instance rows, instance tab and its close button, Overview/Logs/Databases/Config tabs with the arrow keys, buttons), then through the create dialog, Settings and the palette. Do it in the light and the dark theme. | Every stop shows a clearly visible ring (a full outline, not only a tint), Tab never leaves an open dialog or the palette, Shift+Tab walks back. The ring colour is the theme `ring` token (3:1 on every surface by the contrast test); how Windows renders it, and in Windows high-contrast mode, is unverified. | | |
| GUI: delete an instance (W4-I) | Create two instances. On the first press its header "Hapus" (Tab to it, Enter), keep "Hapus instance, simpan data" and confirm. On the second (running) open Ctrl+K, type `hapus`, Enter, choose "Hapus instance & data" with Space, type its exact name, confirm. Then Ctrl+K again. | The first disappears from the sidebar and from `dblocal list` and its data directory is under `archive\` (named with a numeric stamp, see PLAN W4-I), its Credential Manager entry is gone; the second stops its processes first and its data directory is removed; a wrong name keeps the confirm disabled; the keyboard answers right after each confirm. Linux (private Xvfb) only; Windows folder move, process stop and Credential Manager removal never ran there. | | |
| GUI: system theme following (W4-G) | Switch Windows between light and dark while the app runs, then start it with the theme set the other way; set `DBLOCAL_THEME=dark` once. | The app follows the system theme live and at start; `DBLOCAL_THEME` overrides it. The live change cannot be simulated by the headless test platform, so this row is the only check of it. | | |
| GUI: screen reader, high-contrast, IME (W4-G) | With Narrator (and NVDA if installed) walk the create dialog, the palette and an instance's Overview; turn on a Windows high-contrast theme; type a non-Latin name with an IME in the Filter and Nama fields. | Narrator announces control names and each instance status as text ("berjalan", "berhenti"), never colour alone; high-contrast keeps text and rings visible; the IME composes in the fields. Never run on Windows (nor VoiceOver on macOS, nor Orca on Linux). | | |
| Self-update: install kind | Run the update check from the installed MSI build and from a portable zip (when the GUI/CLI exposes it; until then the library `dblocal_core::update` is covered only by Linux tests). | The MSI install is detected as managed by the installer (update offered through a new MSI, not an in-place swap) or as portable, exactly as `docs/architecture/update.md` states; a build tree is detected as `build tree` and updates are disabled. | | |
| Self-update: swap helper | Only after the update engine is wired into the app: apply an update on a portable install. | The running `dblocal.exe` is replaced through the helper plan (rename away, new file in place, relaunch); a failed smoke run rolls back to the old exe; no leftover `.old` after the next start. | | |

## SmartScreen / unsigned artifact checks

Before bypassing SmartScreen:

```powershell
Get-FileHash .\dblocal-<version>-windows-x86_64.zip -Algorithm SHA256
# Compare with checksums.txt from the same GitHub release.
```

Expected: exact hash match. If mismatch, delete file and do not run.

If the MSI is unsigned, expect SmartScreen/Unknown publisher warnings. Do not disable SmartScreen
globally. Verify checksum/provenance, then use "More info" -> "Run anyway" only for the verified
file.

## MSI notes

- Default install scope is **per-user** (no elevation).
- Optional per-machine install: `msiexec /i dblocal-<version>-windows-x86_64.msi ALLUSERS=1 MSIINSTALLPERUSER=""` (requires elevation).
- Uninstall must keep `%USERPROFILE%\.dblocal`.
- The MSI built by CI is unsigned unless signing is added later; document the exact warning text you saw.

## Run in a Windows 10 x64 VM (diuji di VM, not bare metal)

A reviewer ran the **CLI** (no GUI, no tray, no MSI) in a VirtualBox VM `dblocal-win64`
(Windows 10 22H2 x64, built inside the VM with Rust stable + VS 2022 Build Tools + NASM, home
`C:\dbl\home`). This is **not** the dual-boot bare-metal check: it does not replace the matrix
above, it only means the first Windows build was exercised once before the owner's run and that
the defects it exposed are already fixed on `main`.

### Defects the VM run found (all Windows-only, none visible from Linux, all fixed with tests)

| Commit | Defect | Symptom in the VM |
|---|---|---|
| `b07f911` | `mariadb/identity.rs` non-unix arm built an `Unsupported` error with a field that no longer exists | `cargo build` failed on MSVC |
| `c81c217` | `gui/src/vulkan.rs` used `wgpu` on every OS but `wgpu` is a Linux/BSD-only dependency | GUI crate did not compile on Windows |
| `a92f35e` | install check looked for `bin/postgres` (no `.exe`) | no engine could be installed (`DBL-4001`) |
| `fb58d8a` | `pg_ctl start -l <log>` while the supervisor already redirects the log | `pg_ctl` sharing violation, start failed |
| `1a83f89` | PostgreSQL unix-socket plan on Windows (`C:\...` mangled in `postgresql.conf`) | server exited at start |
| `7532782` | MySQL `my.cnf` paths with backslashes (`\r`, `\s`, `\t`, `\n` are escapes in option files) | `mysqld` could not start the bootstrap |
| `2a419c6` | MariaDB: Windows zip has no `scripts/`, the tool is `bin/mariadb-install-db.exe`, takes only `--datadir/--silent`, refuses a non-empty datadir | create failed at `initializing` |
| `e28d82d` | MariaDB bootstrap: `--skip-networking` alone aborts `mariadbd.exe`; it also exits 1 after the init-file `SHUTDOWN` although the password change is applied | create failed at `initializing` |
| `9b84d4f` | MySQL bootstrap passed `--daemonize=OFF` (unknown variable on Windows) | `mysqld.exe` aborted |
| `9baa592` | `stop` signalled right after the shutdown request; Windows `terminate` is `TerminateProcess` and `mysqladmin shutdown` returns before the process exits | `stop` said "Siap" after 340 ms while `mysqld.exe` ran another second; an immediate `delete` failed on locked files (`DBL-3001`, row stuck in `deleting`) |

### What ran green in the VM (CLI, labelled "diuji di VM")

| Flow | Result |
|---|---|
| PostgreSQL 18.7.0: `create` (download, verify, extract, initdb), `start`, `status`, listener on `127.0.0.1` only, `uri`, `db create/list`, `config show/set` (+ backup file), `stop`, `start`, `logs`, `autostart enable/disable` (HKCU Run value written and removed; second `disable` no error), `delete` (archive), no `postgres.exe` left after `stop` | all exit 0 |
| MariaDB 11.8.10: `create`, `start`, `status`, listener `127.0.0.1:<port>` only, `db create/list`, `user create` (ro, rw), `logs`, `stop` (no `mariadbd.exe` left), `delete --purge` | all exit 0 |
| MySQL 8.4.11: same flow | all exit 0 |

### Still **not** covered by the VM run

- GUI window, tray, quit dialog, high DPI, Credential Manager (credential rows), MSI, SmartScreen, `cmd.exe`/Windows Terminal launchers, real login autostart (only the Registry value was checked), `config restore`/`config open`, `db drop`, `password reset`, Valkey (no Windows artifact), `rename`/symlink/ACL behaviour.
- MariaDB on Windows keeps the empty `test` database created by `mariadb-install-db.exe` (the Unix path passes `--skip-test-db`, which the Windows tool does not have).
- MariaDB's server log shows `Aborted connection ... Got an error reading communication packets` warnings after admin calls (the admin client closes without a clean `COM_QUIT`; harmless, not seen as a failure).
- Cosmetic: `autostart status` prints the location as `C:\Users\<user>\HKCU\Software\...\Run#DBLocal`; a detached engine inherits the CLI's stdout pipe, so `dblocal start x | ...` in a pipeline does not reach EOF while the engine runs.
- A VM has no GPU: rendering here would not say anything about Vulkan/DirectX on the real machine.

## What to report back to the coordinator

Paste back:

1. Windows version/build, CPU arch, GPU model/driver, Vulkan status.
2. Exact commands run (`cargo xtask ci`, `cargo run -p dblocal-cli -- --help`, MSI install/uninstall commands) and key output.
3. The completed manual verification table with Outcome and Notes.
4. Any failures with repro steps, logs/screenshots textually described, and whether data was preserved.
5. Whether SmartScreen appeared, exact prompt text, and checksum verification result.
6. Explicit statement of anything not tested (e.g., Valkey unavailable, no high-DPI display, MSI not built).

Do **not** include passwords, secret values, or the contents of `%USERPROFILE%\.dblocal`.

## macOS (no macOS machine available: unverified)

This task cannot verify macOS. The owner/coordinator should verify separately on macOS:

- `cargo xtask ci` passes on macOS.
- `cargo run -p dblocal-cli` opens window; tray behaves correctly.
- Keychain entries are created/removed on credential lifecycle.
- `.dmg` install/launch works; signed+notarized artifact passes Gatekeeper/spctl when secrets configured.
- Real PostgreSQL and Valkey start/stop on localhost ports; uninstall keeps `~/.dblocal`.

### macOS tray (W2-G: `tray-icon` + `muda`; **no macOS machine here**)

Every row is unverified. The macOS backend is the same `tray/desktop.rs` as Windows, on the
AppKit main thread; the compile-only verification and its limits are in `docs/architecture/gui.md`.

| Area | Steps | Expected result | Outcome | Notes |
|---|---|---|---|---|
| Menu bar icon appears | Launch the app; look at the menu bar (status area). | A DBLocal status-dot icon appears in the menu bar (template image, recoloured for light/dark). **no macOS machine here.** | | |
| Menu lists instances with the right dots | Click the menu bar icon. | The menu lists instances grouped by engine with status dots; busy rows disabled; "Buka DBLocal" and "Keluar" present. **no macOS machine here.** | | |
| Start/Stop from the menu | Choose a stopped instance's start row, then stop it. | The engine starts/stops; the window reflects it in ≤ 1 s. **no macOS machine here.** | | |
| "Buka DBLocal" opens a window after close | Close the window, then choose "Buka DBLocal". | A window opens (or the existing one is focused); no second app instance. **no macOS machine here.** | | |
| "Keluar" with a running instance shows the quit dialog | Start an instance, choose "Keluar". | The quit dialog appears with "Biarkan tetap berjalan" as default. **no macOS machine here.** | | |
| Closing the window keeps the process | Click the window close button. | The window closes; the process, menu bar icon and engines stay (AppKit accessory/regular policy per the app). **no macOS machine here.** | | |
| Quit removes the icon, no ghost icon | Quit, then inspect the menu bar. | The status item is removed immediately (`removeStatusItem`); no ghost. **no macOS machine here.** | | |
| Fullscreen-app quirk | Open a fullscreen app, then create the tray (launch DBLocal). | The tray is created after launch (`did_finish_launching`), which `tray-icon` documents as required to avoid fullscreen issues. **no macOS machine here.** | | |

Until run on macOS hardware, macOS behavior remains **unverified**.
