# Toolchains (PHP, Composer, and Node.js)

DBLocal installs and switches **PHP**, **Composer**, and **Node.js** (`node`, `npm`, `npx`) per user under `~/.dblocal/toolchains/`, without containers and without `sudo`. Active versions are exposed to any terminal through lightweight launchers (**shims**) in `~/.dblocal/bin/`.

Toolchain state lives in its own JSON file (`~/.dblocal/toolchains/active.json`), completely separate from `state.db`, so installing or using toolchains never migrates the instance database and downgrading to v0.2.x remains safe.

## Why `dblocal toolchain ...`?

`dblocal list` already lists database instances. To keep instance commands and runtime-language commands unambiguous, all toolchain commands live under the `toolchain` subcommand:

```sh
dblocal toolchain list [php|node|composer] [--installed] [--json]
dblocal toolchain install <tool> <spec> [--profile common|bulk] [--no-activate] [--yes]
dblocal toolchain use <tool> <spec>
dblocal toolchain remove <tool> <spec> [--yes]
dblocal toolchain which <tool>
dblocal toolchain path [--print | --apply [--yes] | --remove]
dblocal toolchain doctor [--json]
dblocal toolchain php-settings <get|set <key> <value>|reset|open> [--json]
```

## Quick start

```sh
# 1. Inspect available versions for your platform
dblocal toolchain list

# 2. Install PHP 8.4 (common profile on Linux), Composer, and Node 24
dblocal toolchain install php 8.4 --yes
dblocal toolchain install composer latest --yes
dblocal toolchain install node 24 --yes

# 3. Preview and opt in to adding ~/.dblocal/bin to PATH
dblocal toolchain path --print
dblocal toolchain path --apply

# 4. Open a new shell (or source your rc file) and verify
dblocal toolchain doctor
php -v
composer --version
node -v
npm -v
```

### Activation and switching

- **First install activates automatically**: installing a tool when no version of that tool is active yet activates it and generates its shim(s) (pass `--no-activate` to skip this).
- **Additional installs do not switch**: installing PHP 8.3 when PHP 8.4 is already active leaves PHP 8.4 active until you run `dblocal toolchain use php 8.3`.
- **Removing the active version**: clears the active entry and removes the shim. If other versions of that tool remain installed, DBLocal does **not** auto-switch; it tells you to pick one with `dblocal toolchain use`.
- **Removing while in use**: DBLocal refuses to remove a version if a running process started from that install directory is detected.
- **Composer and PHP**: the `composer` shim invokes `composer.phar` through the sibling `php` shim. Switching PHP immediately applies to Composer without regenerating the `composer` shim. If no PHP is active, `composer` prints a clear error and exits non-zero.

## PATH management and consent

DBLocal **never** edits your shell startup files or Windows environment automatically.

- `dblocal toolchain path` reports whether the managed block is applied and prints the shims directory.
- `dblocal toolchain path --print` prints the exact shell lines without modifying anything.
- `dblocal toolchain path --apply` shows the exact lines, asks for confirmation (or pass `--yes`), writes a `.dblocal-backup` copy of each existing startup file (`~/.profile`, `~/.bashrc`, `~/.zshrc`, `~/.config/fish/conf.d/dblocal.fish`), and appends an idempotent managed block delimited by `# >>> dblocal toolchains >>>` and `# <<< dblocal toolchains <<<`.
- `dblocal toolchain path --remove` removes the managed block and restores each file byte-for-byte.

Read-only or unreadable rc files are reported in the output and never clobbered.

### Diagnostics (`doctor`)

`dblocal toolchain doctor` (add `--json` for machine output) checks for:

1. `~/.dblocal/bin` missing from `PATH`.
2. `~/.dblocal/bin` present on `PATH`, but not first.
3. Another `php` or `node` binary earlier on `PATH` shadowing the DBLocal shim (names the exact path).
4. A shim pointing at a removed install directory.
5. A managed PATH block referencing a stale shims directory.

## PHP profiles (Linux)

