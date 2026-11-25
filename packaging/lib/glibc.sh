#!/usr/bin/env bash
# Shared glibc floor computation for DBLocal packaging scripts.
# Sourced by packaging/{deb,rpm}/build scripts. Do not execute directly.
#
# Uses readelf because it parses ELF headers regardless of the host binutils
# target support, so an x86_64 runner can read an AArch64 release binary.

# dblocal_glibc_floor <binary>
# Prints the maximum GLIBC symbol version required by <binary> (e.g. "2.43").
# Exits 1 with a clear error if readelf is missing or no GLIBC version is found.
dblocal_glibc_floor() {
  local bin="$1" max
  if ! command -v readelf >/dev/null 2>&1; then
    echo "error: required tool not found: readelf (needed to compute libc floor)" >&2
    return 1
  fi
  # "Version needs section" lists required symbol versions per library.
  # We take the maximum GLIBC_* version across all needed objects.
  max="$(readelf -V "$bin" 2>/dev/null | grep -oE 'GLIBC_[0-9]+(\.[0-9]+)+' | sort -V | tail -n 1)"
  if [ -z "$max" ]; then
    echo "error: could not determine GLIBC symbol versions from $bin (readelf -V produced no GLIBC_*)" >&2
    return 1
  fi
  printf '%s' "${max#GLIBC_}"
}

# dblocal_glibc_check_max <binary> <max_allowed>
# Verifies that <binary>'s maximum required GLIBC symbol version is <= <max_allowed>.
# Prints an informative status line to stderr and returns 0 on pass, 1 on failure.
dblocal_glibc_check_max() {
  local bin="$1" max_allowed="$2" floor highest
  if [ -z "$bin" ] || [ -z "$max_allowed" ]; then
    echo "error: usage: dblocal_glibc_check_max <binary> <max_allowed>" >&2
    return 2
  fi
  floor="$(dblocal_glibc_floor "$bin")" || return 1
  highest="$(printf '%s\n%s\n' "$floor" "$max_allowed" | sort -V | tail -n 1)"
  if [ "$highest" != "$max_allowed" ]; then
    echo "error: GLIBC floor regression on $bin: requires GLIBC_${floor} (max allowed: GLIBC_${max_allowed})" >&2
    return 1
  fi
  echo "ok: GLIBC floor on $bin is GLIBC_${floor} (<= GLIBC_${max_allowed})" >&2
  return 0
}
