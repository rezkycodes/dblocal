#!/usr/bin/env bash
# PostgreSQL spike: auth (scram verifier, bad password), connection URI, detached-spawn
# survival across parent-shell exit, postmaster.pid line 8 transitions, PID validation.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=../lib/common.sh
. "$SCRIPT_DIR/../lib/common.sh"

PG18_VER="18.7.0"
ARCHIVE="$CACHE_DIR/postgresql-${PG18_VER}-x86_64-unknown-linux-gnu.tar.gz"
[ -f "$ARCHIVE" ] || { echo "missing $ARCHIVE, run 01 first" >&2; exit 1; }

WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/dblocal-pg-auth-XXXXXX")"
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
BIN="$WORKDIR/postgresql-${PG18_VER}-x86_64-unknown-linux-gnu/bin"

DATA="$WORKDIR/data"
SOCK="$WORKDIR/sock"
mkdir -p "$DATA" "$SOCK"
PORT="$(pick_port 41200)"
assert_spike_port "$PORT"

PW_FILE="$WORKDIR/pw"
umask 077
PASSWORD="$(python3 -c 'import secrets,string; print("".join(secrets.choice(string.ascii_letters+string.digits) for _ in range(24)))')"
printf '%s\n' "$PASSWORD" > "$PW_FILE"
umask 022

log_section "1. initdb with scram-sha-256 + --pwfile"
t0="$(now_ms)"
"$BIN/initdb" -D "$DATA" -U postgres --encoding=UTF8 --locale=C.UTF-8 \
  --auth-local=scram-sha-256 --auth-host=scram-sha-256 --pwfile="$PW_FILE" \
  >"$WORKDIR/initdb.log" 2>&1
t1="$(now_ms)"
log_kv "initdb ms" "$((t1-t0))"
tail -4 "$WORKDIR/initdb.log"
cat >> "$DATA/postgresql.conf" <<EOF
listen_addresses = '127.0.0.1'
port = $PORT
unix_socket_directories = '$SOCK'
EOF

log_section "2. start and inspect the stored verifier (never the plaintext)"
t0="$(now_ms)"
"$BIN/pg_ctl" -D "$DATA" -l "$DATA/server.log" -w -t 30 start
PID="$(head -1 "$DATA/postmaster.pid")"
PIDS+=("$PID")
for _ in $(seq 1 200); do
  "$BIN/pg_isready" -h 127.0.0.1 -p "$PORT" -U postgres >/dev/null 2>&1 && break
  sleep 0.02
done
t1="$(now_ms)"
log_kv "start->ready ms" "$((t1-t0))"

echo "--- password supplied via PGPASSWORD env (never argv) ---"
PGPASSWORD="$PASSWORD" "$BIN/psql" -h 127.0.0.1 -p "$PORT" -U postgres -d postgres \
  -Atc "select rolname, substr(rolpassword,1,14) as verifier_prefix, length(rolpassword) from pg_authid where rolname='postgres';"
echo "--- grep the plaintext in the whole data dir (must be 0 hits) ---"
hits=$(set +o pipefail; grep -rIl --binary-files=text -e "$PASSWORD" "$DATA" 2>/dev/null | wc -l)
log_kv "plaintext occurrences in datadir" "$hits"
echo "--- full verifier shape ---"
PGPASSWORD="$PASSWORD" "$BIN/psql" -h 127.0.0.1 -p "$PORT" -U postgres -d postgres -Atc \
  "select split_part(rolpassword,'\$',1)||'\$'||split_part(rolpassword,'\$',2) as method_and_iterations from pg_authid where rolname='postgres';"
echo "--- wrong password ---"
set +e
PGPASSWORD="wrong-$PASSWORD" "$BIN/psql" -h 127.0.0.1 -p "$PORT" -U postgres -d postgres -Atc 'select 1' 2>&1
echo "rc=$?"
echo "--- missing password (no PGPASSWORD, non-interactive) ---"
PGPASSWORD="" "$BIN/psql" -w -h 127.0.0.1 -p "$PORT" -U postgres -d postgres -Atc 'select 1' 2>&1
echo "rc=$?"
echo "--- nonexistent database (server up) ---"
PGPASSWORD="$PASSWORD" "$BIN/psql" -h 127.0.0.1 -p "$PORT" -U postgres -d nosuchdb -Atc 'select 1' 2>&1
echo "rc=$?"
set -e

