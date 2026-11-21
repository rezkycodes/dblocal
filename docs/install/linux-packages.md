# Linux package compatibility (deb/rpm/AppImage/tarball)

DBLocal Linux release artifacts are built from the same prebuilt release binary in CI
(`ubuntu-24.04` for x86_64 Linux builds). Package metadata is generated from that binary.

## GLIBC floor

The `.deb` build computes `Depends: libc6 (>= X.Y)` from the real binary at build time by taking
the maximum `GLIBC_*` symbol version reported by `objdump -T`. The `.rpm` build logs the same
computed floor; `rpmbuild` also auto-generates GLIBC requirements from the binary.

**Consequence:** the installable distro range is determined by the glibc version of the machine
that built the release binary, not by a hardcoded floor.

For a binary built on **Ubuntu 24.04** (glibc 2.39), the expected floor is `>= 2.39`. That means:

| Distro | glibc (typical) | CI-built package (glibc 2.39 floor) |
|---|---:|---|
| Ubuntu 24.04+ | 2.39+ | Installable |
| Ubuntu 22.04 | 2.35 | **Not installable** (glibc too old) |
| Debian 13 | 2.41 | Installable |
| Debian 12 | 2.36 | **Not installable** (glibc too old) |
| Fedora 40+ | 2.39+ | Installable |

I did not verify these distro glibc numbers by querying live distro package indexes from this
machine; they are the commonly published distro glibc baselines for those releases. The
authoritative check is the package dependency emitted by the build (`dpkg-deb -I` / `rpm -qpR`)
and the target distro's `libc6`/`glibc` package version.

This limitation applies to **all Linux artifacts built from that binary**, including the `.deb`,
`.rpm`, AppImage, and tarball, because the dynamic loader rejects an older system glibc before the
program starts.

## Optional GUI runtime libraries

The binary has DT_NEEDED entries for X11/xkbcommon/zlib and loads some graphics backends through
`dlopen` (Vulkan/EGL/Wayland client). Linux packages declare:

- **Depends/hard requires:** libc, libgcc, libxcb, libxkbcommon, libxkbcommon-x11, zlib.
- **Recommends:** `libvulkan1`, `libwayland-client0`, `libegl1` on Debian/Ubuntu; `vulkan-loader`,
  `libwayland-client`, `mesa-libEGL` on Fedora/RHEL.

These are Recommends, not hard Depends, because GPUI can fall back to the X11/xcb path; missing
optional backends should not block package install on minimal/headless systems. Vulkan absence is
reported by the app as an actionable error (F-SPK-01 AC2).

Debian package names were not verified with `apt` on this Fedora machine; Fedora/RHEL names were
checked locally with `rpm -q --whatprovides`.
