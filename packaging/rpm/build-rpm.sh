#!/usr/bin/env bash
# Build an .rpm package from a prebuilt DBLocal release binary (F-PKG-04).
#
# Usage:
#   packaging/rpm/build-rpm.sh [--version <ver>] [--arch <rpm-arch>] [--output <dir>]
#
# Inputs:
#   DBLOCAL_BIN   Path to prebuilt release binary
#                 (default: $HOME/.cache/dblocal-target-gui/release/dblocal)
#
# The script does NOT compile DBLocal. It packages a prebuilt binary.
# It does NOT install the resulting package on this machine.
#
# rpmbuild is required. If it is missing, this script exits with a clear error
# and does not attempt to install it (no sudo, no dnf/apt).
set -euo pipefail

resolve_dir() {
  ( unset CDPATH; cd -- "$1" && pwd -P )
}

script_dir="$(resolve_dir "$(dirname -- "$0")")"
repo_root="$(resolve_dir "$script_dir/../..")"
# shellcheck source=packaging/lib/version.sh
. "$repo_root/packaging/lib/version.sh"
# shellcheck source=packaging/lib/glibc.sh
. "$repo_root/packaging/lib/glibc.sh"

usage() {
  cat >&2 <<'USAGE'
Usage: build-rpm.sh [options]
  --version <ver>   Package version (default: git describe --tags --always, v stripped)
  --arch <arch>     RPM architecture (default: uname -m; x86_64/aarch64)
  --output <dir>    Output directory (default: dist/rpm)
  -h, --help        Show help
USAGE
}

VERSION_INPUT=""
ARCH=""
OUTPUT="dist/rpm"

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
# F1: prerelease '-' maps to '~' in package version for correct ordering.
# RPM Version allows '~' for prerelease ordering; keep tag/file naming raw.
dblocal_version_default_or_validate "$VERSION_INPUT" "$repo_root" VERSION_RAW RPM_VERSION VERSION_SNAPSHOT
if [ "$VERSION_SNAPSHOT" = "1" ]; then
  echo "info: snapshot build; package version $RPM_VERSION (raw: $VERSION_RAW)" >&2
fi

if [ -z "$ARCH" ]; then
  case "$(uname -m)" in
    x86_64|amd64) ARCH="x86_64" ;;
    aarch64|arm64) ARCH="aarch64" ;;
    *) echo "error: unsupported architecture $(uname -m); pass --arch" >&2; exit 1 ;;
  esac
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

# F3: compute libc floor from the binary (same check as deb). rpmbuild
# auto-generates GLIBC requires; we log the computed floor so the build record
# shows the real minimum. Fail clearly if readelf is missing or finds nothing.
GLIBC_FLOOR="$(dblocal_glibc_floor "$DBLOCAL_BIN")"
echo "info: computed libc floor from $DBLOCAL_BIN: GLIBC_${GLIBC_FLOOR}; rpmbuild will auto-generate matching libc requires" >&2

if ! command -v rpmbuild >/dev/null 2>&1; then
  cat >&2 <<'ERR'
error: rpmbuild not found.
This script cannot install packages (no sudo, no dnf/apt). Install rpm-build
on a build machine/CI runner, or use the release workflow's RPM job.
You can still validate the spec with: rpmspec -P packaging/rpm/dblocal.spec
ERR
  exit 1
fi

workdir="$(mktemp -d)"
cleanup() { rm -rf "$workdir"; }
trap cleanup EXIT INT TERM

topdir="$workdir/rpmbuild"
mkdir -p "$topdir"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}

cp "$DBLOCAL_BIN" "$topdir/SOURCES/dblocal"
cp "$repo_root/packaging/linux/dblocal.desktop" "$topdir/SOURCES/dblocal.desktop"
cp "$repo_root/packaging/linux/dblocal.svg" "$topdir/SOURCES/dblocal.svg"
cp "$repo_root/COPYING" "$topdir/SOURCES/COPYING"
if [ -f "$repo_root/docs/release/LICENSE-RING-AWSLC-NOTES.md" ]; then
  cp "$repo_root/docs/release/LICENSE-RING-AWSLC-NOTES.md" "$topdir/SOURCES/LICENSE-RING-AWSLC-NOTES.md"
else
  : > "$topdir/SOURCES/LICENSE-RING-AWSLC-NOTES.md"
fi
cp "$script_dir/dblocal.spec" "$topdir/SPECS/dblocal.spec"

rpmbuild -bb \
  --define "_topdir $topdir" \
  --define "_dblocal_version $RPM_VERSION" \
  --target "$ARCH" \
  "$topdir/SPECS/dblocal.spec" >&2

mkdir -p "$OUTPUT"
rpm_path="$(find "$topdir/RPMS" -type f -name '*.rpm' | head -n 1)"
if [ -z "$rpm_path" ]; then
  echo "error: rpmbuild did not produce an .rpm" >&2
  exit 1
fi

dest="$OUTPUT/$(basename "$rpm_path")"
cp "$rpm_path" "$dest"
echo "wrote $dest"
if command -v rpm >/dev/null 2>&1; then
  rpm -qpi "$dest" | sed -n '1,120p'
  echo "---"
  rpm -qpl "$dest"
fi
