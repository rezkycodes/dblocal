#!/usr/bin/env bash
# Valkey spike: config, detached start, PING (auth + unauth), shutdown modes, kill -9,
# persistence, failure texts, redis:// URIs.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=../lib/common.sh
. "$SCRIPT_DIR/../lib/common.sh"

VK_VER="${VK_VER:-9.2.0}"
VARIANT="${VARIANT:-jammy}"   # jammy needs the lowest glibc; see 01 output
ARCHIVE="$CACHE_DIR/valkey-${VK_VER}-${VARIANT}-x86_64.tar.gz"
[ -f "$ARCHIVE" ] || { echo "missing $ARCHIVE, run 01 first" >&2; exit 1; }

WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/dblocal-vk-life-XXXXXX")"
PIDS=()
cleanup() {
  set +e
  for pid in "${PIDS[@]:-}"; do
    [ -n "${pid:-}" ] && [ -d "/proc/$pid" ] && kill -TERM "$pid" 2>/dev/null
  done
  sleep 0.3
  for pid in "${PIDS[@]:-}"; do
    [ -n "${pid:-}" ] && [ -d "/proc/$pid" ] && kill -9 "$pid" 2>/dev/null
  done
  # refuse to leave unix sockets behind
  rm -rf "$WORKDIR"
}
trap cleanup EXIT

tar -xzf "$ARCHIVE" -C "$WORKDIR"
BIN="$WORKDIR/valkey-${VK_VER}-${VARIANT}-x86_64/bin"

log_section "environment"
log_kv "valkey-server" "$("$BIN/valkey-server" --version)"
log_kv "valkey-cli" "$("$BIN/valkey-cli" --version)"
log_kv "glibc" "$(ldd --version | head -1 | awk '{print $NF}')"

PORT1="$(pick_port 41300)"
assert_spike_port "$PORT1"
PASSWORD="$(python3 -c 'import secrets,string; print("".join(secrets.choice(string.ascii_letters+string.digits) for _ in range(24)))')"

log_section "1. minimal valkey.conf (loopback only, daemonize no)"
DATADIR="$WORKDIR/data1"
RUNDIR="$WORKDIR/run1"
mkdir -p "$DATADIR" "$RUNDIR"
CONF="$WORKDIR/valkey1.conf"
cat > "$CONF" <<EOF
bind 127.0.0.1 -::1
protected-mode yes
port $PORT1
tcp-backlog 511
timeout 0
tcp-keepalive 300
daemonize no
supervised no
pidfile $RUNDIR/valkey.pid
loglevel notice
logfile $RUNDIR/valkey.log
databases 16
dir $DATADIR
requirepass $PASSWORD
appendonly yes
appendfilename "appendonly.aof"
appenddirname "appendonlydir"
save 3600 1 300 100 60 10000
EOF
echo "--- effective config file (password line redacted for the doc) ---"
sed "s/^requirepass .*/requirepass <redacted>/" "$CONF"

log_section "2. detached start, time to PONG, authenticated vs unauthenticated PING"
t0="$(now_ms)"
setsid "$BIN/valkey-server" "$CONF" >"$RUNDIR/stdout.log" 2>&1 </dev/null &
VPID=$!
disown "$VPID" 2>/dev/null || true
PIDS+=("$VPID")
# wait for PONG with auth
ready=""
for _ in $(seq 1 600); do
  if "$BIN/valkey-cli" -h 127.0.0.1 -p "$PORT1" -a "$PASSWORD" --no-auth-warning PING 2>/dev/null | grep -q PONG; then
    ready=1; break
  fi
  sleep 0.01
done
t1="$(now_ms)"
if [ -z "$ready" ]; then
  echo "server never answered PING; log:"
  cat "$RUNDIR/valkey.log" "$RUNDIR/stdout.log" 2>/dev/null
  exit 1
fi
log_kv "spawn->PONG ms" "$((t1-t0))"
log_kv "spawned pid" "$VPID"
log_kv "pidfile present" "$( [ -f "$RUNDIR/valkey.pid" ] && cat "$RUNDIR/valkey.pid" || echo no )"
log_kv "comm" "$(cat /proc/$VPID/comm 2>/dev/null || echo n/a)"
log_kv "exe" "$(readlink /proc/$VPID/exe 2>/dev/null || echo n/a)"
awk '{print "ppid:",$4,"sid:",$6,"pgid:",$5}' "/proc/$VPID/stat"

