#!/usr/bin/env bash
# PostgreSQL spike: verbatim failure texts + exit codes for F-LOG-02 diagnosis,
# plus postmaster.pid semantics on unclean shutdown and pg_isready exit codes.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=../lib/common.sh
. "$SCRIPT_DIR/../lib/common.sh"

PG18_VER="18.7.0"
PG17_VER="17.12.0"
extract_pg() {
  local ver="$1" dest="$2"
  local archive="$CACHE_DIR/postgresql-${ver}-x86_64-unknown-linux-gnu.tar.gz"
  [ -f "$archive" ] || { echo "missing $archive, run 01 first" >&2; exit 1; }
  mkdir -p "$dest"
  tar -xzf "$archive" -C "$dest"
  echo "$dest/postgresql-${ver}-x86_64-unknown-linux-gnu/bin"
}

WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/dblocal-pg-fail-XXXXXX")"
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

BIN18="$(extract_pg "$PG18_VER" "$WORKDIR/pg18")"
BIN17="$(extract_pg "$PG17_VER" "$WORKDIR/pg17")"

init_cluster() {
  local bin="$1" datadir="$2" port="$3" sockdir="$4"
  rm -rf "$datadir"; mkdir -p "$datadir" "$sockdir"
  "$bin/initdb" -D "$datadir" -U postgres --encoding=UTF8 --locale=C.UTF-8 \
    --auth-local=trust --auth-host=trust >"$datadir.init.log" 2>&1
  cat >> "$datadir/postgresql.conf" <<EOF
listen_addresses = '127.0.0.1'
port = $port
unix_socket_directories = '$sockdir'
EOF
}

run_case() {
  local title="$1"; shift
  log_section "$title"
  set +e
  out="$("$@" 2>&1)"
  rc=$?
  set -e
  log_kv "exit code" "$rc"
  echo "--- verbatim output ---"
  printf '%s\n' "$out"
  echo "--- end ---"
}

log_section "0. baselines"
log_kv "postgres --version" "$("$BIN18/postgres" --version)"

log_section "1. port already in use"
DATA1="$WORKDIR/d1"; SOCK1="$WORKDIR/s1"; PORT1="$(pick_port 41100)"
init_cluster "$BIN18" "$DATA1" "$PORT1" "$SOCK1"
# occupy the port with a non-postgres listener
python3 - "$PORT1" <<'PY' &
import socket, sys, time
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", int(sys.argv[1]))); s.listen(1)
time.sleep(25)
PY
HOLDER=$!
PIDS+=("$HOLDER")
wait_tcp 127.0.0.1 "$PORT1" 5000 || echo "holder not listening yet"
echo "holder listener:"; ss -ltn | grep ":$PORT1" || true
run_case "pg_ctl start on occupied port" "$BIN18/pg_ctl" -D "$DATA1" -l "$DATA1/fail.log" -w -t 15 start
echo "--- server log (verbatim) ---"
sed -n '1,25p' "$DATA1/fail.log"
echo "--- secondary try: direct postgres, foreground ---"
run_case "postgres -D on occupied port (foreground)" "$BIN18/postgres" -D "$DATA1"
echo "--- server log after direct attempt ---"
sed -n '1,25p' "$DATA1/fail.log"
kill "$HOLDER" 2>/dev/null || true
PIDS=()

log_section "2. data dir created by another major version"
DATA2="$WORKDIR/d2"; SOCK2="$WORKDIR/s2"; PORT2="$(pick_port 41110)"
init_cluster "$BIN17" "$DATA2" "$PORT2" "$SOCK2"
log_kv "PG_VERSION in datadir" "$(cat "$DATA2/PG_VERSION")"
echo "--- pg_ctl 18 start against PG17 datadir ---"
run_case "pg_ctl (18) start on 17 datadir" "$BIN18/pg_ctl" -D "$DATA2" -l "$DATA2/mix.log" -w -t 15 start
echo "--- server log (verbatim) ---"
sed -n '1,25p' "$DATA2/mix.log"
echo "--- psql 18 against a 17 server later (catalog version mismatch) n/a ---"

