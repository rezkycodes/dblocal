#!/usr/bin/env bash
# Shared release asset filename normalization and validation.
# GitHub Releases rewrites characters outside [A-Za-z0-9._-] (notably '~' -> '.')
# on upload, which breaks checksums.txt verification if filenames are not
# normalized before checksum generation.
# Sourced by release workflow steps and packaging tests. Do not execute directly.

# dblocal_asset_check_name <path-or-basename>
# Returns 0 if the basename contains only [A-Za-z0-9._-], otherwise prints an
# error to stderr and returns 1.
dblocal_asset_check_name() {
  local base="${1##*/}"
  if [ -z "$base" ] || [[ ! "$base" =~ ^[A-Za-z0-9._-]+$ ]]; then
    echo "error: release asset filename contains characters outside [A-Za-z0-9._-]: ${base}" >&2
    return 1
  fi
  return 0
}

# dblocal_asset_normalize_dir <dir>
# Renames any regular file in <dir> whose basename contains '~' by replacing '~'
# with '.', then verifies every file in <dir> passes dblocal_asset_check_name.
# Does not modify package metadata inside .deb/.rpm files.
dblocal_asset_normalize_dir() {
  local dir="$1" f base norm
  if [ ! -d "$dir" ]; then
    echo "error: asset directory not found: $dir" >&2
    return 1
  fi
  for f in "$dir"/*; do
    [ -f "$f" ] || continue
    base="${f##*/}"
    norm="${base//\~/.}"
    if [ "$norm" != "$base" ]; then
      if [ -e "$dir/$norm" ]; then
        echo "error: normalized asset name collides with existing file: $dir/$norm" >&2
        return 1
      fi
      mv -- "$f" "$dir/$norm"
      echo "info: normalized release asset filename: ${base} -> ${norm}" >&2
      base="$norm"
    fi
    dblocal_asset_check_name "$base" || return 1
  done
  return 0
}