log_section "3. pg_isready auth behaviour and exit codes"
set +e
echo "--- pg_isready, correct db, no password needed (it does not authenticate fully) ---"
"$BIN/pg_isready" -h 127.0.0.1 -p "$PORT" -U postgres -d postgres; echo "rc=$?"
echo "--- pg_isready with -d nosuchdb (documented surprise below) ---"
"$BIN/pg_isready" -h 127.0.0.1 -p "$PORT" -U postgres -d nosuchdb; echo "rc=$?"
echo "--- pg_isready with a role that cannot log in ---"
PGPASSWORD="$PASSWORD" "$BIN/psql" -h 127.0.0.1 -p "$PORT" -U postgres -d postgres -Atc \
  "create role nologin_role nologin;" >/dev/null 2>&1 || true
"$BIN/pg_isready" -h 127.0.0.1 -p "$PORT" -U nologin_role -d postgres; echo "rc=$?"
set -e

log_section "4. connection URI format (percent-encoding matters)"
python3 - "$PASSWORD" "$PORT" <<'PY'
import sys, urllib.parse
pw, port = sys.argv[1], sys.argv[2]
safe = urllib.parse.quote(pw, safe="")
print("raw password length:", len(pw))
print("percent-encoded length:", len(safe))
print("uri:", f"postgresql://postgres:{safe}@127.0.0.1:{port}/postgres")
print("special chars in password:", [c for c in pw if not c.isalnum()])
PY
echo "--- password with URI-significant characters, end to end ---"
PW2='p@ss:w/rd?#%[]{} '"'"'"x'
umask 077; printf '%s\n' "$PW2" > "$WORKDIR/pw2"; umask 022
DATA2="$WORKDIR/data2"; SOCK2="$WORKDIR/sock2"; mkdir -p "$SOCK2"
PORT2="$(pick_port 41210)"
"$BIN/initdb" -D "$DATA2" -U postgres --encoding=UTF8 --locale=C.UTF-8 \
  --auth-local=trust --auth-host=scram-sha-256 --pwfile="$WORKDIR/pw2" >/dev/null 2>&1
cat >> "$DATA2/postgresql.conf" <<EOF
listen_addresses = '127.0.0.1'
port = $PORT2
unix_socket_directories = '$SOCK2'
EOF
"$BIN/pg_ctl" -D "$DATA2" -l "$DATA2/log" -w -t 30 start
PID2="$(head -1 "$DATA2/postmaster.pid")"; PIDS+=("$PID2")
URI="$(python3 -c 'import sys,urllib.parse;print("postgresql://postgres:%s@127.0.0.1:%s/postgres"%(urllib.parse.quote(sys.argv[1],safe=""),sys.argv[2]))' "$PW2" "$PORT2")"
echo "uri: $URI"
PGPASSWORD="$PW2" "$BIN/psql" "$URI" -Atc "select 'uri-ok', current_user;" 2>&1 || true
echo "--- with the raw (unencoded) password inside the URI: must fail ---"
RAW_URI="postgresql://postgres:${PW2}@127.0.0.1:${PORT2}/postgres"
set +e
PGPASSWORD="$PW2" "$BIN/psql" "$RAW_URI" -Atc "select 'raw-uri-ok';" 2>&1 | sed -n '1,4p'
set -e
"$BIN/pg_ctl" -D "$DATA2" -m fast -w -t 20 stop

