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

usage() {
  cat >&2 <<'USAGE'
Usage: build-deb.sh [options]
  --version <ver>   Package version (default: git describe --tags --always, v stripped)
  --arch <arch>     dpkg architecture (default: dpkg --print-architecture or amd64)
  --output <dir>    Output directory (default: dist/deb)
  -h, --help        Show help
USAGE
}

VERSION=""
ARCH=""
OUTPUT="dist/deb"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version) [ "$#" -ge 2 ] || { echo "error: --version requires a value" >&2; exit 2; }; VERSION="$2"; shift 2 ;;
    --version=*) VERSION="${1#--version=}"; shift ;;
    --arch) [ "$#" -ge 2 ] || { echo "error: --arch requires a value" >&2; exit 2; }; ARCH="$2"; shift 2 ;;
    --arch=*) ARCH="${1#--arch=}"; shift ;;
    --output) [ "$#" -ge 2 ] || { echo "error: --output requires a value" >&2; exit 2; }; OUTPUT="$2"; shift 2 ;;
    --output=*) OUTPUT="${1#--output=}"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "error: unknown argument: $1" >&2; usage; exit 2 ;;
  esac
done

DBLOCAL_BIN="${DBLOCAL_BIN:-$HOME/.cache/dblocal-target-gui/release/dblocal}"

if [ -z "$VERSION" ]; then
  VERSION="$(git -C "$repo_root" describe --tags --always 2>/dev/null || echo 0.0.0-dev)"
fi
VERSION="${VERSION#v}"

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

# Depends are derived from the real binary's DT_NEEDED entries and verified
# against ldd output. We intentionally do not use dpkg-shlibdeps because it is
# not installed in this environment and the package is built from a prebuilt
# binary outside a dpkg build tree.
#
# DT_NEEDED (readelf -d): libc.so.6 libgcc_s.so.1 libm.so.6 libxcb.so.1
# libxkbcommon.so.0 libxkbcommon-x11.so.0 libz.so.1 ld-linux-x86-64.so.2
#
# Debian package mapping (amd64):
#   libc.so.6, libm.so.6, ld-linux         -> libc6 (>= 2.31)
#   libgcc_s.so.1                           -> libgcc-s1 (>= 4.2)
#   libxcb.so.1                             -> libxcb1
#   libxkbcommon.so.0                       -> libxkbcommon0 (>= 0.5.0)
#   libxkbcommon-x11.so.0                   -> libxkbcommon-x11-0
#   libz.so.1                               -> zlib1g (>= 1:1.1.4)
#
# libvulkan1 is a Recommends (not Depends): GPUI renders through Vulkan and
# needs a host Vulkan driver/loader, but the package should remain installable
# on headless systems; the app reports a clear error when Vulkan is missing
# (F-SPK-01 AC2 / docs/release/RELEASING.md).
case "$ARCH" in
  amd64)
    DEPENDS="libc6 (>= 2.31), libgcc-s1 (>= 4.2), libxcb1, libxkbcommon0 (>= 0.5.0), libxkbcommon-x11-0, zlib1g (>= 1:1.1.4)"
    ;;
  arm64)
    DEPENDS="libc6 (>= 2.31), libgcc-s1 (>= 4.2), libxcb1, libxkbcommon0 (>= 0.5.0), libxkbcommon-x11-0, zlib1g (>= 1:1.1.4)"
    ;;
  *)
    echo "error: unsupported deb arch '$ARCH'; add dependency mapping before building" >&2
    exit 1
    ;;
esac

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
  -e "s/__VERSION__/$VERSION/g" \
  -e "s/__ARCH__/$ARCH/g" \
  -e "s/__DEPENDS__/$DEPENDS/g" \
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
dblocal ($VERSION) unstable; urgency=medium

  * Automated package build.

 -- DBLocal Contributors <noreply@commandcode.ai>  $(date -R)
EOF
gzip -9 -n -f "$pkgroot/usr/share/doc/dblocal/changelog.Debian"

mkdir -p "$OUTPUT"
out_file="$OUTPUT/dblocal_${VERSION}_${ARCH}.deb"
dpkg-deb --build --root-owner-group "$pkgroot" "$out_file" >/dev/null

echo "wrote $out_file"
dpkg-deb -I "$out_file" | sed -n '1,80p'
