#!/usr/bin/env bash
# PostgreSQL spike: download, SHA-256 verification, archive layout, ldd.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=../lib/common.sh
. "$SCRIPT_DIR/../lib/common.sh"

WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/dblocal-pg-dl-XXXXXX")"
cleanup() {
  rm -rf "$WORKDIR"
}
trap cleanup EXIT

PG18_VER="18.7.0"
PG17_VER="17.12.0"
BASE="https://github.com/theseus-rs/postgresql-binaries/releases/download"

for tag in "$PG18_VER" "$PG17_VER"; do
  log_section "PostgreSQL $tag download+layout"
  asset="postgresql-${tag}-x86_64-unknown-linux-gnu.tar.gz"
  url="$BASE/$tag/$asset"
  sha_url="$url.sha256"
  cache="$CACHE_DIR/$asset"

  start="$(now_ms)"
  fetch_verify "$url" "$sha_url" "$cache"
  end="$(now_ms)"
  log_kv "download/verify ms" "$((end-start))"
  log_kv "archive size bytes" "$(stat -c '%s' "$cache")"

  listing="$WORKDIR/$tag.list"
  tar -tzf "$cache" > "$listing"
  log_kv "archive entries" "$(wc -l < "$listing")"
  log_section "tar listing (top 20) $tag"
  sed -n '1,20p' "$listing"
  top="$(sed -n '1p' "$listing" | sed 's#/.*##')"
  log_kv "top-level dir" "$top"
  log_kv "strip_prefix (recommended)" "$top/"

  out="$WORKDIR/$tag"
  mkdir -p "$out"
  tar -xzf "$cache" -C "$out"
  echo "--- extracted tree depth 2 ---"
  (cd "$out" && find . -maxdepth 2 -type d | sort)

  bindir="$out/$top/bin"
  echo "--- bin listing (key tools) ---"
  for name in postgres initdb pg_ctl pg_isready psql createdb createuser; do
    [ -e "$bindir/$name" ] && ls -la "$bindir/$name"
  done || true
  echo "--- ldd bin/postgres ---"
  ldd "$bindir/postgres" 2>&1 || true
  echo "--- ldd not-found count ---"
  missing=$(ldd "$bindir/postgres" 2>&1 | grep -c 'not found' || true)
  log_kv "ldd missing libs" "$missing"
  echo "--- bin/postgres --version ---"
  "$bindir/postgres" --version
  echo "--- initdb --version ---"
  "$bindir/initdb" --version
  echo "--- pg_ctl --version ---"
  "$bindir/pg_ctl" --version
  echo "--- psql --version ---"
  "$bindir/psql" --version
  echo "--- pg_isready --version ---"
  "$bindir/pg_isready" --version
  echo "--- valkey/redis note: n/a ---"
done

log_section "done"