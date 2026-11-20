#!/usr/bin/env bash
# PostgreSQL spike: initdb flags, auth modes, locale, unix socket directory and the
# 107-byte AF_UNIX path limit.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=../lib/common.sh
. "$SCRIPT_DIR/../lib/common.sh"

PG_VER="${PG_VER:-18.7.0}"
ARCHIVE="$CACHE_DIR/postgresql-${PG_VER}-x86_64-unknown-linux-gnu.tar.gz"
[ -f "$ARCHIVE" ] || { echo "missing $ARCHIVE, run 01_download_layout.sh first" >&2; exit 1; }

WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/dblocal-pg-init-XXXXXX")"
cleanup() {
  rm -rf "$WORKDIR"
}
trap cleanup EXIT

TOP="postgresql-${PG_VER}-x86_64-unknown-linux-gnu"
tar -xzf "$ARCHIVE" -C "$WORKDIR"
BIN="$WORKDIR/$TOP/bin"

log_section "locale inventory"
locale -a | grep -iE '^(c|C)\.?(utf-?8)?$' || true
log_kv "LANG" "${LANG:-<unset>}"

log_section "A. initdb trust auth, --locale=C.UTF-8"
DATA_TRUST="$WORKDIR/data-trust"
t0="$(now_ms)"
"$BIN/initdb" -D "$DATA_TRUST" \
  -U postgres \
  --encoding=UTF8 \
  --locale=C.UTF-8 \
  --auth-local=trust \
  --auth-host=trust \
  >"$WORKDIR/initdb-trust.log" 2>&1
t1="$(now_ms)"
log_kv "initdb ms (trust)" "$((t1-t0))"
log_kv "initdb exit" "0"
tail -12 "$WORKDIR/initdb-trust.log"
log_kv "data dir size" "$(du -sh "$DATA_TRUST" | awk '{print $1}')"

log_section "A2. resulting locale settings"
grep -E '^(lc_|default_text_search)' "$DATA_TRUST/postgresql.conf" | sed 's/^#//'
"$BIN/pg_controldata" "$DATA_TRUST" | grep -iE 'encoding|collate|ctype|locale|checksum|block size|database cluster state'
echo "--- PG_VERSION ---"
cat "$DATA_TRUST/PG_VERSION"

log_section "A3. pg_hba.conf produced"
grep -vE '^\s*#|^\s*$' "$DATA_TRUST/pg_hba.conf"

log_section "B. initdb scram-sha-256 with --pwfile"
DATA_SCRAM="$WORKDIR/data-scram"
PW_FILE="$WORKDIR/pwfile"
umask 077
printf 'spike-%s\n' "$(head -c 12 /dev/urandom | base64 | tr -d '/+=' | head -c 16)" > "$PW_FILE"
umask 022
t0="$(now_ms)"
"$BIN/initdb" -D "$DATA_SCRAM" \
  -U postgres \
  --encoding=UTF8 \
  --locale=C.UTF-8 \
  --auth-local=scram-sha-256 \
  --auth-host=scram-sha-256 \
  --pwfile="$PW_FILE" \
  >"$WORKDIR/initdb-scram.log" 2>&1
t1="$(now_ms)"
log_kv "initdb ms (scram)" "$((t1-t0))"
tail -8 "$WORKDIR/initdb-scram.log"
echo "--- pg_hba.conf (scram) ---"
grep -vE '^\s*#|^\s*$' "$DATA_SCRAM/pg_hba.conf"

log_section "B2. password verifier (must be SCRAM-SHA-256, never the plaintext)"
( cd "$DATA_SCRAM" && "$BIN/pg_controldata" . >/dev/null )
if [ -x "$BIN/pg_ctl" ]; then
  echo "note: password verifier inspected after start in 03_lifecycle.sh"
fi
echo "pwfile mode: $(stat -c '%a' "$PW_FILE")"

