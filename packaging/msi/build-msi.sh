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

VERSION="${1:-0.0.0-dev}"
DBLOCAL_EXE="${DBLOCAL_BIN_EXE:-}"
OUT_DIR="${2:-dist/msi}"

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
  out="$OUT_DIR/dblocal-${VERSION}-windows-x86_64.msi"
  wixl -v \
    -D ProductVersion="${VERSION#v}" \
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