echo "--- authenticated PING ---"
"$BIN/valkey-cli" -h 127.0.0.1 -p "$PORT1" -a "$PASSWORD" --no-auth-warning PING
echo "--- unauthenticated PING (expected NOAUTH) ---"
set +e
"$BIN/valkey-cli" -h 127.0.0.1 -p "$PORT1" PING 2>&1
echo "rc=$?"
echo "--- raw RESP over a socket, no auth ---"
printf 'PING\r\n' | timeout 2 python3 -c "
import socket,sys
s=socket.create_connection(('127.0.0.1',$PORT1),2)
s.sendall(sys.stdin.buffer.read())
print(repr(s.recv(200)))
s.close()"
echo "--- raw RESP, inline AUTH then PING ---"
python3 - "$PORT1" "$PASSWORD" <<'PY'
import socket, sys
port, pw = int(sys.argv[1]), sys.argv[2]
s = socket.create_connection(("127.0.0.1", port), 2)
s.sendall(f"AUTH {pw}\r\nPING\r\n".encode())
import time; time.sleep(0.1)
print(repr(s.recv(300)))
s.close()
PY
set -e

log_section "3. listeners: loopback only, ss proof"
ss -ltnp | grep ":$PORT1" || true
local_addrs="$(ss -ltn | awk -v p=":$PORT1" 'NR > 1 && $4 ~ p"$" {print $4}')"
printf 'local addresses: %s\n' "${local_addrs:-<none>}"
if printf '%s\n' "$local_addrs" | grep -q '^0\.0\.0\.0:'; then
  echo "FAIL: 0.0.0.0 listener present"
  exit 1
else
  echo "OK: no 0.0.0.0 listener"
fi
echo "--- bind 127.0.0.1 -::1 -> unix socket too? ---"
ls -la /tmp/valkey*.sock 2>/dev/null || echo "(no unixsocket configured; see note)"

log_section "4. data written, then SHUTDOWN NOSAVE vs SIGTERM vs kill -9"
"$BIN/valkey-cli" -h 127.0.0.1 -p "$PORT1" -a "$PASSWORD" --no-auth-warning \
  SET spike:key "value-1" >/dev/null
"$BIN/valkey-cli" -h 127.0.0.1 -p "$PORT1" -a "$PASSWORD" --no-auth-warning \
  DBSIZE
echo "--- appendonly dir contents before shutdown ---"
find "$DATADIR" -maxdepth 2 | sort

echo "--- 4a. SHUTDOWN NOSAVE ---"
t0="$(now_ms)"
"$BIN/valkey-cli" -h 127.0.0.1 -p "$PORT1" -a "$PASSWORD" --no-auth-warning SHUTDOWN NOSAVE 2>&1 || true
wait_pid_gone "$VPID" 10000 && echo "process exited" || echo "process STILL ALIVE after SHUTDOWN NOSAVE"
t1="$(now_ms)"
log_kv "SHUTDOWN NOSAVE ms" "$((t1-t0))"
log_kv "pidfile removed" "$( [ -f "$RUNDIR/valkey.pid" ] && echo no || echo yes )"
PIDS=()

log_section "5. persistence semantics: SIGTERM (no save point reached) vs SHUTDOWN SAVE"
rm -rf "$DATADIR"; mkdir -p "$DATADIR"
setsid "$BIN/valkey-server" "$CONF" >"$RUNDIR/stdout2.log" 2>&1 </dev/null &
VPID2=$!
disown "$VPID2" 2>/dev/null || true
PIDS+=("$VPID2")
for _ in $(seq 1 600); do
  "$BIN/valkey-cli" -h 127.0.0.1 -p "$PORT1" -a "$PASSWORD" --no-auth-warning PING 2>/dev/null | grep -q PONG && break
  sleep 0.01
done
"$BIN/valkey-cli" -h 127.0.0.1 -p "$PORT1" -a "$PASSWORD" --no-auth-warning SET spike:k2 "value-2" >/dev/null
echo "--- SIGTERM ---"
t0="$(now_ms)"
kill -TERM "$VPID2"
wait_pid_gone "$VPID2" 15000 && echo "exited cleanly" || echo "STILL ALIVE"
t1="$(now_ms)"
log_kv "SIGTERM exit ms" "$((t1-t0))"
echo "--- log tail after SIGTERM (note: does it mention saving?) ---"
tail -12 "$RUNDIR/valkey.log"
PIDS=()
echo "--- restart and check whether spike:k2 survived ---"
setsid "$BIN/valkey-server" "$CONF" >"$RUNDIR/stdout3.log" 2>&1 </dev/null &
VPID3=$!
disown "$VPID3" 2>/dev/null || true
PIDS+=("$VPID3")
for _ in $(seq 1 600); do
  "$BIN/valkey-cli" -h 127.0.0.1 -p "$PORT1" -a "$PASSWORD" --no-auth-warning PING 2>/dev/null | grep -q PONG && break
  sleep 0.01