log_section "C. alternative locale spellings on this machine"
for loc in C.UTF-8 C.utf8 en_US.UTF-8 C POSIX; do
  out="$WORKDIR/loc-$(echo "$loc" | tr './' '__')"
  if "$BIN/initdb" -D "$out" -U postgres --encoding=UTF8 --locale="$loc" \
      --auth-local=trust --auth-host=trust >"$WORKDIR/loc.log" 2>&1; then
    printf '%-12s OK\n' "$loc"
    rm -rf "$out"
  else
    printf '%-12s FAIL: %s\n' "$loc" "$(grep -m1 -E 'initdb:|error|FATAL' "$WORKDIR/loc.log" || head -1 "$WORKDIR/loc.log")"
  fi
done

log_section "D. unix socket directory + 107-byte AF_UNIX limit"
SOCKDIR="$WORKDIR/sock"
mkdir -p "$SOCKDIR"
PORT="$(pick_port 41000)"
assert_spike_port "$PORT"
echo "--- postgresql.conf: default unix_socket_directories ---"
grep -n 'unix_socket_directories' "$DATA_TRUST/postgresql.conf"
echo "--- what initdb leaves in pg_ctl's default socket dir ---"
echo "  (computed default; postgres falls back to /tmp when unset and /var/run/postgresql is not writable)"
log_kv "configured socket dir" "$SOCKDIR"
cat >> "$DATA_TRUST/postgresql.conf" <<EOF
listen_addresses = '127.0.0.1'
port = $PORT
unix_socket_directories = '$SOCKDIR'
EOF
"$BIN/pg_ctl" -D "$DATA_TRUST" -l "$WORKDIR/sock.log" -w -t 30 start >"$WORKDIR/sockstart.log" 2>&1 || {
  echo "start failed:"
  cat "$WORKDIR/sockstart.log" "$WORKDIR/sock.log"
  exit 1
}
ls -la "$SOCKDIR"
log_kv "socket path length" "$(printf '%s' "$SOCKDIR/.s.PGSQL.$PORT" | wc -c)"
"$BIN/psql" -h "$SOCKDIR" -p "$PORT" -U postgres -d postgres -Atc 'select current_setting('"'"'unix_socket_directories'"'"'), version();'
"$BIN/pg_ctl" -D "$DATA_TRUST" -m fast -w -t 20 stop

log_section "D2. long socket dir (>107 bytes) — expected to fail"
LONG="$WORKDIR/$(printf 'a%.0s' $(seq 1 70))/$(printf 'b%.0s' $(seq 1 70))"
mkdir -p "$LONG" 2>/dev/null || echo "(mkdir itself failed, path too long)"
echo "path bytes: $(printf '%s' "$LONG/.s.PGSQL.$PORT" | wc -c)"
rm -rf "$LONG"
LONGROOT="$WORKDIR/$(printf 'l%.0s' $(seq 1 100))"
mkdir -p "$LONGROOT"
DATA_LONG="$WORKDIR/data-long"
cp -a "$DATA_TRUST" "$DATA_LONG"
LONGDIR="$LONGROOT/sock"
mkdir -p "$LONGDIR"
LONGDIR_ABS="$(cd "$LONGDIR" && pwd -P)"
log_kv "long socket dir bytes" "$(printf '%s' "$LONGDIR_ABS" | wc -c)"
log_kv "long socket path bytes" "$(printf '%s' "$LONGDIR_ABS/.s.PGSQL.$PORT" | wc -c)"
sed -i "s#^unix_socket_directories = .*#unix_socket_directories = '$LONGDIR_ABS'#" "$DATA_LONG/postgresql.conf"
set +e
LONG_OUT="$("$BIN/pg_ctl" -D "$DATA_LONG" -l "$WORKDIR/long.log" -w -t 15 start 2>&1)"
LONG_RC=$?
set -e
log_kv "pg_ctl exit (long path)" "$LONG_RC"
echo "--- pg_ctl stdout/stderr ---"
printf '%s\n' "$LONG_OUT"
echo "--- postgres log (relevant lines) ---"
grep -iE 'socket|too long|FATAL|could not' "$WORKDIR/long.log" | head -20 || true
"$BIN/pg_ctl" -D "$DATA_LONG" -m immediate stop >/dev/null 2>&1 || true

log_section "E. postmaster.pid from the trust instance (last known good)"
echo "(populated in 03_lifecycle.sh; content shown there)"

log_section "done"