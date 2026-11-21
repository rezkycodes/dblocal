#!/usr/bin/env bash
# Best-effort local MSI build/validation helper (F-PKG-04).
#
# The real MSI is built on Windows in CI. On Linux we:
#   1. Validate WiX XML with xmllint (always).
#   2. If wixl (msitools) is installed, attempt a build.
#   3. Otherwise report clearly that MSI build is unverified on Linux.
#
# This script never installs packages and never requires sudo.
set -euo pipefail

resolve_dir() { ( unset CDPATH; cd -- "$1" && pwd -P ); }
script_dir="$(resolve_dir "$(dirname -- "$0")")"
repo_root="$(resolve_dir "$script_dir/../..")"
# shellcheck source=packaging/lib/version.sh
. "$repo_root/packaging/lib/version.sh"

VERSION_INPUT="${1:-}"
DBLOCAL_EXE="${DBLOCAL_BIN_EXE:-}"
OUT_DIR="${2:-dist/msi}"

# F2: explicit version is validated; default with no tag becomes 0.0.0~git<hash>.
dblocal_version_default_or_validate "$VERSION_INPUT" "$repo_root" VERSION_RAW VERSION_PKG VERSION_SNAPSHOT
if [ "$VERSION_SNAPSHOT" = "1" ]; then
  echo "info: snapshot build; package version $VERSION_PKG (raw: $VERSION_RAW)" >&2
fi
VERSION="$VERSION_PKG"

command -v xmllint >/dev/null 2>&1 || { echo "error: xmllint not found" >&2; exit 1; }
xmllint --noout "$script_dir/dblocal.wxs"
echo "xmllint: OK packaging/msi/dblocal.wxs"

if command -v wixl >/dev/null 2>&1; then
  if [ -z "$DBLOCAL_EXE" ] || [ ! -f "$DBLOCAL_EXE" ]; then
    echo "wixl found, but DBLOCAL_BIN_EXE is not set to an existing .exe; skipping wixl build." >&2
    echo "MSI build unverified on Linux without a Windows dblocal.exe input." >&2
    exit 0
  fi
  mkdir -p "$OUT_DIR"
  # WiX ProductVersion must be major.minor.build numeric; strip prerelease/snapshot.
  MSI_PRODUCT_VERSION="$(printf '%s' "$VERSION_RAW" | sed -E 's/^([0-9]+\.[0-9]+\.[0-9]+).*/\1/')"
  if ! printf '%s' "$MSI_PRODUCT_VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
    MSI_PRODUCT_VERSION="0.0.0"
  fi
  out="$OUT_DIR/dblocal-${VERSION_RAW}-windows-x86_64.msi"
  wixl -v \
    -D ProductVersion="$MSI_PRODUCT_VERSION" \
    -D DblocalExeSource="$DBLOCAL_EXE" \
    -o "$out" \
    "$script_dir/dblocal.wxs"
  echo "wrote $out"
  exit 0
fi

cat >&2 <<'MSG'
wixl (msitools) not found; MSI was XML-validated only.
The MSI is not built or run on Linux. CI builds it on windows-latest (WiX).
Treat the MSI as unbuilt and unrun until the owner tests it on Windows.
MSG
exit 0
