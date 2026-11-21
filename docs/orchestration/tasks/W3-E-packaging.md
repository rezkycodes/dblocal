# W3-E — Native packages (.deb, .rpm, .msi), release workflow, Windows verification guide

Branch `w3-packaging` from `main`. Read `docs/orchestration/tasks/_COMMON-RULES.md` first. PRD: **F-PKG-01..05** (especially F-PKG-04 and the business rules at the top of F-PKG), §6 size budget (60 MB), `docs/release/**`, `docs/install/**`, existing `packaging/` (install.sh, AppImage script, desktop file, svg), `.github/workflows/release.yml`, `scripts/install-linux-deps.sh`, and the WIN row in `docs/orchestration/PLAN.md`.

## Goal

Phase 2 packaging: `.deb`, `.rpm`, `.msi` produced by CI from the same release binary as the AppImage/tarball/zip, checksummed and size-gated; plus the guide the owner will follow to verify Windows by rebooting this dual-boot machine.

## You own

`packaging/**`, `.github/workflows/release.yml`, `scripts/**` (except `scripts/review/`), `docs/release/**`, `docs/install/**`, `docs/dev/windows-verification.md` (new). You do NOT edit any Rust crate. Build packages from a PREBUILT release binary passed as `DBLOCAL_BIN` (default `$HOME/.cache/dblocal-target-gui/release/dblocal`; do not compile the GUI yourself, it is slow and the machine is tight).

## Deliverables

1. **`.deb`**: `packaging/deb/` with a control template, `postinst`/`postrm`, a build script that uses `dpkg-deb` (no `cargo-deb` so `crates/cli/Cargo.toml` stays untouched). Depends determined from the real binary (`ldd` / `readelf -d`): list them and justify each. Installs `/usr/bin/dblocal`, the desktop file, icon, copyright file with the GPL-3.0-only text and the third-party notice, no files outside `/usr`. Removal leaves `~/.dblocal` alone and says so (F-PKG-04 AC2).
2. **`.rpm`**: `packaging/rpm/dblocal.spec` and a build script (`rpmbuild`; if it is not installed, say so and verify with `rpmspec`/by reading, never install it). Fedora and RHEL dependency names. Same file layout and uninstall message.
3. **`.msi`**: `packaging/msi/` WiX source (per-user install by default, no administrator needed; optional per-machine), Start Menu shortcut, uninstall keeps `%USERPROFILE%\.dblocal`, unsigned (document SmartScreen in `docs/install/`). It cannot be built on Linux: validate the XML (`xmllint`), try `wixl` if present, and have CI build it on `windows-latest`. Say plainly that it is unbuilt and unrun until the owner tests it on Windows.
4. **`release.yml`**: add the three package jobs to the matrix, include them in `checksums.txt` and the provenance attestation, keep the 60 MB size gate (build fails above it), nothing publishes unless the tag is `v*`. Validate with `actionlint` if installed, otherwise `yamllint` plus a careful read; you cannot run GitHub workflows, so list every step you could not execute.
5. **Local verification on Linux** with pasted output: `dpkg-deb -I`/`-c`, `lintian` if present, `rpm -qpl`/`-qp --scripts` (or `rpm2cpio | cpio -t`), a test that no path outside `/usr` (deb) or the declared set (rpm) is installed, that the postrm does not delete user data, and `shellcheck` on every script you wrote (install.sh included: idempotent, no `sudo` unless a flag asks for it). Do not install any package on this machine.
6. **`docs/dev/windows-verification.md`**: the checklist the owner follows after rebooting into Windows: toolchain (rustup, MSVC Build Tools, git, a Vulkan-capable GPU driver), `git clone`, the exact commands (`cargo xtask ci`, `cargo run -p dblocal-cli`), then a table of things to test with the expected result and a column to record the outcome: window opens, tray icon and menu, close-to-tray and quit dialog, Credential Manager entry created and removed on delete, rename/symlink/ACL behaviour of data dirs, engine start/stop on a real PostgreSQL and Valkey (ports), MSI install and uninstall, SmartScreen first-run steps, high-DPI, what to paste back to the coordinator. Nothing in it claims a result. Add a short macOS section marked "no macOS machine available: unverified".

## Done when

All six deliverables with pasted evidence, the exact list of what is unverified (MSI build, Windows and macOS behaviour, the workflow run on GitHub, a real install on Fedora/Ubuntu), and a note of anything in the PRD's F-PKG that the repository still lacks (for example notarization secrets, AppImage on aarch64).