log_section "3. data dir permission denied"
DATA3="$WORKDIR/d3"; SOCK3="$WORKDIR/s3"; PORT3="$(pick_port 41120)"
init_cluster "$BIN18" "$DATA3" "$PORT3" "$SOCK3"
chmod 000 "$DATA3"
echo "owner/mode: $(stat -c '%U %a' "$DATA3")"
if [ "$(id -u)" = "0" ]; then
  echo "skipped: running as root, chmod does not deny"
else
  run_case "pg_ctl start on 000 datadir" "$BIN18/pg_ctl" -D "$DATA3" -l "$WORKDIR/denied.log" -w -t 15 start
  echo "--- server log (verbatim) — note: log file itself may be unwritable, so pg_ctl writes to stderr ---"
  sed -n '1,20p' "$WORKDIR/denied.log" 2>/dev/null || echo "(no log file)"
fi
chmod 700 "$DATA3"

log_section "3b. specific unwritable file: postmaster.pid present but data dir read-only"
DATA3B="$WORKDIR/d3b"; SOCK3B="$WORKDIR/s3b"; PORT3B="$(pick_port 41121)"
init_cluster "$BIN18" "$DATA3B" "$PORT3B" "$SOCK3B"
chmod 500 "$DATA3B"
run_case "pg_ctl start on read-only (500) datadir" "$BIN18/pg_ctl" -D "$DATA3B" -l "$WORKDIR/ro.log" -w -t 15 start
sed -n '1,20p' "$WORKDIR/ro.log" 2>/dev/null || true
chmod 700 "$DATA3B"

log_section "4. missing binary"
echo "--- missing postgres binary ---"
run_case "pg_ctl from a bindir with no postgres" env PATH="/nonexistent" "$BIN18/pg_ctl" -D "$WORKDIR/nodir" -w start
echo "--- direct exec of a non-existent binary ---"
set +e
out="$("$WORKDIR/pg18/postgresql-${PG18_VER}-x86_64-unknown-linux-gnu/bin/postgres-missing" 2>&1)"; rc=$?
set -e
log_kv "exit code (missing bin, via shell)" "$rc"
printf '%s\n' "$out"
echo "--- truncated (zero-byte) postgres binary ---"
BROKEN="$WORKDIR/broken-bin"
mkdir -p "$BROKEN"
cp -a "$BIN18/." "$BROKEN/"
: > "$BROKEN/postgres"
run_case "corrupt postgres (empty file)" "$BROKEN/postgres" --version

log_section "5. unix socket dir missing"
DATA5="$WORKDIR/d5"; SOCK5="$WORKDIR/s5"; PORT5="$(pick_port 41130)"
init_cluster "$BIN18" "$DATA5" "$PORT5" "$SOCK5"
rm -rf "$SOCK5"
run_case "pg_ctl start with a missing socket dir" "$BIN18/pg_ctl" -D "$DATA5" -l "$WORKDIR/nosock.log" -w -t 15 start
sed -n '1,20p' "$WORKDIR/nosock.log" 2>/dev/null || true
mkdir -p "$SOCK5"

log_section "6. postmaster.pid on unclean shutdown (kill -9) — full detail"
DATA6="$WORKDIR/d6"; SOCK6="$WORKDIR/s6"; PORT6="$(pick_port 41140)"
init_cluster "$BIN18" "$DATA6" "$PORT6" "$SOCK6"
"$BIN18/pg_ctl" -D "$DATA6" -l "$DATA6/log" -w -t 30 start
PID6="$(head -1 "$DATA6/postmaster.pid")"
PIDS+=("$PID6")
echo "--- clean postmaster.pid (8 lines) ---"
nl -ba "$DATA6/postmaster.pid"
"$BIN18/psql" -h 127.0.0.1 -p "$PORT6" -U postgres -c 'select 1' >/dev/null
kill -9 "$PID6"
sleep 0.3
echo "--- postmaster.pid right after kill -9 ---"
nl -ba "$DATA6/postmaster.pid"
echo "--- pg_ctl status after kill -9 ---"
set +e; "$BIN18/pg_ctl" -D "$DATA6" status; echo "status rc=$?"; set -e
echo "--- stale pidfile PID reuse risk: is the pid still alive? ---"
[ -d "/proc/$PID6" ] && echo "pid $PID6 alive (reused!)" || echo "pid $PID6 gone"

