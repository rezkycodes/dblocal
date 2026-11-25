# Linux package compatibility (deb/rpm/AppImage/tarball)

DBLocal Linux release artifacts are built from the same prebuilt release binary in CI
(inside a digest-pinned `ubuntu:22.04` container on `ubuntu-24.04` and `ubuntu-24.04-arm`
runners). Package metadata is generated from that binary.

> **Verification status:** the `ubuntu:22.04` container build and `<= 2.35` GLIBC gate are
> wired into `.github/workflows/release.yml`, but end-to-end Linux artifact compatibility is
> **unverified until a real release-workflow run completes green on GitHub-hosted runners**
> (for example via a `release-dry/<name>` branch push).

## GLIBC floor

The Linux build job enforces an anti-regression gate (`dblocal_glibc_check_max ... 2.35`)
immediately after `cargo build --release`, failing the build if the binary requires any
`GLIBC_*` symbol newer than `2.35`.

The `.deb` build computes `Depends: libc6 (>= X.Y)` from the real binary at build time by taking
the maximum `GLIBC_*` symbol version reported by `readelf -V`. The `.rpm` build logs the same
computed floor; `rpmbuild` also auto-generates GLIBC requirements from the binary. `readelf` is
used instead of `objdump` because it parses ELF headers regardless of host binutils target
support, so x86_64 CI runners can compute the floor for AArch64 release binaries.

**Consequence:** the installable distro range is determined by the glibc version of the build
container (`ubuntu:22.04`, glibc 2.35) and guarded by the CI gate, not by a hardcoded floor.

For a binary built in the **Ubuntu 22.04** container (glibc 2.35), the expected floor is
`<= 2.35`. That means:

| Distro | glibc (typical) | CI-built package (glibc <= 2.35 floor) |
|---|---:|---|
| Ubuntu 24.04+ | 2.39+ | Installable |
| Ubuntu 22.04 | 2.35 | Installable |
| Debian 13 | 2.41 | Installable |
| Debian 12 | 2.36 | Installable |
| Fedora 41+ | 2.40+ | Installable |

The authoritative check for any published artifact is the package dependency emitted by the
build (`dpkg-deb -I` / `rpm -qpR`) and the target distro's `libc6`/`glibc` package version.

This floor applies to **all Linux artifacts built from that binary**, including the `.deb`,
`.rpm`, AppImage, and tarball, because the dynamic loader rejects an older system glibc before the
program starts.

## Runtime library dependencies

The `.deb` build derives `Depends` directly from the binary's `DT_NEEDED` entries (`readelf -d`)
via an explicit soname-to-Debian-package table in `packaging/deb/build-deb.sh` (unmapped sonames
fail the build). The `.rpm` build relies on `rpmbuild`'s automatic shared-library Requires.

In addition to `DT_NEEDED` libraries, DBLocal loads some graphics backends through `dlopen`
(Vulkan/EGL/Wayland client). Linux packages declare:

- **Depends/hard requires:** derived from `DT_NEEDED` (typically `libc6`, `libgcc-s1`, `libxcb1`,
  `libxkbcommon0`, `libxkbcommon-x11-0`, `zlib1g`, and `libfreetype6` if linked dynamically).
- **Recommends:** `libvulkan1`, `libwayland-client0`, `libegl1` on Debian/Ubuntu; `vulkan-loader`,
  `libwayland-client`, `mesa-libEGL` on Fedora/RHEL.

These are Recommends, not hard Depends, because GPUI can fall back to the X11/xcb path; missing
optional backends should not block package install on minimal/headless systems. Vulkan absence is
reported by the app as an actionable error (F-SPK-01 AC2).

Debian/Ubuntu package names were verified with `apt-cache policy` in `ubuntu:22.04` and
`debian:12` containers; Fedora/RHEL names were checked locally with `rpm -q --whatprovides`.
