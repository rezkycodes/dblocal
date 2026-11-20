#!/usr/bin/env bash
# build-appimage.sh — assemble an AppDir and pack it into a .AppImage.
#
# Usage:
#   packaging/appimage/build-appimage.sh <path-to-dblocal-binary> [<output.AppImage>]
#
# What it does (F-PKG-02):
#   1. Creates a self-contained AppDir (AppRun, .desktop, icon, binary).
#   2. Looks for appimagetool (bundled AppImage) in $APPIMAGETOOL, then on PATH;
#      if it is a .AppImage itself, runs it via --appimage-extract-and-run so no
#      FUSE is required on the build host.
#   3. Packs the AppDir into a type-2 AppImage.
#
# Host dependency note (IMPORTANT):
#   The AppImage bundles the DBLocal binary and its Rust/GPUI runtime libraries
#   that the packer can find, but it does NOT bundle a GPU driver. GPUI renders
#   through Vulkan, so the .AppImage requires a working Vulkan driver on the
#   host: Mesa (radv/ANV/lavapipe) on Linux. On a machine without Vulkan the app
#   is expected to exit with an actionable message (F-SPK-01 AC2), not crash.
#   See docs/release/RELEASING.md "AppImage and the Vulkan driver".
#
# This script is UNVERIFIED until release.yml runs it on a real Linux runner.
set -euo pipefail

resolve_dir() {
  # CDPATH can make `cd` print arbitrary paths; neutralise it locally.
  ( unset CDPATH; cd -- "$1" && pwd -P )
}

script_dir="$(resolve_dir "$(dirname -- "$0")")"
repo_root="$(resolve_dir "$script_dir/../..")"

# Global so the EXIT trap (which outlives main's local scope) can see it.
workdir=""
cleanup() {
  if [ -n "$workdir" ] && [ -d "$workdir" ]; then
    rm -rf "$workdir"
  fi
}
trap cleanup EXIT INT TERM

main() {
  if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
    printf 'usage: %s <path-to-dblocal-binary> [<output.AppImage>]\n' "$0" >&2
    exit 2
  fi

  local binary="$1"
  local output="${2:-}"

  if [ ! -f "$binary" ]; then
    printf 'error: binary not found: %s\n' "$binary" >&2
    exit 1
  fi
  if [ ! -x "$binary" ]; then
    printf 'error: binary is not executable: %s (run: chmod +x)\n' "$binary" >&2
    exit 1
  fi

  if [ -z "$output" ]; then
    case "$(uname -m)" in
      x86_64|amd64) arch="linux-x86_64" ;;
      aarch64|arm64) arch="linux-aarch64" ;;
      *) printf 'error: unsupported architecture %s\n' "$(uname -m)" >&2; exit 1 ;;
    esac
    output="$PWD/dblocal-$arch.AppImage"
  fi

  workdir="$(mktemp -d)"

  local appdir="$workdir/DBLocal.AppDir"
  assemble_appdir "$appdir" "$binary"

  pack_appimage "$appdir" "$output"

  printf 'AppImage written: %s\n' "$output"
  printf 'Reminder: the AppImage needs a working host Vulkan driver (see docs/release/RELEASING.md).\n'
}

assemble_appdir() {
  local appdir="$1"
  local binary="$2"

  mkdir -p \
    "$appdir/usr/bin" \
    "$appdir/usr/share/applications" \
    "$appdir/usr/share/icons/hicolor/scalable/apps"

  cp "$binary" "$appdir/usr/bin/dblocal"
  chmod 0755 "$appdir/usr/bin/dblocal"

  cp "$repo_root/packaging/linux/dblocal.desktop" "$appdir/usr/share/applications/dblocal.desktop"
  cp "$appdir/usr/share/applications/dblocal.desktop" "$appdir/dblocal.desktop"
  cp "$repo_root/packaging/linux/dblocal.svg" "$appdir/usr/share/icons/hicolor/scalable/apps/dblocal.svg"
  cp "$appdir/usr/share/icons/hicolor/scalable/apps/dblocal.svg" "$appdir/dblocal.svg"

  # AppRun: resolve the payload directory even when the AppImage is mounted at a
  # path containing spaces, then hand off to the real binary with all arguments.
  cat > "$appdir/AppRun" <<'APPRUN'
#!/bin/sh
set -eu
SELF="$(readlink -f "$0")"
HERE="${SELF%/*}"
exec "$HERE/usr/bin/dblocal" "$@"
APPRUN
  chmod 0755 "$appdir/AppRun"
}

pack_appimage() {
  local appdir="$1"
  local output="$2"

  local tool="${APPIMAGETOOL:-}"
  if [ -z "$tool" ]; then
    if command -v appimagetool >/dev/null 2>&1; then
      tool="$(command -v appimagetool)"
    else
      printf 'error: appimagetool not found. Set APPIMAGETOOL=/path/to/appimagetool\n' >&2
      printf '       or download it from https://github.com/AppImage/appimagetool/releases\n' >&2
      exit 1
    fi
  fi

  # ARCH must be exported for appimagetool to skip an interactive prompt.
  local arch
  case "$(uname -m)" in
    x86_64|amd64) arch="x86_64" ;;
    aarch64|arm64) arch="aarch64" ;;
    *) arch="$(uname -m)" ;;
  esac

  if [ -f "$tool" ] && [ -x "$tool" ] && [ "${tool##*.}" = "AppImage" ]; then
    ARCH="$arch" "$tool" --appimage-extract-and-run --no-appstream "$appdir" "$output"
  else
    ARCH="$arch" "$tool" --no-appstream "$appdir" "$output"
  fi
}

main "$@"