log_section "7. pg_isready exit codes"
DATA7="$WORKDIR/d7"; SOCK7="$WORKDIR/s7"; PORT7="$(pick_port 41150)"
init_cluster "$BIN18" "$DATA7" "$PORT7" "$SOCK7"
echo "--- server NOT running ---"
set +e
"$BIN18/pg_isready" -h 127.0.0.1 -p "$PORT7"; echo "rc=$? (expect 2: no response)"
"$BIN18/pg_isready" -h 127.0.0.1 -p "$PORT7" -t 1; echo "rc=$? with -t 1"
set -e
"$BIN18/pg_ctl" -D "$DATA7" -l "$DATA7/log" -w -t 30 start
PID7="$(head -1 "$DATA7/postmaster.pid")"; PIDS+=("$PID7")
echo "--- server running, valid db ---"
set +e
"$BIN18/pg_isready" -h 127.0.0.1 -p "$PORT7" -U postgres -d postgres; echo "rc=$? (expect 0: accepting)"
echo "--- server running, nonexistent db ---"
"$BIN18/pg_isready" -h 127.0.0.1 -p "$PORT7" -U postgres -d nosuchdb; echo "rc=$? (expect 1: rejecting)"
echo "--- bogus host ---"
"$BIN18/pg_isready" -h 192.0.2.1 -p "$PORT7" -t 1; echo "rc=$? (expect 2)"
set -e
"$BIN18/pg_ctl" -D "$DATA7" -m fast -w -t 20 stop
PIDS=()

log_section "8. initdb into a non-empty / existing datadir"
DATA8="$WORKDIR/d8"
mkdir -p "$DATA8"; echo junk > "$DATA8/junk.txt"
run_case "initdb on non-empty dir" "$BIN18/initdb" -D "$DATA8" -U postgres
echo "--- initdb on a datadir already initialised (second run) ---"
DATA8B="$WORKDIR/d8b"
"$BIN18/initdb" -D "$DATA8B" -U postgres --encoding=UTF8 --locale=C.UTF-8 --auth-local=trust --auth-host=trust >/dev/null 2>&1
run_case "initdb twice on the same dir" "$BIN18/initdb" -D "$DATA8B" -U postgres
echo "--- initdb with an unsupported locale ---"
run_case "initdb --locale=xx_YY.UTF-8" "$BIN18/initdb" -D "$WORKDIR/d8c" -U postgres --encoding=UTF8 --locale=xx_YY.UTF-8

log_section "9. default unix socket directory when unset (fresh initdb, nothing configured)"
DATA9="$WORKDIR/d9"; PORT9="$(pick_port 41160)"
rm -rf "$DATA9"; mkdir -p "$DATA9"
"$BIN18/initdb" -D "$DATA9" -U postgres --encoding=UTF8 --locale=C.UTF-8 \
  --auth-local=trust --auth-host=trust >/dev/null 2>&1
cat >> "$DATA9/postgresql.conf" <<EOF
listen_addresses = '127.0.0.1'
port = $PORT9
EOF
echo "--- grep default in shipped postgresql.conf ---"
grep -n '^#unix_socket_directories' "$DATA9/postgresql.conf"
"$BIN18/pg_ctl" -D "$DATA9" -l "$DATA9/log" -w -t 30 start
PID9="$(head -1 "$DATA9/postmaster.pid")"; PIDS+=("$PID9")
echo "--- postmaster.pid line 5 tells us the live socket dir ---"
nl -ba "$DATA9/postmaster.pid"
echo "--- sockets actually present in /tmp for port $PORT9 ---"
ls -la /tmp/.s.PGSQL."$PORT9"* 2>/dev/null || echo "(none in /tmp)"
"$BIN18/psql" -h /tmp -p "$PORT9" -U postgres -Atc "select current_setting('unix_socket_directories');"
"$BIN18/pg_ctl" -D "$DATA9" -m fast -w -t 20 stop
PIDS=()
rm -f /tmp/.s.PGSQL."$PORT9" /tmp/.s.PGSQL."$PORT9".lock 2>/dev/null || true

log_section "done"