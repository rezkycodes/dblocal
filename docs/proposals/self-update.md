# Proposal: In-App Self-Update (Stage 1: Core Engine + CLI)

## 1. Investigation Summary: What Already Exists

Before designing, we audited the existing codebase (`v0.1.3`, commit `04717bd`):

- **`crates/core/src/update/`** already implements a foundational self-update module:
  - `check.rs`: Queries `https://api.github.com/repos/<owner>/<name>/releases?per_page=20` via `UpdateFetcher` (`ReqwestUpdateFetcher`), enforces HTTPS-only with redirect allowlist (`api.github.com`), filters out `draft` and `prerelease` on `ReleaseChannel::Stable`, compares strict semver without downgrade, and sanitizes/bounds release notes.
  - `download.rs`: Downloads `checksums.txt` and the selected asset with `ALLOWED_HOSTS` (`github.com`, `objects.githubusercontent.com`, `github-releases.githubusercontent.com`, `release-assets.githubusercontent.com`), 65 MiB size cap, connect/read timeouts, and `User-Agent: dblocal/<version>`.
  - `verify.rs`: Parses `checksums.txt` (`<sha256>  <filename>`) and verifies SHA-256 in constant time, deleting the staged file on mismatch.
  - `kind.rs`: Detects `InstallKind` (`AppImage`, `PortableTarball`, `WindowsZip`, `WindowsMsi`, `MacApp`, `DistPackage`, `Homebrew`, `Unknown`) via `$APPIMAGE`/`$APPDIR`, `dpkg-query -S` / `rpm -qf`, and path rules.
  - `install.rs`: Builds an `InstallPlan`, copies staged binary next to target, fsyncs, creates `<exe>.old` backup, atomically renames over target, runs `--version` smoke check, and rolls back on failure.
  - `error.rs`: Defines `UpdateError` with stable `DBL-8001`..`DBL-8011` codes.
- **`crates/core/src/service/mod.rs` & `crates/core/src/instance/update_ops.rs`**:
  - `DblService::check_update(&self) -> ServiceResult<UpdateStatus>`
  - `DblService::apply_update(&self) -> ServiceResult<()>`
  - `ServiceEvent::UpdateChecked { version, can_self_update }`
  - `LocalService` wires `ReqwestUpdateFetcher` and `DownloadInstaller`.
- **`crates/core/src/store/`**:
  - SQLite `settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)` table with `Store::get_setting` and `Store::set_setting` already exists.
- **`crates/core/src/download/`**:
  - `CancellationToken` and `space::check_disk_space` already exist and can be reused directly.
- **`crates/cli/src/commands/update.rs`**:
  - `dblocal update check [--json]` and `dblocal update apply [--yes]` already exist.
- **`packaging/install.sh` and `.github/workflows/release.yml`**:
  - Linux tarball (`dblocal-<tag>-linux-<arch>.tar.gz`) contains `dblocal`, `share/applications/dev.dblocal.DBLocal.desktop`, `share/icons/hicolor/scalable/apps/dblocal.svg`, and `share/icons/hicolor/{16,24,32,48,64,128,256,512}x{size}/apps/dblocal.png`. `install.sh` installs `dblocal` into `~/.local/bin` (or `--prefix`), rewrites `Exec=` and `TryExec=` in the `.desktop` file to the absolute binary path, and installs desktop + icons under `$XDG_DATA_HOME` (default `$HOME/.local/share`).
  - Linux AppImage: `dblocal-<tag>-linux-<arch>.AppImage`.
  - Linux distro packages: `dblocal_<ver>_<arch>.deb`, `dblocal-<ver>-1.<arch>.rpm`, installed under `/usr/bin/dblocal`.
  - Windows portable zip: `dblocal-<tag>-windows-x86_64.zip` containing `dblocal.exe`.
  - Windows MSI: `dblocal-<ver>-windows-x86_64.msi`, per-user by default (`%LOCALAPPDATA%\DBLocal\dblocal.exe`, `UpgradeCode="A3D5E2D1-7B4A-4D7E-9F3A-8C2B1D4E5F60"`, `MajorUpgrade`).
  - Checksums: `checksums.txt` with bare filenames.

### Gaps Found in Existing Implementation

1. **Asset selection ignored `InstallKind`**: `select_platform_asset` picked `AppImage` over `TarGz` on Linux and `Zip` over `Msi` on Windows regardless of how DBLocal was installed.
2. **`DownloadInstaller` did not extract archives**: It copied the downloaded `.tar.gz` directly over `current_exe` instead of extracting `dblocal`, desktop entry, and icons.
3. **No release page URL or structured refusal reason** in `AvailableUpdate` / `UpdateStatus` (needed by GUI and package-manager hints).
4. **No update check caching or auto-check setting**: Repeated checks hit GitHub's unauthenticated rate limit (60 req/hr); no 6-hour cache or `update.auto_check` setting.
5. **No progress events, cancellation, disk-space preflight, or concurrent-update lock** during apply.
6. **No pre-swap binary verification**: `smoke_check` ran only after replacing the target binary instead of verifying the extracted candidate before swapping.
7. **Windows MSI (per-user) & Windows Zip apply**: `WindowsMsi` was marked non-updatable even for per-user `%LOCALAPPDATA%\DBLocal` installs, and `MacApp` was incorrectly marked self-updatable despite macOS being deferred. The `--apply-update` helper mode was planned in `windows_helper_command` but not wired in `crates/cli`.
8. **Verification was not behind a trait seam** for future Ed25519 signature verification over `checksums.txt`.

