#!/usr/bin/env bash
# Supplemental PostgreSQL spike: stale pidfile with a LIVE unrelated PID (PID reuse),
# pidfile pointing at a dead PID, data dir path that is a file, pg_ctl stop timing
# with no connections, multiple socket dirs, and the ::1-only listen case.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/../lib/common.sh"

PG_VER="${PG_VER:-18.7.0}"
ARCHIVE="$CACHE_DIR/postgresql-${PG_VER}-x86_64-unknown-linux-gnu.tar.gz"
[ -f "$ARCHIVE" ] || { echo "missing $ARCHIVE, run 01 first" >&2; exit 1; }

WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/dblocal-pg-pid-XXXXXX")"
PIDS=()
cleanup() {
  set +e
  for pid in "${PIDS[@]:-}"; do
    [ -n "${pid:-}" ] && [ -d "/proc/$pid" ] && kill -TERM "$pid" 2>/dev/null
  done
  sleep 0.2
  for pid in "${PIDS[@]:-}"; do
    [ -n "${pid:-}" ] && [ -d "/proc/$pid" ] && kill -9 "$pid" 2>/dev/null
  done
  rm -rf "$WORKDIR"
}
trap cleanup EXIT

tar -xzf "$ARCHIVE" -C "$WORKDIR"
BIN="$WORKDIR/postgresql-${PG_VER}-x86_64-unknown-linux-gnu/bin"

mkinst() {
  local name="$1"
  local d="$WORKDIR/$name"
  mkdir -p "$d/data" "$d/sock"
  "$BIN/initdb" -D "$d/data" -U postgres --encoding=UTF8 --locale=C.UTF-8 \
    --auth-local=trust --auth-host=trust >"$d/init.log" 2>&1
  local p; p="$(pick_port "${2:-}")"
  cat >> "$d/data/postgresql.conf" <<EOF
listen_addresses = '127.0.0.1'
port = $p
unix_socket_directories = '$d/sock'
EOF
  echo "$d"
}

log_section "1. stale postmaster.pid whose PID belongs to a LIVE unrelated process (PID reuse)"
I1="$(mkinst i1 41400)"; PORT1="$(grep -E '^port ' "$I1/data/postgresql.conf" | awk '{print $3}')"
assert_spike_port "$PORT1"
sleep 600 & FAKE=$!
PIDS+=("$FAKE")
echo "victim (unrelated) pid=$FAKE exe=$(readlink /proc/$FAKE/exe)"
cat > "$I1/data/postmaster.pid" <<EOF
$FAKE
$I1/data
$(date +%s)
$PORT1
$I1/sock
127.0.0.1
  0        0
ready
EOF
echo "--- fabricated stale pidfile (line 1 is a live 'sleep') ---"
nl -ba "$I1/data/postmaster.pid"
echo "--- pg_ctl status (checks liveness only) ---"
set +e
"$BIN/pg_ctl" -D "$I1/data" status; echo "status rc=$?"
echo "--- pg_ctl start while that pidfile exists ---"
out="$("$BIN/pg_ctl" -D "$I1/data" -l "$I1/data/s.log" -w -t 15 start 2>&1)"; rc=$?
set -e
echo "start rc=$rc"
printf '%s\n' "$out"
if [ -f "$I1/data/postmaster.pid" ]; then
  echo "--- pidfile after start attempt ---"
  nl -ba "$I1/data/postmaster.pid"
  NEW="$(head -1 "$I1/data/postmaster.pid")"
  if [ "$NEW" != "$FAKE" ] && [ -d "/proc/$NEW" ]; then
    PIDS+=("$NEW")
    "$BIN/pg_ctl" -D "$I1/data" -m fast -w -t 20 stop >/dev/null 2>&1 || true
  fi
fi
echo "--- did the unrelated process get signalled? (it must not) ---"
if [ -d "/proc/$FAKE" ]; then echo "unrelated pid $FAKE STILL ALIVE (good)"; else echo "unrelated pid $FAKE was killed (BAD)"; fi
kill "$FAKE" 2>/dev/null || true
PIDS=()

log_section "2. pg_ctl stop timing with NO open connections"
I2="$(mkinst i2 41410)"
"$BIN/pg_ctl" -D "$I2/data" -l "$I2/data/log" -w -t 30 start >/dev/null
P2="$(head -1 "$I2/data/postmaster.pid")"; PIDS+=("$P2")
t0="$(now_ms)"; "$BIN/pg_ctl" -D "$I2/data" -m fast -w -t 20 stop >/dev/null; t1="$(now_ms)"
log_kv "fast stop ms (idle)" "$((t1-t0))"
echo "--- restart then measure -m immediate ---"
"$BIN/pg_ctl" -D "$I2/data" -l "$I2/data/log" -w -t 30 start >/dev/null
P2B="$(head -1 "$I2/data/postmaster.pid")"; PIDS+=("$P2B")
t0="$(now_ms)"; "$BIN/pg_ctl" -D "$I2/data" -m immediate -w -t 20 stop >/dev/null; t1="$(now_ms)"
log_kv "immediate stop ms" "$((t1-t0))"
echo "--- restart then measure smart (open connections would block forever) ---"
"$BIN/pg_ctl" -D "$I2/data" -l "$I2/data/log" -w -t 30 start >/dev/null
P2C="$(head -1 "$I2/data/postmaster.pid")"; PIDS+=("$P2C")
t0="$(now_ms)"; "$BIN/pg_ctl" -D "$I2/data" -m smart -w -t 20 stop >/dev/null; t1="$(now_ms)"
log_kv "smart stop ms (no conns)" "$((t1-t0))"
PIDS=()