log_section "5. pidfile line 8 (status) transitions"
echo "--- running ---"
nl -ba "$DATA"/*.pid 2>/dev/null || nl -ba "$DATA/postmaster.pid"
( "$BIN/pg_ctl" -D "$DATA" -m fast -w -t 20 stop >/dev/null 2>&1 & )
for _ in $(seq 1 40); do
  if [ -f "$DATA/postmaster.pid" ]; then
    line8="$(sed -n '8p' "$DATA/postmaster.pid")"
    echo "during stop, line 8: '$line8'"
  else
    echo "during stop: postmaster.pid already removed"
    break
  fi
  sleep 0.01
done
echo "--- after stop: pidfile exists? ---"
[ -f "$DATA/postmaster.pid" ] && echo yes || echo "no (removed on clean shutdown)"

log_section "6. detached spawn: does the engine survive the parent shell exiting?"
DATA6="$WORKDIR/data6"; SOCK6="$WORKDIR/sock6"; mkdir -p "$SOCK6"
PORT6="$(pick_port 41220)"
"$BIN/initdb" -D "$DATA6" -U postgres --encoding=UTF8 --locale=C.UTF-8 \
  --auth-local=trust --auth-host=trust >/dev/null 2>&1
cat >> "$DATA6/postgresql.conf" <<EOF
listen_addresses = '127.0.0.1'
port = $PORT6
unix_socket_directories = '$SOCK6'
EOF

echo "--- 6a. pg_ctl start inside a subshell that then exits ---"
bash -c "'$BIN/pg_ctl' -D '$DATA6' -l '$DATA6/log' -w -t 30 start >/dev/null 2>&1; echo subshell-done"
sleep 0.1
PGPID="$(head -1 "$DATA6/postmaster.pid" 2>/dev/null || echo)"
log_kv "postmaster pid" "$PGPID"
if [ -n "$PGPID" ] && [ -d "/proc/$PGPID" ]; then
  echo "SURVIVED parent shell exit: yes"
  awk '{print "ppid:",$4,"sid:",$6}' "/proc/$PGPID/stat"
  PIDS+=("$PGPID")
  "$BIN/pg_ctl" -D "$DATA6" -m fast -w -t 20 stop >/dev/null
else
  echo "SURVIVED parent shell exit: no"
fi

echo "--- 6b. plain '&' background from a subshell (no setsid, no pg_ctl) ---"
bash -c "'$BIN/postgres' -D '$DATA6' >>'$DATA6/log' 2>&1 </dev/null & echo bg-pid=\$!; exit 0"
sleep 0.5
PGPID2="$(head -1 "$DATA6/postmaster.pid" 2>/dev/null || echo)"
log_kv "postmaster pid (bg)" "$PGPID2"
if [ -n "$PGPID2" ] && [ -d "/proc/$PGPID2" ]; then
  echo "SURVIVED parent shell exit: yes"
  awk '{print "ppid:",$4,"sid:",$6}' "/proc/$PGPID2/stat"
  PIDS+=("$PGPID2")
  "$BIN/pg_ctl" -D "$DATA6" -m fast -w -t 20 stop >/dev/null
else
  echo "SURVIVED parent shell exit: no (SIGHUP took it down)"
fi

log_section "7. PID validation inputs available without pg_ctl"
DATA7="$WORKDIR/data7"; SOCK7="$WORKDIR/sock7"; mkdir -p "$SOCK7"
PORT7="$(pick_port 41230)"
"$BIN/initdb" -D "$DATA7" -U postgres --encoding=UTF8 --locale=C.UTF-8 \
  --auth-local=trust --auth-host=trust >/dev/null 2>&1
cat >> "$DATA7/postgresql.conf" <<EOF
listen_addresses = '127.0.0.1'
port = $PORT7
unix_socket_directories = '$SOCK7'
EOF
"$BIN/pg_ctl" -D "$DATA7" -l "$DATA7/log" -w -t 30 start
PID7="$(head -1 "$DATA7/postmaster.pid")"; PIDS+=("$PID7")
log_kv "pidfile pid" "$PID7"
log_kv "pidfile start epoch (line 3)" "$(sed -n '3p' "$DATA7/postmaster.pid")"
start_ticks="$(awk '{print $22}' "/proc/$PID7/stat")"
btime="$(awk '/^btime/{print $2}' /proc/stat)"
hz="$(getconf CLK_TCK)"
python3 - "$start_ticks" "$btime" "$hz" "$(sed -n '3p' "$DATA7/postmaster.pid")" <<'PY'
import sys
ticks, btime, hz, pidline = int(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4])
derived = btime + ticks // hz
print("derived start epoch from /proc:", derived)
print("pidfile line 3          :", pidline)
print("delta seconds           :", abs(derived - pidline))
print("=> line 3 is the postmaster start time in epoch seconds (validates start time without /proc/stat parsing)")
PY
echo "--- exe name available 3 ways ---"
log_kv "readlink /proc/PID/exe" "$(readlink "/proc/$PID7/exe")"
log_kv "comm" "$(cat "/proc/$PID7/comm")"
log_kv "pg_ctl status" "$("$BIN/pg_ctl" -D "$DATA7" status 2>&1 || true)"
echo "--- negative test: pid alive but not postgres (PID reuse simulation) ---"
sleep 300 & FAKE=$!
PIDS+=("$FAKE")
log_kv "fake pid" "$FAKE"
log_kv "fake exe" "$(readlink "/proc/$FAKE/exe")"
echo "an adapter that only did kill(pid,0) would now signal an unrelated process; the exe+starttime check prevents it"
kill "$FAKE" 2>/dev/null || true
"$BIN/pg_ctl" -D "$DATA7" -m fast -w -t 20 stop
PIDS=()

log_section "done"