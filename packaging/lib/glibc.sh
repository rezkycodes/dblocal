#!/usr/bin/env bash
# Shared glibc floor computation for DBLocal packaging scripts.
# Sourced by packaging/{deb,rpm}/build scripts. Do not execute directly.
#
# Uses readelf (not objdump) because readelf parses ELF headers regardless of
# host binutils target support; objdump on a stock x86_64 Ubuntu cannot read
# AArch64 release binaries, which breaks arm64 package jobs in CI.

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
