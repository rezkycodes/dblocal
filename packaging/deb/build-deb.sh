#!/usr/bin/env bash
# Build a .deb package from a prebuilt DBLocal release binary (F-PKG-04).
#
# Usage:
#   packaging/deb/build-deb.sh [--version <ver>] [--arch <dpkg-arch>] [--output <dir>]
#
# Inputs (env or flags):
#   DBLOCAL_BIN   Path to prebuilt release binary
#                 (default: $HOME/.cache/dblocal-target-gui/release/dblocal)
#
# The script does NOT compile DBLocal. It packages a prebuilt binary.
# It does NOT install the resulting package on this machine.
set -euo pipefail

resolve_dir() {
  ( unset CDPATH; cd -- "$1" && pwd -P )
}

script_dir="$(resolve_dir "$(dirname -- "$0")")"
repo_root="$(resolve_dir "$script_dir/../..")"
# shellcheck source=packaging/lib/version.sh
. "$repo_root/packaging/lib/version.sh"

usage() {
  cat >&2 <<'USAGE'
Usage: build-deb.sh [options]
  --version <ver>   Package version (default: git describe --tags --always, v stripped)
  --arch <arch>     dpkg architecture (default: dpkg --print-architecture or amd64)
  --output <dir>    Output directory (default: dist/deb)
  -h, --help        Show help
USAGE
}

VERSION_INPUT=""
ARCH=""
OUTPUT="dist/deb"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version) [ "$#" -ge 2 ] || { echo "error: --version requires a value" >&2; exit 2; }; VERSION_INPUT="$2"; shift 2 ;;
    --version=*) VERSION_INPUT="${1#--version=}"; shift ;;
    --arch) [ "$#" -ge 2 ] || { echo "error: --arch requires a value" >&2; exit 2; }; ARCH="$2"; shift 2 ;;
    --arch=*) ARCH="${1#--arch=}"; shift ;;
    --output) [ "$#" -ge 2 ] || { echo "error: --output requires a value" >&2; exit 2; }; OUTPUT="$2"; shift 2 ;;
    --output=*) OUTPUT="${1#--output=}"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "error: unknown argument: $1" >&2; usage; exit 2 ;;
  esac
done

DBLOCAL_BIN="${DBLOCAL_BIN:-$HOME/.cache/dblocal-target-gui/release/dblocal}"

# F2: explicit --version is validated; untagged default becomes 0.0.0~git<hash>.
# F1: prerelease '-' maps to '~' in the package version only (tag/name unchanged).
dblocal_version_default_or_validate "$VERSION_INPUT" "$repo_root" VERSION_RAW VERSION_PKG VERSION_SNAPSHOT
if [ "$VERSION_SNAPSHOT" = "1" ]; then
  echo "info: snapshot build; package version $VERSION_PKG (raw: $VERSION_RAW)" >&2
fi

if [ -z "$ARCH" ]; then
  if command -v dpkg >/dev/null 2>&1; then
    ARCH="$(dpkg --print-architecture)"
  else
    case "$(uname -m)" in
      x86_64|amd64) ARCH="amd64" ;;
      aarch64|arm64) ARCH="arm64" ;;
      *) echo "error: unsupported architecture $(uname -m); pass --arch" >&2; exit 1 ;;
    esac
  fi
fi

if [ ! -f "$DBLOCAL_BIN" ]; then
  echo "error: prebuilt binary not found: $DBLOCAL_BIN" >&2
  echo "Set DBLOCAL_BIN to a prebuilt release binary. This script does not compile DBLocal." >&2
  exit 1
fi
if [ ! -x "$DBLOCAL_BIN" ]; then
  echo "error: DBLOCAL_BIN is not executable: $DBLOCAL_BIN" >&2
  exit 1
fi

command -v dpkg-deb >/dev/null 2>&1 || { echo "error: required tool not found: dpkg-deb" >&2; exit 1; }
command -v objdump >/dev/null 2>&1 || { echo "error: required tool not found: objdump (needed to compute libc floor)" >&2; exit 1; }

# Depends are derived from the real binary's DT_NEEDED entries.
# F3: compute libc floor from the binary at build time; never hardcode.
# DT_NEEDED (readelf -d): libc.so.6 libgcc_s.so.1 libm.so.6 libxcb.so.1
# libxkbcommon.so.0 libxkbcommon-x11.so.0 libz.so.1 ld-linux-x86-64.so.2
#
# Debian package mapping (amd64/arm64):
#   libc.so.6, libm.so.6, ld-linux         -> libc6 (>= <computed GLIBC max>)
#   libgcc_s.so.1                           -> libgcc-s1 (>= 4.2)
#   libxcb.so.1                             -> libxcb1
#   libxkbcommon.so.0                       -> libxkbcommon0 (>= 0.5.0)
#   libxkbcommon-x11.so.0                   -> libxkbcommon-x11-0
#   libz.so.1                               -> zlib1g (>= 1:1.1.4)
#
# Recommends (F4): libvulkan1 for Vulkan, libegl1/libwayland-client0 for
# EGL/Wayland paths loaded by dlopen in the GPUI stack. Not hard Depends:
# GPUI can fall back to X11/xcb; missing optional backends should not block
# install on minimal/headless systems.
compute_glibc_floor() {
  local bin="$1" max
  max="$(objdump -T "$bin" | grep -oE 'GLIBC_[0-9]+(\.[0-9]+)+' | sort -V | tail -n 1)"
  if [ -z "$max" ]; then
    echo "error: could not determine GLIBC symbol versions from $bin (objdump -T produced no GLIBC_*)" >&2
    exit 1
  fi
  printf '%s' "${max#GLIBC_}"
}