log_section "3. data dir path that is a regular file"
echo "not a directory" > "$WORKDIR/notadir"
set +e
out="$("$BIN/pg_ctl" -D "$WORKDIR/notadir" -w -t 10 start 2>&1)"; rc=$?
set -e
log_kv "pg_ctl rc" "$rc"; printf '%s\n' "$out"
set +e
out="$("$BIN/initdb" -D "$WORKDIR/notadir" -U postgres 2>&1)"; rc=$?
set -e
log_kv "initdb rc" "$rc"; printf '%s\n' "$out" | tail -4

log_section "4. multiple unix socket directories (comma separated)"
I4="$(mkinst i4 41420)"; PORT4="$(grep -E '^port ' "$I4/data/postgresql.conf" | awk '{print $3}')"
mkdir -p "$I4/sock2"
# first dir is >107 bytes when appended to, so PG must skip it and use the second
LONGDIR="$WORKDIR/$(printf 'x%.0s' $(seq 1 90))/sock"
mkdir -p "$LONGDIR"
sed -i "s#^unix_socket_directories = .*#unix_socket_directories = '$LONGDIR,$I4/sock2'#" "$I4/data/postgresql.conf"
"$BIN/pg_ctl" -D "$I4/data" -l "$I4/data/log" -w -t 30 start
P4="$(head -1 "$I4/data/postmaster.pid")"; PIDS+=("$P4")
echo "--- postmaster.pid line 5 (what actually got used) ---"
sed -n '5p' "$I4/data/postmaster.pid"
echo "--- sockets in each dir ---"
ls -1 "$I4/sock2" 2>/dev/null
echo "--- connect via the working dir ---"
"$BIN/psql" -h "$I4/sock2" -p "$PORT4" -U postgres -Atc "show unix_socket_directories;"
echo "--- relevant log lines ---"
grep -iE 'socket|too long|skipp' "$I4/data/log" | head -6 || true
"$BIN/pg_ctl" -D "$I4/data" -m fast -w -t 20 stop >/dev/null
PIDS=()

log_section "5. listen_addresses = '::1' only"
I5="$(mkinst i5 41430)"; PORT5="$(grep -E '^port ' "$I5/data/postgresql.conf" | awk '{print $3}')"
sed -i "s/^listen_addresses = .*/listen_addresses = '::1'/" "$I5/data/postgresql.conf"
"$BIN/pg_ctl" -D "$I5/data" -l "$I5/data/log" -w -t 30 start
P5="$(head -1 "$I5/data/postmaster.pid")"; PIDS+=("$P5")
ss -ltnp | grep ":$PORT5" || true
echo "--- connect over IPv6 loopback ---"
"$BIN/psql" -h ::1 -p "$PORT5" -U postgres -Atc "select 'ipv6-ok', inet_server_addr();"
"$BIN/pg_ctl" -D "$I5/data" -m fast -w -t 20 stop >/dev/null
PIDS=()

log_section "6. pg_ctl start when the port is held but only on 127.0.0.1 (IPv6 free)"
echo "(covered: bind failure text already captured in 04_failures.sh)"

log_section "7. initdb auth-local=trust with auth-host=scram (mixed, what DBLocal will use)"
I7="$WORKDIR/i7"; mkdir -p "$I7/data" "$I7/sock"; PORT7="$(pick_port 41440)"
PW_FILE="$I7/pw"; umask 077; printf 'mixed-mode-pw-1234\n' > "$PW_FILE"; umask 022
"$BIN/initdb" -D "$I7/data" -U postgres --encoding=UTF8 --locale=C.UTF-8 \
  --auth-local=trust --auth-host=scram-sha-256 --pwfile="$PW_FILE" >/dev/null 2>&1
grep -vE '^\s*#|^\s*$' "$I7/data/pg_hba.conf"
cat >> "$I7/data/postgresql.conf" <<EOF
listen_addresses = '127.0.0.1'
port = $PORT7
unix_socket_directories = '$I7/sock'
EOF
"$BIN/pg_ctl" -D "$I7/data" -l "$I7/data/log" -w -t 30 start
P7="$(head -1 "$I7/data/postmaster.pid")"; PIDS+=("$P7")
echo "--- unix socket, no password (local trust) ---"
"$BIN/psql" -h "$I7/sock" -p "$PORT7" -U postgres -Atc "select 'local-trust-ok';"
echo "--- TCP with no password (host scram) ---"
set +e
"$BIN/psql" -w -h 127.0.0.1 -p "$PORT7" -U postgres -Atc "select 'tcp-no-pw';" 2>&1
echo "rc=$?"
echo "--- TCP with password ---"
PGPASSWORD='mixed-mode-pw-1234' "$BIN/psql" -h 127.0.0.1 -p "$PORT7" -U postgres -Atc "select 'tcp-pw-ok';"
set -e
"$BIN/pg_ctl" -D "$I7/data" -m fast -w -t 20 stop >/dev/null
PIDS=()

log_section "done"