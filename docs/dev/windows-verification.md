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
| MySQL/MariaDB database admin (W4-A2/W4-A3) | Create a MySQL and a MariaDB instance (Windows artifacts), then `dblocal.exe db create <name> app`, `db list`, `user create <name> u1 --db app --level ro`, `password reset <name>`, `db drop <name> app --confirm app`. | Every command works. Admin connects over loopback TCP on Windows (a unix socket does not exist); before W4-A3 it fails with `DBL-8100`. Record the first failing step if any. **No Windows run exists.** | | |
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
