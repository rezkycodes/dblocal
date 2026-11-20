#!/usr/bin/env bash
# Valkey spike: download both Ubuntu builds, SHA-256, layout/strip_prefix, ldd, glibc floor.
# Answers ASSUMPTION-02: does a jammy/noble tarball run on this Fedora machine?
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=../lib/common.sh
. "$SCRIPT_DIR/../lib/common.sh"

VK_VER="${VK_VER:-9.2.0}"
BASE="https://download.valkey.io/releases"

WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/dblocal-vk-dl-XXXXXX")"
cleanup() { rm -rf "$WORKDIR"; }
trap cleanup EXIT

log_section "host glibc"
log_kv "glibc version" "$(ldd --version | head -1 | awk '{print $NF}')"
log_kv "distro" "$(. /etc/os-release && echo "$PRETTY_NAME")"
log_kv "kernel" "$(uname -r)"

for variant in noble jammy; do
  asset="valkey-${VK_VER}-${variant}-x86_64.tar.gz"
  url="$BASE/$asset"
  sha_url="$url.sha256"
  cache="$CACHE_DIR/$asset"
  log_section "Valkey $VK_VER $variant"

  t0="$(now_ms)"
  fetch_verify "$url" "$sha_url" "$cache"
  t1="$(now_ms)"
  log_kv "download/verify ms" "$((t1-t0))"
  log_kv "archive size bytes" "$(stat -c '%s' "$cache")"

  listing="$WORKDIR/$variant.list"
  tar -tzf "$cache" > "$listing"
  log_kv "archive entries" "$(wc -l < "$listing")"
  echo "--- tar listing (first 25) ---"
  sed -n '1,25p' "$listing"
  top="$(sed -n '1p' "$listing" | sed 's#/.*##')"
  log_kv "top-level dir" "$top"
  log_kv "strip_prefix (recommended)" "$top/"

  out="$WORKDIR/$variant"
  mkdir -p "$out"
  tar -xzf "$cache" -C "$out"
  echo "--- tree depth 2 ---"
  (cd "$out" && find . -maxdepth 2 | sort | sed -n '1,40p')

  bindir="$out/$top/bin"
  echo "--- key binaries ---"
  find "$bindir" -maxdepth 1 -printf '%M %u %g %s %TY-%Tm-%Td %TH:%TM %f\n' 2>/dev/null | sort | sed -n '1,25p'
  echo "--- valkey-server --version ---"
  "$bindir/valkey-server" --version 2>&1 || true
  echo "--- valkey-cli bundled? ---"
  if [ -x "$bindir/valkey-cli" ]; then
    "$bindir/valkey-cli" --version
  else
    echo "valkey-cli NOT present in $bindir"
    find "$out/$top" -name 'valkey-cli*' -o -name 'redis-cli*' | sed -n '1,10p'
  fi
  echo "--- ldd valkey-server ---"
  ldd "$bindir/valkey-server" 2>&1 || true
  missing=$(ldd "$bindir/valkey-server" 2>&1 | grep -c 'not found' || true)
  log_kv "ldd missing libs" "$missing"
  echo "--- GLIBC symbol versions required (max) ---"
  readelf -V "$bindir/valkey-server" 2>/dev/null | grep -o 'GLIBC_[0-9.]*' | sort -Vu | tail -6 || true
  echo "--- GLIBCXX / CXXABI (C++ runtime needs) ---"
  readelf -V "$bindir/valkey-server" 2>/dev/null | grep -oE 'GLIBCXX_[0-9.]+|CXXABI_[0-9.]+' | sort -Vu | tail -6 || true
  echo "--- bundled libs? ---"
  find "$out/$top" -name '*.so*' | sed -n '1,20p'
  echo "--- du ---"
  du -sh "$out/$top"
  echo "--- can it actually start? (foreground --version already ran above) ---"
  echo "note: full start/PING is in 02_lifecycle.sh"
done

log_section "done"