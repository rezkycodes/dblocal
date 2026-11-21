# Packaging DBLocal (Phase 2, F-PKG-04)

This document covers the native packages added in Phase 2: `.deb`, `.rpm`, and `.msi`.
It complements [`RELEASING.md`](./RELEASING.md), which covers the full release flow.

## Scope and honesty

- `.deb` and `.rpm` are **built from a prebuilt release binary** via `packaging/deb/build-deb.sh`
  and `packaging/rpm/build-rpm.sh`. They do not compile DBLocal and do not use `cargo-deb` or
  `cargo-generate-rpm`, so no Rust crate manifest is modified.
- `.msi` is authored as WiX source (`packaging/msi/dblocal.wxs`) and validated on Linux with
  `xmllint`. The MSI is **not built or run on Linux** in this environment; CI builds it on
  `windows-latest` with WiX. Until the owner tests it on Windows, treat it as unbuilt/unrun.
- No package is installed on this development machine by these scripts. Local verification uses
  metadata/listing tools (`dpkg-deb`, `rpm -qpl`, `rpm2cpio`) and tests.

## Inputs

All package build scripts take a prebuilt binary:

- Linux packages: `DBLOCAL_BIN` (default `$HOME/.cache/dblocal-target-gui/release/dblocal`)
- Windows MSI: `-DblocalExe path\to\dblocal.exe` (CI passes the built `dblocal.exe`)

The binary is the same release artifact produced by the release workflow.

## Runtime dependencies (Linux)

Derived from `readelf -d` DT_NEEDED on the actual release binary:

| DT_NEEDED | Debian/Ubuntu | Fedora/RHEL | Why |
|---|---|---|---|
| `libc.so.6`, `libm.so.6`, `ld-linux-x86-64.so.2` | `libc6` | `glibc` | C runtime and dynamic loader |
| `libgcc_s.so.1` | `libgcc-s1` | `libgcc` | GCC unwind/runtime support used by Rust std |
| `libxcb.so.1` | `libxcb1` | `libxcb` | X11/XCB windowing (GPUI Linux backend) |
| `libxkbcommon.so.0` | `libxkbcommon0` | `libxkbcommon` | Keyboard handling |
| `libxkbcommon-x11.so.0` | `libxkbcommon-x11-0` | `libxkbcommon-x11` | X11 keyboard handling |
| `libz.so.1` | `zlib1g` | `zlib` (or `zlib-ng-compat` provides) | Compression used by dependencies |

`libvulkan1` / `vulkan-loader` is a **Recommends**, not a hard Depends. GPUI renders through
Vulkan and needs a host Vulkan driver; the app is expected to report an actionable error when
Vulkan is missing (F-SPK-01 AC2). Making it a hard dependency would block install on minimal
headless systems where DBLocal CLI can still be useful.

The package must not depend on engine binaries. Engines are downloaded and managed by DBLocal into
`~/.dblocal`.

## Install layout

Linux packages install only under `/usr`:

| Path | Purpose |
|---|---|
| `/usr/bin/dblocal` | CLI/GUI binary |
| `/usr/share/applications/dblocal.desktop` | Desktop entry |
| `/usr/share/icons/hicolor/scalable/apps/dblocal.svg` | Icon |
| `/usr/share/doc/dblocal/copyright` | Debian copyright (deb) |
| `/usr/share/doc/dblocal/COPYING.gz` | GPL-3.0-only text |
| `/usr/share/doc/dblocal/LICENSE-RING-AWSLC-NOTES.md.gz` | Third-party licence note |
| `/usr/share/licenses/dblocal/*` | Licence files (rpm) |

No files are installed outside these paths. Package scripts do not create or delete `~/.dblocal`.

## Uninstall / user data (F-PKG-04 AC2)

Removing `.deb`/`.rpm`/`.msi` deletes package files only. `~/.dblocal` (engine runtimes, instance
data, archives, `state.db`) is intentionally kept. The uninstall scripts/message say so:

- deb/rpm postrm/postun: prints a message on remove/purge that `~/.dblocal` is kept.
- MSI: no custom action deletes `%USERPROFILE%\.dblocal`; ARP comment documents it.

## Build commands

```sh
# Debian/Ubuntu
DBLOCAL_BIN=$HOME/.cache/dblocal-target-gui/release/dblocal \
  packaging/deb/build-deb.sh --version 0.1.0 --arch amd64 --output dist/deb

# Fedora/RHEL (requires rpmbuild)
DBLOCAL_BIN=$HOME/.cache/dblocal-target-gui/release/dblocal \
  packaging/rpm/build-rpm.sh --version 0.1.0 --arch x86_64 --output dist/rpm

# MSI XML validation (Linux)
xmllint --noout packaging/msi/dblocal.wxs

# MSI build (Windows CI / Windows machine with WiX v3)
pwsh packaging/msi/build-msi.ps1 -Version v0.1.0 -DblocalExe .\dblocal.exe -OutDir dist\msi
```

## Verification checklist

- `dpkg-deb -I` and `dpkg-deb -c` show expected metadata and only `/usr/...` paths.
- `rpm -qpl`/`rpm2cpio | cpio -t` show only declared paths.
- `rpm -qp --scripts` shows post/postun messages that keep `~/.dblocal`.
- `lintian` if present; otherwise record as unverified.
- `shellcheck` on all packaging scripts and `packaging/install.sh`.
- No package install on the dev machine.