GLIBC_FLOOR="$(compute_glibc_floor "$DBLOCAL_BIN")"
echo "info: computed libc floor from $DBLOCAL_BIN: GLIBC_${GLIBC_FLOOR} -> Depends libc6 (>= ${GLIBC_FLOOR})" >&2

case "$ARCH" in
  amd64|arm64)
    DEPENDS="libc6 (>= ${GLIBC_FLOOR}), libgcc-s1 (>= 4.2), libxcb1, libxkbcommon0 (>= 0.5.0), libxkbcommon-x11-0, zlib1g (>= 1:1.1.4)"
    ;;
  *)
    echo "error: unsupported deb arch '$ARCH'; add dependency mapping before building" >&2
    exit 1
    ;;
esac
RECOMMENDS="libvulkan1, libwayland-client0, libegl1"

workdir="$(mktemp -d)"
cleanup() { rm -rf "$workdir"; }
trap cleanup EXIT INT TERM

pkgroot="$workdir/pkg"
install -d -m 0755 \
  "$pkgroot/DEBIAN" \
  "$pkgroot/usr/bin" \
  "$pkgroot/usr/share/applications" \
  "$pkgroot/usr/share/icons/hicolor/scalable/apps" \
  "$pkgroot/usr/share/doc/dblocal"

install -m 0755 "$DBLOCAL_BIN" "$pkgroot/usr/bin/dblocal"
install -m 0644 "$repo_root/packaging/linux/dblocal.desktop" "$pkgroot/usr/share/applications/dblocal.desktop"
install -m 0644 "$repo_root/packaging/linux/dblocal.svg" "$pkgroot/usr/share/icons/hicolor/scalable/apps/dblocal.svg"

sed \
  -e "s/__VERSION__/$VERSION_PKG/g" \
  -e "s/__ARCH__/$ARCH/g" \
  -e "s/__DEPENDS__/$DEPENDS/g" \
  -e "s/__RECOMMENDS__/$RECOMMENDS/g" \
  "$script_dir/control.template" > "$pkgroot/DEBIAN/control"

installed_size_kb="$(du -sk "$pkgroot/usr" | awk '{print $1}')"
printf 'Installed-Size: %s\n' "$installed_size_kb" >> "$pkgroot/DEBIAN/control"

install -m 0755 "$script_dir/postinst" "$pkgroot/DEBIAN/postinst"
install -m 0755 "$script_dir/postrm" "$pkgroot/DEBIAN/postrm"

# Debian copyright file: includes GPL-3.0-only notice and points at the full
# license text shipped in the same package (COPYING). Third-party notices are
# referenced; the full third-party notice file is shipped when present.
cat > "$pkgroot/usr/share/doc/dblocal/copyright" <<EOF
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: DBLocal
Upstream-Contact: DBLocal Contributors <noreply@commandcode.ai>
Source: https://github.com/rezkycodes/dblocal

Files: *
Copyright: 2026 DBLocal Contributors
License: GPL-3.0-only

License: GPL-3.0-only
 DBLocal is free software: you can redistribute it and/or modify it under the
 terms of the GNU General Public License version 3 as published by the Free
 Software Foundation.
 .
 DBLocal is distributed in the hope that it will be useful, but WITHOUT ANY
 WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR
 A PARTICULAR PURPOSE. See the GNU General Public License for more details.
 .
 On Debian systems, the complete text of the GNU General Public License version
 3 can be found in /usr/share/common-licenses/GPL-3. A copy of the GPL-3.0-only
 license text is also included in /usr/share/doc/dblocal/COPYING.gz.
EOF

gzip -9 -n -c "$repo_root/COPYING" > "$pkgroot/usr/share/doc/dblocal/COPYING.gz"

if [ -f "$repo_root/docs/release/LICENSE-RING-AWSLC-NOTES.md" ]; then
  gzip -9 -n -c "$repo_root/docs/release/LICENSE-RING-AWSLC-NOTES.md" \
    > "$pkgroot/usr/share/doc/dblocal/LICENSE-RING-AWSLC-NOTES.md.gz"
fi

cat > "$pkgroot/usr/share/doc/dblocal/changelog.Debian" <<EOF
dblocal ($VERSION_PKG) unstable; urgency=medium

  * Automated package build.

 -- DBLocal Contributors <noreply@commandcode.ai>  $(date -R)
EOF
gzip -9 -n -f "$pkgroot/usr/share/doc/dblocal/changelog.Debian"

mkdir -p "$OUTPUT"
out_file="$OUTPUT/dblocal_${VERSION_PKG}_${ARCH}.deb"
dpkg-deb --build --root-owner-group "$pkgroot" "$out_file" >/dev/null

echo "wrote $out_file"
dpkg-deb -I "$out_file" | sed -n '1,120p'