On Linux `x86_64`, PHP binaries come from [static-php-cli](https://dl.static-php.dev) in two profiles for series `8.0` through `8.5` (PHP 7.4 is not published for Linux):

| Profile | Default | Intended use | Key extensions |
|---|---|---|---|
| `common` | Yes | Composer, Laravel, general CLI | `pdo_mysql`, `pdo_pgsql`*, `pdo_sqlite`, `mbstring`, `openssl`, `curl`, `ctype`, `tokenizer`, `fileinfo`, `filter`, `iconv`, `phar`, `zlib`, `opcache` (8.5+)* |
| `bulk` | No | Apps needing `intl`, `sodium`, `opcache`, `mysqli` | Adds `intl`, `sodium`, `opcache`, `mysqli`; omits `pdo_pgsql` and `pdo_sqlite` |

\* Observed upstream deviations:
- PHP `8.0.30` (`common`) does **not** include `pdo_pgsql` (series `8.1`–`8.5` `common` all include `pdo_pgsql`).
- PHP `8.5.8` (`common`) already includes `opcache` (built into PHP 8.5 core).

Select a profile explicitly with `--profile bulk`:

```sh
dblocal toolchain install php 8.4 --profile bulk --yes
```

## PHP settings

DBLocal manages six global PHP settings applied to whichever PHP version is active:

| Setting | Default | Example values |
|---|---|---|
| `memory_limit` | `512M` | `256M`, `1G`, `-1` |
| `upload_max_filesize` | `64M` | `32M`, `128M` |
| `post_max_size` | `64M` | `64M`, `128M` |
| `max_execution_time` | `120` | `0`, `30`, `300` |
| `date.timezone` | `UTC` | `Asia/Jakarta`, `Europe/London` |
| `display_errors` | `On` | `on`, `off`, `true`, `false`, `1`, `0` |

Settings are validated (rejecting newlines, quotes, `;`, and invalid timezone names) and written atomically to `~/.dblocal/toolchains/php/conf.d/99-dblocal.ini`. The `php` shim exports `PHP_INI_SCAN_DIR` pointing at that `conf.d` directory.

```sh
dblocal toolchain php-settings get
dblocal toolchain php-settings get --json
dblocal toolchain php-settings set memory_limit 1G
dblocal toolchain php-settings set date.timezone Asia/Jakarta
dblocal toolchain php-settings reset
dblocal toolchain php-settings open   # prints the managed conf.d and ini paths
```

## Windows notes

- **PHP**: official NTS `x64` zips from `windows.php.net` (no profile; series `7.4`–`8.6`). Because official Windows zips ship without a `php.ini`, DBLocal writes a managed base `php.ini` in the install directory enabling `curl`, `fileinfo`, `intl`, `mbstring`, `opcache`, `openssl`, `pdo_mysql`, `pdo_pgsql`, `pdo_sqlite`, `sodium`, and `zip`.
- **Visual C++ Redistributable**: official Windows PHP builds require the Microsoft Visual C++ 2015–2022 Redistributable (x64). DBLocal does not bundle it; install it from Microsoft if `php.exe` reports a missing `VCRUNTIME140.dll`.
- **Shims**: generated as `.cmd` wrappers in `%USERPROFILE%\.dblocal\bin\` forwarding `%*` and propagating `%ERRORLEVEL%`.
- **PATH**: `dblocal toolchain path --apply` prepends `%USERPROFILE%\.dblocal\bin` to the user `Path` under `HKCU\Environment` (preserving `REG_EXPAND_SZ` / `REG_SZ`) and broadcasts `WM_SETTINGCHANGE`. `--remove` removes the entry cleanly.

## Trust model and security

- **HTTPS only**: all catalog downloads and artifact URLs must use `https://` (TLS via `rustls`).
- **Mandatory SHA-256 verification**: every downloaded byte is verified against the catalog's SHA-256 digest **before** extraction or setting executable permissions. There is no skip flag.
  - **`hash_origin: "upstream"`** — Node.js (`SHASUMS256.txt` from `nodejs.org`), Windows PHP (`releases.json` from `windows.php.net`), and Composer (`getcomposer.org`).
  - **`hash_origin: "pinned"`** — Linux PHP (`dl.static-php.dev` publishes no upstream checksums, so the catalog generator downloads each tarball once, verifies `php -n -v` / `php -n -m`, and pins its SHA-256).
- **Safe extraction**: enforces download and extracted-size caps; rejects absolute paths, `..` traversal (tar-slip / zip-slip), symlink escapes, device/FIFO entries, and setuid/setgid bits.
- **Atomic installs**: archives extract into a staging directory under `~/.dblocal/toolchains/.tmp` and rename into place; failed or interrupted installs leave no partial version behind.
- **Safe shim removal**: every generated shim carries a DBLocal marker comment; DBLocal never deletes a file in `~/.dblocal/bin` that it did not create.

## Uninstalling toolchains

1. Remove the managed PATH entry:
   ```sh
   dblocal toolchain path --remove
   ```
2. Remove individual versions with `dblocal toolchain remove <tool> <version> --yes`, or delete `~/.dblocal/toolchains/` and any DBLocal-generated shims in `~/.dblocal/bin/`.

Your database instances in `~/.dblocal/instances/` and `state.db` are unaffected.

## Desktop GUI

In the DBLocal desktop app, toolchains live in the **TOOLCHAINS** section of the left sidebar (between **INSTANCES** and **RUNTIMES**), listing **PHP**, **Node.js**, and **Composer** with their current active version (or `Not installed` / `Inactive`).

### Version pages

Selecting **PHP**, **Node.js**, or **Composer** opens its version page:

- **Header**: shows the tool icon, the currently active version badge, a quick **Make active** button when installed versions exist without an active selection, and a **Refresh** button.
- **First-launch callout**: when no version of the tool is installed yet, a top card offers a single **Install** button for the recommended newest version (which is activated automatically on first install).
- **Versions list**: lists every version available for your OS, newest first, showing status (`Active`, `Installed`, `Available`), an `Update <version>` badge when a newer patch exists in the same series, size, live download progress with **Cancel**, inline errors with **Retry**, **Make active**, and a trash icon opening the removal confirmation dialog.
- **Linux PHP profiles**: on Linux, the PHP page includes an **Extension profile** selector (**Common (default)** and **Bulk**) with a short description, an expandable **Extensions (N)** list per version row, and an inline notice when a build omits `pdo_pgsql` (such as PHP 8.0 `common`).
- **Composer dependency notice**: when no PHP version is active, the Composer page displays a banner explaining that Composer runs through the active PHP shim, with a **Go to PHP** shortcut.
- **Offline / empty catalog**: when the catalog is unavailable, a **Catalog unavailable** banner is shown while already-installed versions remain listed and switchable.
- **Live refresh**: DBLocal re-scans toolchain state when you open a toolchain page or return focus to the app window (no background polling timers), so installs or switches made via the CLI appear automatically.

### Shell PATH panel and consent dialog

Every toolchain page includes a **SHELL PATH** card showing whether `~/.dblocal/bin` is on `PATH` and first (`On PATH (first)`, `Not on PATH`, or `Needs attention`), along with plain-English findings from `doctor` (such as another `php` or `node` earlier on `PATH` with its full path, or a dangling shim).

Clicking **Add to PATH** never modifies shell files immediately: it opens a **consent dialog** displaying the exact files that will be updated (`~/.profile`, `~/.bashrc`, `~/.zshrc`, `~/.config/fish/conf.d/dblocal.fish`, or `HKCU\Environment#Path` on Windows) and the exact lines that will be appended, noting that `.dblocal-backup` backups are written first. Clicking **Remove from PATH** undoes the managed block cleanly and reports per-file outcomes, including any read-only or unreadable files.

### PHP settings card

The PHP page includes a **PHP SETTINGS** card (enabled whenever a PHP version is active) for editing `memory_limit`, `upload_max_filesize`, `post_max_size`, `max_execution_time`, `date.timezone`, and `display_errors`:

- Values are validated inline using the same parser as the CLI, with specific error messages under any invalid field.
- **Save** writes `~/.dblocal/toolchains/php/conf.d/99-dblocal.ini` atomically; **Reset to defaults** restores the default values; **Open config folder** opens the managed `conf.d` directory in your OS file manager.