done
"$BIN/valkey-cli" -h 127.0.0.1 -p "$PORT1" -a "$PASSWORD" --no-auth-warning GET spike:k2 2>&1 || true
"$BIN/valkey-cli" -h 127.0.0.1 -p "$PORT1" -a "$PASSWORD" --no-auth-warning DBSIZE
echo "--- SHUTDOWN SAVE (explicit) ---"
t0="$(now_ms)"
"$BIN/valkey-cli" -h 127.0.0.1 -p "$PORT1" -a "$PASSWORD" --no-auth-warning SHUTDOWN SAVE 2>&1 || true
wait_pid_gone "$VPID3" 15000 && echo "exited" || echo "STILL ALIVE"
t1="$(now_ms)"
log_kv "SHUTDOWN SAVE ms" "$((t1-t0))"
echo "--- datadir after SHUTDOWN SAVE ---"
find "$DATADIR" -maxdepth 2 -type f | sort | sed -n '1,20p'
PIDS=()

log_section "6. kill -9 (crash) behaviour"
setsid "$BIN/valkey-server" "$CONF" >"$RUNDIR/stdout4.log" 2>&1 </dev/null &
VPID4=$!
disown "$VPID4" 2>/dev/null || true
PIDS+=("$VPID4")
for _ in $(seq 1 600); do
  "$BIN/valkey-cli" -h 127.0.0.1 -p "$PORT1" -a "$PASSWORD" --no-auth-warning PING 2>/dev/null | grep -q PONG && break
  sleep 0.01
done
"$BIN/valkey-cli" -h 127.0.0.1 -p "$PORT1" -a "$PASSWORD" --no-auth-warning SET spike:k3 v3 >/dev/null
echo "--- pidfile before kill -9 ---"
cat "$RUNDIR/valkey.pid" 2>/dev/null || echo "(absent)"
kill -9 "$VPID4"
sleep 0.2
log_kv "pid gone" "$( [ -d "/proc/$VPID4" ] && echo no || echo yes )"
log_kv "stale pidfile left on disk" "$( [ -f "$RUNDIR/valkey.pid" ] && echo yes || echo no )"
echo "--- restart after kill -9 (must succeed, recovers from AOF) ---"
t0="$(now_ms)"
setsid "$BIN/valkey-server" "$CONF" >"$RUNDIR/stdout5.log" 2>&1 </dev/null &
VPID5=$!
disown "$VPID5" 2>/dev/null || true
PIDS+=("$VPID5")
ok=""
for _ in $(seq 1 1000); do
  "$BIN/valkey-cli" -h 127.0.0.1 -p "$PORT1" -a "$PASSWORD" --no-auth-warning PING 2>/dev/null | grep -q PONG && { ok=1; break; }
  sleep 0.01
done
t1="$(now_ms)"
if [ -n "$ok" ]; then
  log_kv "restart after kill -9 ms" "$((t1-t0))"
  "$BIN/valkey-cli" -h 127.0.0.1 -p "$PORT1" -a "$PASSWORD" --no-auth-warning GET spike:k3
  echo "--- startup log (AOF recovery lines) ---"
  grep -iE 'aof|append|loading|recover|ready to accept' "$RUNDIR/valkey.log" | tail -8 || true
else
  echo "restart FAILED; log:"
  tail -30 "$RUNDIR/valkey.log" "$RUNDIR/stdout5.log"
fi
"$BIN/valkey-cli" -h 127.0.0.1 -p "$PORT1" -a "$PASSWORD" --no-auth-warning SHUTDOWN NOSAVE 2>/dev/null || true
wait_pid_gone "$VPID5" 8000 || true
PIDS=()

log_section "7. failure texts: port in use, bad config, unwritable dir"
DATADIR7="$WORKDIR/data7"; RUNDIR7="$WORKDIR/run7"; mkdir -p "$DATADIR7" "$RUNDIR7"
PORT7="$(pick_port 41310)"
CONF7="$WORKDIR/valkey7.conf"
sed -e "s/^port .*/port $PORT7/" -e "s#^dir .*#dir $DATADIR7#" \
    -e "s#^pidfile .*#pidfile $RUNDIR7/valkey.pid#" \
    -e "s#^logfile .*#logfile $RUNDIR7/valkey.log#" "$CONF" > "$CONF7"

