# W0-D — CI, packaging, install script and legal

Branch: `w0-ci-packaging`. Read first: AGENTS.md, then PRD §6 `F-PKG-01..05`, `F-SPK-02`, `F-SPK-05`, `F-CAT-03`, §10 (licence and
distribution NFRs), decisions D-05, D-08, D-13, D-20 and open questions OPEN-04, OPEN-05, OPEN-06.

## Goal

Everything needed to build, check and ship DBLocal that does not depend on the Rust code existing yet, plus the GPL-3.0 legal files.

## You own

`.github/**`, `packaging/**`, `scripts/**`, `deny.toml`, `COPYING`, `README.md`, `docs/release/**`, `docs/install/**`. Nothing else.
The Rust workspace is being created in parallel by another worker (binary `dblocal` from `crates/cli`, `cargo xtask ci`,
`cargo xtask catalog build`). Reference those names; do not create them.

## Deliverables

### 1. Legal

- `COPYING`: the full, verbatim GPL-3.0 text from `https://www.gnu.org/licenses/gpl-3.0.txt`. Verify it is complete (starts with
  "GNU GENERAL PUBLIC LICENSE Version 3, 29 June 2007" and ends with the "Use with the GNU Affero..." section).
- `deny.toml` for `cargo deny`: licences allowlist limited to GPL-3.0-compatible licences (MIT, Apache-2.0, Apache-2.0 WITH
  LLVM-exception, BSD-2-Clause, BSD-3-Clause, ISC, Unicode-3.0, Zlib, MPL-2.0, CC0-1.0, 0BSD, BSL-1.0, Unlicense, GPL-3.0-only,
  GPL-3.0-or-later, LGPL-3.0-or-later). `bans`: deny `openssl`, `openssl-sys`, `native-tls` (rustls only). `sources`: crates.io only.
  `advisories`: fail on vulnerabilities. Do **not** allow the `OpenSSL` licence. Add a comment block explaining that `ring` and
  `aws-lc-sys` carry OpenSSL-derived code and that this is an unresolved question for GPL-3.0 (see report). You cannot run
  `cargo deny` until the workspace exists; write the config carefully and note it as unverified.
- Research and report: is the `ring` / `aws-lc-rs` licence situation a real GPL-3.0 problem for a binary that uses
  `reqwest` + `rustls`? Cite the FSF position and what other GPL projects do. Recommend one option (for example `rustls` with the
  `ring` provider and a documented licence exception, or a different crypto provider).

### 2. GitHub Actions (`.github/workflows/`)

Use current action versions; verify names and versions rather than recalling them. Verify runner labels exist today (the Intel macOS
label has changed over time).

- `ci.yml`: on push and pull request. Matrix `ubuntu-24.04`, `ubuntu-24.04-arm`, `macos-14`, an Intel macOS runner, `windows-latest`.
  Steps: install Linux GPUI build dependencies (take the list from Zed's `script/linux`, which is the source of truth; cite the commit
  you read), `cargo xtask ci`, `cargo deny check`, `cargo audit`. Cache with `Swatinem/rust-cache`.
- `release.yml`: on tag `v*`. Builds `linux-x86_64`, `linux-aarch64`, `macos-universal`, `windows-x86_64`. Artifacts for Phase 1:
  `.AppImage` and `.tar.gz` (Linux), `.dmg` (macOS), `.zip` (Windows), plus `checksums.txt` and build provenance via
  `actions/attest-build-provenance`. **Size gate**: fail if any artifact is over 60 MB. macOS signing and notarisation use Apple
  Developer ID secrets; if the secrets are absent the job still produces an **unsigned** `.dmg` and prints a loud warning (D-20).
  Windows is unsigned in v1. Phase 2 packages (`.deb`, `.rpm`, `.msi`) are described in `docs/release/RELEASING.md`, not built yet.
- `catalog.yml`: daily schedule + manual. Runs `cargo xtask catalog build --output catalog/engines.json` and opens a pull request
  (never pushes to main) with `peter-evans/create-pull-request` or equivalent.
- `valkey-build.yml` (`F-SPK-02`): manual. Builds Valkey from source with CMake on `windows-latest`, `macos-14` and an Intel macOS
  runner, then starts `valkey-server`, answers `PING`, shuts down, and uploads the binary. Pin the Valkey tag as an input. Add a
  header comment: **UNVERIFIED until run on GitHub**. Also write `docs/spikes/valkey-build.md` explaining what to look at.
- Validate every workflow as YAML (`python3 -c 'import yaml,sys; ...'`) and with `actionlint` if it is installed. State what you could not verify.

### 3. Install script (`packaging/install.sh`, `F-PKG-03`)

POSIX `sh`, passes `shellcheck` (run it if installed; otherwise say it was not run). Must:

- Detect OS and arch (Linux and macOS; x86_64 and aarch64/arm64). Unsupported targets stop with a clear message and install nothing.
- Download the right artifact from GitHub Releases (`DBLOCAL_REPO`, default `rezkycodes/dblocal`; the project name is unconfirmed, OPEN-06),
  verify SHA-256 against `checksums.txt` (`sha256sum` or `shasum -a 256`), install to `~/.local/bin`, or `/usr/local/bin` when writable.
- Be idempotent (re-running upgrades in place), support `--version <tag>`, `--dry-run`, `--prefix <dir>`, and never call `sudo` unless the
  user passes `--sudo`.
- Print the next steps, and warn if the install directory is not on `PATH`.
- Test it **offline**: `packaging/test/install_test.sh` serves a fake release from a local `python3 -m http.server` and exercises
  detection (via overridable `DBLOCAL_TEST_UNAME_S` / `DBLOCAL_TEST_UNAME_M`), checksum mismatch rejection, dry run, and idempotence.

### 4. Linux packaging descriptors (`packaging/linux/`)

`dblocal.desktop`, a simple original `dblocal.svg` icon, and an AppImage `AppDir` layout script `packaging/appimage/build-appimage.sh`
that takes a built `dblocal` binary. Document how the AppImage depends on the host Vulkan driver.

### 5. Docs

- Root `README.md`: what DBLocal is, install (`curl -fsSL .../install.sh | sh`, release downloads), supported platforms, the GPL-3.0 licence,
  the fact that there is no telemetry, the unsigned-Windows note, and a "Status: pre-alpha, not yet released" banner.
- `docs/install/windows-smartscreen.md`: honest instructions for the "Unknown publisher" prompt (D-20), with checksum verification steps.
- `docs/release/RELEASING.md`: how to cut a release, every required secret (Apple Developer ID certificate, notarisation credentials), how to
  verify checksums and provenance, and the Phase 2 packaging plan (`.deb` via cargo-deb, `.rpm` via cargo-generate-rpm, `.msi` via WiX).

## Evidence required in your report

The YAML validation output, the install-script test output, `shellcheck` output (or a statement it is not installed), the `COPYING`
verification (line count, first and last lines), the licence research verdict first, and a list of everything you could not verify.
