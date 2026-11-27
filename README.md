# DBLocal

> **Status: pre-alpha, not yet released.** No installable artifacts exist yet. Everything in this
> repository describes the Phase 1 target; commands and downloads below will 404 until the first tag
> is published.

DBLocal is a Rust desktop app (GPUI) that installs and runs **PostgreSQL, MySQL, MariaDB,
Valkey, Redis and RustFS** servers **on your own machine**, several versions side by side, with no containers and no
`sudo`. Pick a version, press Create, get a working connection string you can paste into `.env`.

> **Redis note:** Redis `7.4.12` is available on **Linux only** (`x86_64` and `aarch64`, downloaded directly from Redis's official APT repository `packages.redis.io/deb`, requiring `libssl3`/`openssl-libs` and `libsystemd0`/`systemd-libs`). Redis 7.4 is dual-licensed **RSALv2 / SSPLv1**; DBLocal does not host or redistribute Redis binaries.

> **RustFS & Storage tab note:** RustFS (Apache-2.0, S3-compatible object storage) lives under the **Storage** tab in the New instance dialog (alongside **Database** and **Cache & Queue**). RustFS enables its web console on `[::]:9001` by default when no console flag is passed, so DBLocal **explicitly disables it** via `RUSTFS_CONSOLE_ENABLE=false` unless you check **Enable web console** in the dialog (or pass `--console` to `dblocal create --engine rustfs`), which binds the console strictly to a second loopback port (default `9001` when free) and shows an **Open console** button on the instance page.
>
> **RustFS security posture:** GitHub lists 43 published security advisories for RustFS (4 critical, 17 high, 18 medium, 4 low), all patched in `1.0.2` or earlier, with none listing `1.0.7` (released `2026-11-26`, latest stable) as affected as of `2026-11-27`; the ongoing risk is the rate of new findings rather than unpatched known ones. DBLocal enforces a `>= 1.0.6` minimum version, pins explicit versioned archives (never `-latest`), explicitly disables the console unless requested, binds both the S3 API and optional web console strictly to `127.0.0.1` (verified on Linux against `/proc/<pid>/fd` and `/proc/net/tcp{,6}` after startup), generates random per-instance access and 24-character secret keys passed only via environment variables (refusing empty or default `rustfsadmin` credentials), and refreshes the catalog daily so new security patches propagate promptly.

It manages **servers**, not **data**: no table browser, no SQL editor. For that, DBLocal has an
"Open in…" button for DBDelve, TablePlus, DBeaver or `psql`.

## Upgrading and going back

Before DBLocal upgrades `~/.dblocal/state.db` to a newer schema it writes a complete copy of the
database as it was, named after the version it found: upgrading from schema version 1 leaves
`state.db.v1.bak` next to `state.db`. If the copy cannot be written and verified, DBLocal does not
migrate and reports the error instead, so the old database is never half-upgraded.

To go back to v0.1.x after upgrading:

1. Stop every instance and quit DBLocal (both the app and any `dblocal` CLI command).
2. Delete `state.db-wal` and `state.db-shm` in `~/.dblocal/`.
3. Copy `state.db.v1.bak` over `state.db`.
4. Start the old version.

The restored database is the one from before the upgrade: instances created after upgrading are not
in it.

## Why not Docker or a system package?

| Today | What breaks |
|---|---|
| `dnf install postgresql-server`, `brew install mysql` | One major version per machine. An old project needs PG 14, a new one needs PG 18. |
| `docker compose up` per project | RAM and battery cost at idle, a YAML file to maintain, an extra abstraction to debug, WSL2 on Windows. |

DBLocal runs each engine as a **native process owned by your user**, detached from the app, so the
database stays up when you close the window.

## Install

**Coming soon.** These commands document the intended Phase 1 install paths.

### Script (Linux, macOS)

```sh
curl -fsSL https://raw.githubusercontent.com/rezkycodes/dblocal/main/packaging/install.sh | sh
```

The script detects your OS and architecture, downloads the matching release, **verifies SHA-256
against `checksums.txt`**, and installs `dblocal` to `~/.local/bin` (or `/usr/local/bin` when
writable). It is idempotent — re-run it to upgrade — and never calls `sudo` unless you pass
`--sudo`.

`checksums.txt` is downloaded from the same release as the archive, so it protects against
corruption and a bad mirror but not against a compromised release; verify provenance with
`gh attestation verify` for a stronger guarantee.

```sh
# install a specific tag
curl -fsSL https://raw.githubusercontent.com/rezkycodes/dblocal/main/packaging/install.sh \
  | sh -s -- --version v0.2.2

# see what it would do, without changing anything
sh install.sh --dry-run

# choose the install directory
sh install.sh --prefix "$HOME/bin"
```

### Release downloads (GitHub Releases)

| Platform | Artifact |
|---|---|
| Linux x86_64 | `dblocal-<version>-linux-x86_64.AppImage`, `…-linux-x86_64.tar.gz` |
| Linux aarch64 | `dblocal-<version>-linux-aarch64.AppImage`, `…-linux-aarch64.tar.gz` |
| Windows x86_64 | `dblocal-<version>-windows-x86_64.zip` |

macOS (`.dmg`, `.tar.gz`) is **not** part of the first release — see
[Supported platforms](#supported-platforms).

Each release also ships **`checksums.txt`** and GitHub **build provenance attestations**. Verify
before running:

```sh
sha256sum -c checksums.txt --ignore-missing      # Linux
gh attestation verify dblocal-<version>-linux-x86_64.AppImage --repo rezkycodes/dblocal
```

### Updating

Starting with **v0.1.4**, DBLocal can update itself from inside the app (via the title-bar download icon or **Settings → Updates**):

- **Self-updating installs:** per-user tarball (including `install.sh`), AppImage, Windows `.zip`, and per-user Windows `.msi`.
- **Managed installs:** `.deb`, `.rpm`, and per-machine Windows `.msi` cannot replace themselves; the update dialog links to the GitHub release page instead.
- **Upgrading to v0.1.4:** v0.1.4 is the first release that includes the in-app updater, so it must be installed manually before subsequent releases can update in place.

## Supported platforms

The first release targets **Linux and Windows**. macOS is deferred, not dropped.

| OS | Architecture | Notes |
|---|---|---|
| Linux (Fedora 41+, Ubuntu 22.04+, Debian 12+) | x86_64 | Requires a working **Vulkan** driver (Mesa). Without one, the app exits with an actionable message instead of crashing. |
| Linux | aarch64 | Best effort. |
| Windows 10 22H2+ / 11 | x86_64 | **Unsigned** in v1 — see below. |
| macOS 13+ | arm64 + x86_64 (universal) | **Deferred for the first release.** |

> **macOS: deferred for the first release.** The first release targets Linux and Windows only;
> macOS is postponed, not dropped — see [`docs/dev/macos-deferred.md`](docs/dev/macos-deferred.md).

### Engine availability

Not every engine has a build for every OS. The matrix below is what the catalog
(`catalog/engines.json`) ships. A combination with no build is shown in the app as
**"Not available on this platform"** — never as a hidden engine or a download that fails later. The
CLI refuses the same combination with the `DBL-6001` code. The macOS column lists catalog coverage
only; macOS is not in the first release.

| Engine | Linux x86_64 | Linux aarch64 | Windows x86_64 | macOS (catalog only) |
|---|---|---|---|---|
| PostgreSQL | ✅ | ✅ | ✅ | ✅ |
| MySQL | ✅ | — | ✅ | ✅ |
| MariaDB | ✅ | — | ✅ | — |
| Valkey | ✅ | ✅ | **not available yet** | — |
| Redis | ✅ | ✅ | — | — |
| RustFS | ✅ | ✅ | ✅ | — |

Dashes mean the upstream project publishes no binary for that target. **Valkey has no Windows
build in this release** (deferred together with its CI job); on Windows the engine card and
`dblocal create valkey` report that it is not available on the platform.

## Windows: "Unknown publisher"

The Windows build is **not code-signed in v1** (a code-signing certificate is a recurring cost the
project does not carry yet). Windows SmartScreen will show an *"Unknown publisher"* prompt the first
time you run it. That is expected, not a sign of tampering by itself — but since you cannot verify
the publisher, **verify the checksum** before you run the app:

```powershell
Get-FileHash .\dblocal-<version>-windows-x86_64.zip -Algorithm SHA256
# compare with the matching line in checksums.txt from the same release
```

Full instructions, including what the prompt looks like and how to proceed safely, are in
[`docs/install/windows-smartscreen.md`](docs/install/windows-smartscreen.md).

## Privacy: there is no telemetry

DBLocal collects **no telemetry**, has **no accounts**, and sends nothing to any server except:

1. HTTP requests to the **engine download sources** and the catalog, and
2. a version check against the **GitHub Releases API**.

No identifiers, no usage data. All state stays on your machine, under `~/.dblocal/`. Passwords live
in your OS keychain (Secret Service / Keychain / Credential Manager), never in the app database.

## Where your data lives

```
~/.dblocal/
├─ runtimes/<engine>/<version>/     # downloaded engine binaries
├─ instances/<name>/{data,logs,run} # per-instance data directory, logs, pidfiles
├─ archive/                         # deleted instances whose data you kept
└─ state.db                         # app state (SQLite, WAL)
```

Uninstalling DBLocal **does not delete** `~/.dblocal/`; your databases survive.

Each downloaded engine version stays under `runtimes/` even after you delete every instance that
used it. To reclaim that space, remove an unused runtime from the dock's **RUNTIMES** section, or
run `dblocal runtime list` to see what is installed and `dblocal runtime remove <engine> <version>
--yes` to delete one. A runtime still used by an instance is refused; a removed engine is simply
downloaded again the next time you create an instance of that version.

## License

DBLocal is free software under the **GNU General Public License v3.0 only** (`GPL-3.0-only`). See
[`COPYING`](COPYING). Every release binary corresponds to a source tag, satisfying GPLv3 §6. If you
distribute a modified version, it must also be GPL-3.0.

Third-party dependency licenses are checked with `cargo deny`; the allowlist is limited to
GPL-3.0-compatible licenses.

## Development

```sh
cargo xtask ci          # fmt check + clippy -D warnings + tests
cargo xtask catalog build --output catalog/engines.json
cargo deny check licenses bans sources advisories
```

Release process, required secrets and package plan: [`docs/release/RELEASING.md`](docs/release/RELEASING.md).
