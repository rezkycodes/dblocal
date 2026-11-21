#!/usr/bin/env bash
# Shared version helpers for DBLocal packaging scripts.
# Sourced by packaging/{deb,rpm,msi}/build scripts. Do not execute directly.
#
# SemVer input accepted (leading v stripped): MAJOR.MINOR.PATCH[-PRERELEASE]
# Package version mapping (F-PKG fix): prerelease '-' becomes '~' so that
# prereleases sort BEFORE the final release in both dpkg and rpm.
#   0.2.0-rc.1 -> 0.2.0~rc.1  (package version only; tag/file name unchanged)

# dblocal_version_strip_v: strip a single leading v.
dblocal_version_strip_v() {
  printf '%s' "${1#v}"
}

# dblocal_version_is_valid: 0 if valid SemVer-like package input after v-strip.
dblocal_version_is_valid() {
  local v
  v="$(dblocal_version_strip_v "$1")"
  printf '%s' "$v" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([-~][0-9A-Za-z.]+)?$'
}

# dblocal_version_require_valid <raw> <context>
# Exits 2 with a clear one-line message if <raw> is not a valid explicit version.
dblocal_version_require_valid() {
  local raw="$1" ctx="${2:-version}"
  local v
  v="$(dblocal_version_strip_v "$raw")"
  if dblocal_version_is_valid "$v"; then
    return 0
  fi
  echo "error: invalid ${ctx} '$raw': expected MAJOR.MINOR.PATCH with optional [-~]prerelease (e.g. 0.2.0, 0.2.0-rc.1)" >&2
  exit 2
}

# dblocal_version_package <raw>
# Prints the mapped package version (prerelease '-' -> '~'). Requires valid input.
dblocal_version_package() {
  local v
  v="$(dblocal_version_strip_v "$1")"
  if ! dblocal_version_is_valid "$v"; then
    echo "error: invalid version '$1'" >&2
    return 2
  fi
  # Map only the first '-' that introduces the prerelease segment to '~'.
  # The validated shape has at most one prerelease separator group.
  if printf '%s' "$v" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+-'; then
    printf '%s' "$v" | sed 's/-/~/'
  else
    printf '%s' "$v"
  fi
}

# dblocal_version_default_or_validate <explicit_or_empty> <repo_root> <var_raw> <var_pkg> <var_is_snapshot>
# Resolves version from explicit arg or git describe default.
# - explicit: validated strictly; snapshot=0
# - default with SemVer tag: snapshot=0
# - default with no tag / unparseable describe: snapshot=1, pkg=0.0.0~git<short>
# Sets named variables in caller scope.
dblocal_version_default_or_validate() {
  local explicit="$1" repo_root="$2" _out_raw="$3" _out_pkg="$4" _out_snap="$5"
  local raw pkg snap
  snap=0
  if [ -n "$explicit" ]; then
    dblocal_version_require_valid "$explicit" "version"
    raw="$(dblocal_version_strip_v "$explicit")"
    pkg="$(dblocal_version_package "$raw")"
  else
    raw="$(git -C "$repo_root" describe --tags --always 2>/dev/null || true)"
    raw="$(printf '%s' "$raw" | tr -d '[:space:]')"
    if [ -z "$raw" ]; then
      raw="0.0.0~gitunknown"
      pkg="$raw"
      snap=1
    else
      local stripped
      stripped="$(dblocal_version_strip_v "$raw")"
      if dblocal_version_is_valid "$stripped"; then
        raw="$stripped"
        pkg="$(dblocal_version_package "$raw")"
      else
        # Untagged/unparseable describe: snapshot fallback. Keep a short hash.
        local short
        short="$(git -C "$repo_root" rev-parse --short=12 HEAD 2>/dev/null || printf '%s' "$stripped" | tr -cd '0-9A-Za-z' | cut -c1-12)"
        raw="0.0.0~git${short}"
        pkg="$raw"
        snap=1
        echo "info: no SemVer tag describes HEAD; using snapshot package version $pkg" >&2
      fi
    fi
  fi
  printf -v "$_out_raw" '%s' "$raw"
  printf -v "$_out_pkg" '%s' "$pkg"
  printf -v "$_out_snap" '%s' "$snap"
}