---

## 2. Architecture & Design Decisions

### 2.1 Check, Caching & Auto-Check Setting

- **Source**: Fixed repository `rezkycodes/dblocal` over HTTPS (`api.github.com`), `User-Agent: dblocal/<version>`, no telemetry. Drafts and pre-releases are ignored on `ReleaseChannel::Stable`; strict semver comparison never downgrades.
- **Asset matching by `InstallKind`**:
  - `PortableTarball` -> `.tar.gz`
  - `AppImage` -> `.AppImage`
  - `WindowsZip` -> `.zip`
  - `WindowsMsi` (per-user) -> `.msi`
  - `DistPackage { Dpkg }` -> `.deb` (reported as matching asset name + release page URL; `can_self_update = false`)
  - `DistPackage { Rpm }` -> `.rpm` (reported as matching asset name + release page URL; `can_self_update = false`)
  - `MacApp`, `Homebrew`, `Unknown` -> fallback platform asset for display; `can_self_update = false` with explicit reason.
- **Caching (6 hours)**: Stored in SQLite `settings` table (`update.auto_check`, `update.last_checked_at`, `update.cached_release`).
  - `check_update()` (manual): forces a fresh check and updates the cache.
  - `check_update_if_due()` (startup & periodic every 6 hours):
    - Returns `Ok(None)` if `update.auto_check` is `"false"`.
    - Returns the cached `UpdateStatus` without network if `< 6 hours` old and matching `current_version` + `install_kind` (and emits `ServiceEvent::UpdateChecked`).
    - Otherwise queries GitHub; on `Network` or `RateLimited`, logs at debug/info and returns `Ok(None)` quietly (no error popup).
  - CLI commands never trigger unsolicited background checks. The GUI (Stage 2) invokes `check_update_if_due()` at startup and on a 6-hour timer.

### 2.2 Install-Kind Detection & Behaviour

