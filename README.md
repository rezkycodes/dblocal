# DBLocal

> **Status: pre-alpha, not yet released.** No installable artifacts exist yet. Everything in this
> repository describes the Phase 1 target; commands and downloads below will 404 until the first tag
> is published.

DBLocal is a Rust desktop app (GPUI) that installs and runs **PostgreSQL, MySQL, MariaDB and
Valkey** servers **on your own machine**, several versions side by side, with no containers and no
`sudo`. Pick a version, press Create, get a working connection string you can paste into `.env`.

It manages **servers**, not **data**: no table browser, no SQL editor. For that, DBLocal has an
"Open in…" button for DBDelve, TablePlus, DBeaver or `psql`.

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
  | sh -s -- --version v0.1.3

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