echo "--- 7a. occupy the port with a non-valkey listener ---"
python3 - "$PORT7" <<'PY' &
import socket, sys, time
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", int(sys.argv[1]))); s.listen(1); time.sleep(20)
PY
HOLDER=$!
PIDS+=("$HOLDER")
wait_tcp 127.0.0.1 "$PORT7" 5000 || true
set +e
out="$("$BIN/valkey-server" "$CONF7" 2>&1)"; rc=$?
set -e
log_kv "exit code" "$rc"
echo "--- verbatim stdout/stderr ---"
printf '%s\n' "$out"
kill "$HOLDER" 2>/dev/null || true
PIDS=()

echo "--- 7b. bad config directive ---"
BADCONF="$WORKDIR/bad.conf"
cat > "$BADCONF" <<EOF
bind 127.0.0.1
port $PORT7
dir $DATADIR7
logfile $RUNDIR7/bad.log
nonexistent_directive_on_purpose 1
EOF
set +e
out="$("$BIN/valkey-server" "$BADCONF" 2>&1)"; rc=$?
set -e
log_kv "exit code" "$rc"
printf '%s\n' "$out"

echo "--- 7c. config with an invalid value type ---"
BADCONF2="$WORKDIR/bad2.conf"
cat > "$BADCONF2" <<EOF
bind 127.0.0.1
port $PORT7
dir $DATADIR7
logfile $RUNDIR7/bad2.log
maxmemory not-a-number
EOF
set +e
out="$("$BIN/valkey-server" "$BADCONF2" 2>&1)"; rc=$?
set -e
log_kv "exit code" "$rc"
printf '%s\n' "$out"

echo "--- 7d. unwritable data dir ---"
DATADIR7B="$WORKDIR/data7b"; mkdir -p "$DATADIR7B"; chmod 500 "$DATADIR7B"
CONF7B="$WORKDIR/valkey7b.conf"
sed -e "s/^port .*/port $PORT7/" -e "s#^dir .*#dir $DATADIR7B#" \
    -e "s#^logfile .*#logfile /dev/stderr#" "$CONF" > "$CONF7B"
set +e
out="$(timeout 5 "$BIN/valkey-server" "$CONF7B" 2>&1)"; rc=$?
set -e
log_kv "exit code" "$rc"
printf '%s\n' "$out" | sed -n '1,15p'
chmod 700 "$DATADIR7B"

log_section "8. redis:// and valkey:// URIs with a password"
DATADIR8="$WORKDIR/data8"; RUNDIR8="$WORKDIR/run8"; mkdir -p "$DATADIR8" "$RUNDIR8"
PORT8="$(pick_port 41320)"
PW8='pa@ss:w/rd?#%'   # deliberately URI-hostile
CONF8="$WORKDIR/valkey8.conf"
cat > "$CONF8" <<EOF
bind 127.0.0.1
protected-mode yes
port $PORT8
daemonize no
pidfile $RUNDIR8/valkey.pid
logfile $RUNDIR8/valkey.log
dir $DATADIR8
requirepass $PW8
save ""
appendonly no
EOF
setsid "$BIN/valkey-server" "$CONF8" >"$RUNDIR8/stdout.log" 2>&1 </dev/null &
VPID8=$!
disown "$VPID8" 2>/dev/null || true
PIDS+=("$VPID8")
for _ in $(seq 1 600); do
  "$BIN/valkey-cli" -h 127.0.0.1 -p "$PORT8" -a "$PW8" --no-auth-warning PING 2>/dev/null | grep -q PONG && break
  sleep 0.01
done
ENC="$(python3 -c 'import sys,urllib.parse;print(urllib.parse.quote(sys.argv[1],safe=""))' "$PW8")"
echo "encoded password: $ENC"
for scheme in redis valkey rediss; do
  URI="$scheme://default:$ENC@127.0.0.1:$PORT8/0"
  echo "--- $scheme URI: $URI ---"
  set +e
  out="$("$BIN/valkey-cli" -u "$URI" PING 2>&1)"; rc=$?
  set -e
  printf 'rc=%s out=%s\n' "$rc" "$out"
done
echo "--- same via -h/-p/-a only ---"
"$BIN/valkey-cli" -h 127.0.0.1 -p "$PORT8" -a "$PW8" --no-auth-warning PING
echo "--- does valkey-cli accept --user default? ---"
set +e
"$BIN/valkey-cli" -h 127.0.0.1 -p "$PORT8" --user default -a "$PW8" --no-auth-warning PING 2>&1
set -e
"$BIN/valkey-cli" -h 127.0.0.1 -p "$PORT8" -a "$PW8" --no-auth-warning SHUTDOWN NOSAVE 2>/dev/null || true
wait_pid_gone "$VPID8" 8000 || true
PIDS=()

log_section "done"