| Install Kind | Detection | `can_self_update` | Apply Strategy |
|---|---|---|---|
| **Linux per-user tarball** (`PortableTarball`) | Executable named `dblocal` in a user-writable directory not under system/build prefixes | `true` | Download `.tar.gz`, verify SHA-256, safely extract allowlisted members into fresh private dir on target filesystem, run `--version` smoke check before swap, back up to `dblocal.old`, atomically rename binary, atomically update `.desktop` (`Exec=`/`TryExec=` rewritten) and hicolor icons under `$XDG_DATA_HOME` / `~/.local/share`, rollback on failure. |
| **Linux AppImage** (`AppImage`) | `$APPIMAGE` file + `$APPDIR` containing `current_exe` | `true` | Download `.AppImage`, verify SHA-256, stage in private dir on target filesystem, `chmod` from target, smoke check `--version` before swap, back up to `<appimage>.old`, atomically rename over `$APPIMAGE`, rollback on failure. |
| **Linux `.deb` / `.rpm`** (`DistPackage`) | `dpkg-query -S` or `rpm -qf` owns `exe_path` | `false` | Never modify, never elevate. Report "managed by the package manager", release page URL (`https://github.com/rezkycodes/dblocal/releases/tag/<tag>`), and matching `.deb`/`.rpm` asset name. |
| **Windows per-user MSI** (`WindowsMsi`) | Path under `\AppData\Local\DBLocal\` (writable without elevation) | `true` | Download & verify `.msi` into staging dir. On restart, spawn detached `dblocal.exe --apply-update --pid <pid> --msi <staged_msi> --relaunch <exe>` which waits for the app to exit, runs `msiexec.exe /i <staged_msi> /passive`, and relaunches `dblocal.exe`. |
| **Windows per-machine MSI** (`Unknown` / managed) | Path under `\Program Files\` or `\Program Files (x86)\` | `false` | Never elevate (no UAC). Report that per-machine installs require running the MSI installer manually, with release URL and `.msi` asset name. |
| **Windows portable zip** (`WindowsZip`) | Other writable Windows paths (not dev build) | `true` | Download & verify `.zip`, safely extract `dblocal.exe` into fresh private dir, smoke check `--version`, rename running `dblocal.exe` -> `dblocal.exe.old` and rename new `dblocal.exe` into place (or fall back to `--apply-update` helper after exit if locked). |
| **macOS** (`MacApp`) | Inside `*.app` bundle on macOS | `false` | Deferred; report that macOS self-update is not supported yet and link to the release page. |
| **Dev builds / Homebrew / Unknown / read-only** | `target/debug`, `target/release`, `dblocal-target*`, `.cargo/bin`, `/usr/`, `/opt/`, `/nix/store/`, `/snap/`, `/app/`, Homebrew, read-only dir | `false` | Refuse with clear reason explaining why self-update is unavailable. |

### 2.3 Running Database Instances & Restart

- Engine processes (PostgreSQL, MySQL, MariaDB, Valkey) are spawned **detached** via `crates/core/src/process/spawn.rs` (`setsid` on Unix, `CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS` on Windows).
- Replacing the `dblocal` binary on disk and exiting/restarting DBLocal **never signals, stops, or touches any running database server** or `~/.dblocal/instances/*` data.
- On Linux, `rename(2)` replaces the directory entry while the old binary stays mapped in memory; `dblocal` exits cleanly and spawns the new binary detached.
- On startup of the new `dblocal` process:
  1. `LocalService::new` runs `reconcile_on_startup`, which probes each instance's persisted `(pid, process_started_at, process_exe)` and adopts live database servers back into `Running` state without signalling them.
  2. Any leftover `<exe>.old` backup from a successful update can be cleaned up via `cleanup_backup`.
- Login items (`autostart`) point at the installed binary path; because per-user tarball and AppImage replace the file in place at the same path, autostart continues working without re-registration.

### 2.4 Security & Robustness

- **HTTPS-only & allowlisted hosts**: Only `api.github.com` for release metadata and `github.com`, `objects.githubusercontent.com`, `github-releases.githubusercontent.com`, `release-assets.githubusercontent.com` for assets and redirects (max 5 redirects).
- **Test overrides compiled out in release**: Any loopback/base-URL override is gated behind `#[cfg(any(test, debug_assertions))]` and unavailable in `--release` builds.
- **Verification seam (`ReleaseVerifier`)**:
  - `verify_manifest(&self, checksums_bytes: &[u8], signature_bytes: Option<&[u8]>) -> UpdateResult<Checksums>`
  - `verify_artifact(&self, path: &Path, expected_hex: &str, label: &str) -> ...`
  - Current production implementation: `Sha256ReleaseVerifier`.
  - **Future Ed25519 signature support**: When signing is enabled later, CI (`release.yml`) will sign `checksums.txt` with an offline/KMS Ed25519 signing key and upload `checksums.txt.sig` alongside `checksums.txt`. The public verifying key(s) (supporting key rotation with a key ID or multiple trusted keys) will be embedded in `crates/core/src/update/verify.rs`, and `Ed25519ReleaseVerifier` will implement `ReleaseVerifier::verify_manifest` to require and verify `checksums.txt.sig` before parsing any SHA-256 digests. No callers above `ReleaseVerifier` need to change.
- **Safe archive extraction**: Strict entry allowlist for Linux `.tar.gz` (`dblocal`, `share/applications/dev.dblocal.DBLocal.desktop`, `share/icons/hicolor/scalable/apps/dblocal.svg`, `share/icons/hicolor/{16,24,32,48,64,128,256,512}x{size}/apps/dblocal.png`, and their parent directories) and Windows `.zip` (`dblocal.exe`). Rejects `..`, absolute paths, symlinks, hard links, special files, unexpected entries, and oversized extractions. Extracts into a fresh private (`0700`) temp directory on the same filesystem as the target so `rename(2)` is atomic.
- **Exclusive lock**: A lockfile (`~/.dblocal/update/update.lock` via `O_CREAT | O_EXCL` with stale-PID detection + in-process atomic guard) guarantees only one update runs at a time across GUI and CLI processes.
- **Disk space preflight**: Reuses `crate::download::space::check_disk_space` (`asset_size * 3`) before downloading.
- **Partial download cleanup**: Every failed or cancelled attempt removes its private staging directory.

### 2.5 GUI-Facing API (for Stage 2)

- **Types** (`dblocal_core::service` & `dblocal_core::update`):
  - `UpdateStatus`: `current`, `available: Option<AvailableUpdateView>`, `install_kind`, `package_manager_hint: Option<String>`, `cannot_update_reason: Option<String>`.
  - `AvailableUpdateView`: `version`, `tag`, `release_notes`, `release_url`, `asset_name`, `size_bytes`, `can_self_update`.
  - `UpdateProgress`: `Downloading { received_bytes, total_bytes }`, `Verifying`, `Installing`, `ReadyToRestart { version, restart_target }`, `Cancelled`, `Failed { code, message }`.
  - `ServiceEvent::UpdateProgress { id: OperationId, progress: UpdateProgress }`.
- **Methods on `DblService`**:
  - `check_update(&self) -> ServiceResult<UpdateStatus>` (manual, bypasses cache)
  - `check_update_if_due(&self) -> ServiceResult<Option<UpdateStatus>>` (startup/6h periodic, quiet on offline/rate-limit)
  - `auto_update_check_enabled(&self) -> Result<bool>`
  - `set_auto_update_check(&self, enabled: bool) -> Result<()>`
  - `start_update(&self) -> ServiceResult<OperationId>` (async, emits `ServiceEvent::UpdateProgress` and `ServiceEvent::OperationFinished`, cancellable via `cancel_operation(&op_id)`)
  - `apply_update(&self) -> ServiceResult<()>` (synchronous wrapper for compatibility)
  - `restart_for_update(&self) -> ServiceResult<()>` (spawns updated binary / Windows helper detached so GUI can exit cleanly)
