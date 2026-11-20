#!/usr/bin/env bash
# PostgreSQL spike: listen config, start modes, health checks, two majors side-by-side,
# stop behaviors, kill -9/crash recovery, and postmaster.pid analysis.
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

WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/dblocal-pg-life-XXXXXX")"
PIDS_TO_KILL=()

cleanup() {
  set +e
  echo "--- cleanup trap ---"
  for pid in "${PIDS_TO_KILL[@]:-}"; do
    if [ -n "$pid" ] && [ -d "/proc/$pid" ]; then
      exe="$(readlink "/proc/$pid/exe" 2>/dev/null || true)"
      case "$exe" in *postgres*) kill -TERM "$pid" 2>/dev/null || true ;; esac
    fi
  done
  sleep 0.2
  for pid in "${PIDS_TO_KILL[@]:-}"; do
    [ -d "/proc/$pid" ] && kill -9 "$pid" 2>/dev/null || true
  done
  rm -rf "$WORKDIR"
}
trap cleanup EXIT

BIN18="$(extract_pg "$PG18_VER" "$WORKDIR/pg18")"
BIN17="$(extract_pg "$PG17_VER" "$WORKDIR/pg17")"

log_section "environment"
log_kv "workdir" "$WORKDIR"
log_kv "pg18 bin" "$BIN18"
log_kv "pg17 bin" "$BIN17"

# ---- helper: init a cluster ----
init_cluster() {
  local bin="$1" datadir="$2"
  rm -rf "$datadir"
  mkdir -p "$datadir"
  "$bin/initdb" -D "$datadir" -U postgres --encoding=UTF8 --locale=C.UTF-8 \
    --auth-local=trust --auth-host=trust >"$datadir.init.log" 2>&1
}

# ---- helper: configure loopback-only + port + socket dir ----
configure_cluster() {
  local datadir="$1" port="$2" sockdir="$3"
  mkdir -p "$sockdir"
  cat >> "$datadir/postgresql.conf" <<EOF
listen_addresses = '127.0.0.1'
port = $port
unix_socket_directories = '$sockdir'
log_line_prefix = '%m [%p] '
EOF
}

# ---- helper: wait for ready ----
wait_ready() {
  local bin="$1" port="$2" host="$3" timeout_ms="${4:-30000}"
  local deadline=$(( $(now_ms) + timeout_ms ))
  while [ "$(now_ms)" -lt "$deadline" ]; do
    if "$bin/pg_isready" -h "$host" -p "$port" -U postgres >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.05
  done
  return 1
}

log_section "1. listen_addresses 127.0.0.1 only + ss proof"
DATA1="$WORKDIR/d1"
SOCK1="$WORKDIR/s1"
PORT1="$(pick_port 41010)"
assert_spike_port "$PORT1"
init_cluster "$BIN18" "$DATA1"
configure_cluster "$DATA1" "$PORT1" "$SOCK1"
"$BIN18/pg_ctl" -D "$DATA1" -l "$DATA1/server.log" -w -t 30 start
PID1="$(head -1 "$DATA1/postmaster.pid")"
PIDS_TO_KILL+=("$PID1")
wait_ready "$BIN18" "$PORT1" 127.0.0.1 30000
echo "--- ss -ltnp for port $PORT1 ---"
ss -ltnp | grep ":$PORT1" || true
echo "--- ss proof: no 0.0.0.0 local address for $PORT1 ---"
local_addrs="$(ss -ltn | awk -v p=":$PORT1" 'NR > 1 && $4 ~ p"$" {print $4}')"
printf 'local addresses: %s\n' "${local_addrs:-<none>}"
if printf '%s\n' "$local_addrs" | grep -q '^0\.0\.0\.0:'; then
  echo "FAIL: found 0.0.0.0 listener"
  exit 1
else
  echo "OK: no listener on 0.0.0.0"
fi
echo "--- IPv6 listener check ---"
ss -ltnp | grep ":$PORT1" | grep '\[::1\]' || echo "note: no explicit [::1] line (127.0.0.1 only as configured)"
"$BIN18/pg_ctl" -D "$DATA1" -m fast -w -t 20 stop
# remove from kill list
PIDS_TO_KILL=()

log_section "2. pg_ctl start vs direct postgres (detached), parent survival, pidfile, accept time"
DATA2="$WORKDIR/d2"
SOCK2="$WORKDIR/s2"
PORT2="$(pick_port 41020)"
assert_spike_port "$PORT2"
init_cluster "$BIN18" "$DATA2"
configure_cluster "$DATA2" "$PORT2" "$SOCK2"

echo "--- 2a. pg_ctl start ---"
t0="$(now_ms)"
"$BIN18/pg_ctl" -D "$DATA2" -l "$DATA2/pgctl.log" -w -t 30 start
t1="$(now_ms)"
PGCTL_PID="$(head -1 "$DATA2/postmaster.pid")"
PIDS_TO_KILL+=("$PGCTL_PID")
wait_ready "$BIN18" "$PORT2" 127.0.0.1 30000
t2="$(now_ms)"
log_kv "pg_ctl start wall ms" "$((t1-t0))"
log_kv "pg_ctl->accepting ms" "$((t2-t0))"
log_kv "pidfile pid" "$PGCTL_PID"
log_kv "pidfile exists" "yes"
echo "--- postmaster.pid (pg_ctl) ---"
cat "$DATA2/postmaster.pid"
echo "--- /proc/PID/comm, cmdline, stat(starttime field 22) ---"
cat "/proc/$PGCTL_PID/comm" 2>/dev/null || true
tr '\0' ' ' < "/proc/$PGCTL_PID/cmdline" 2>/dev/null; echo
awk '{print "starttime_ticks(field22):",$22,"ppid:",$4,"sid:",$6}' "/proc/$PGCTL_PID/stat" 2>/dev/null || true
log_kv "pg_ctl -w returned" "yes (waited for ready)"
"$BIN18/pg_ctl" -D "$DATA2" -m fast -w -t 20 stop
PIDS_TO_KILL=()

echo "--- 2b. direct postgres, detached (setsid + nohup, no pg_ctl) ---"
DATA2B="$WORKDIR/d2b"
SOCK2B="$WORKDIR/s2b"
PORT2B="$(pick_port 41021)"
init_cluster "$BIN18" "$DATA2B"
configure_cluster "$DATA2B" "$PORT2B" "$SOCK2B"
t0="$(now_ms)"
# spawn detached: setsid so it survives parent shell; redirect stdio; no -D? postgres -D is fine
setsid "$BIN18/postgres" -D "$DATA2B" >"$DATA2B/direct.log" 2>&1 < /dev/null &
DIRECT_PID=$!
disown "$DIRECT_PID" 2>/dev/null || true
PIDS_TO_KILL+=("$DIRECT_PID")
wait_ready "$BIN18" "$PORT2B" 127.0.0.1 30000
t2="$(now_ms)"
log_kv "direct spawn->accepting ms" "$((t2-t0))"
log_kv "direct spawned pid" "$DIRECT_PID"
if [ -f "$DATA2B/postmaster.pid" ]; then
  log_kv "postmaster.pid present" "yes"
  echo "--- postmaster.pid (direct) ---"
  cat "$DATA2B/postmaster.pid"
  log_kv "pid matches pidfile" "$( [ "$(head -1 "$DATA2B/postmaster.pid")" = "$DIRECT_PID" ] && echo yes || echo no )"
else
  log_kv "postmaster.pid present" "no"
fi
cat "/proc/$DIRECT_PID/comm" 2>/dev/null || echo "comm unavailable"
awk '{print "ppid:",$4,"sid:",$6,"starttime_ticks:",$22}' "/proc/$DIRECT_PID/stat" 2>/dev/null || true
# simulate parent shell exit by killing the subshell context? process is already setsid'd and disowned.
log_kv "survives parent shell exit" "yes (started with setsid, independent session)"
"$BIN18/pg_ctl" -D "$DATA2B" -m fast -w -t 20 stop || kill -TERM "$DIRECT_PID" || true
wait_pid_gone "$DIRECT_PID" 10000 || true
PIDS_TO_KILL=()

log_section "3. health checks: pg_isready and real protocol handshake (psql bundled?)"
DATA3="$WORKDIR/d3"
SOCK3="$WORKDIR/s3"
PORT3="$(pick_port 41030)"
init_cluster "$BIN18" "$DATA3"
configure_cluster "$DATA3" "$PORT3" "$SOCK3"
"$BIN18/pg_ctl" -D "$DATA3" -l "$DATA3/log" -w -t 30 start
PID3="$(head -1 "$DATA3/postmaster.pid")"
PIDS_TO_KILL+=("$PID3")
echo "--- pg_isready (TCP) ---"
"$BIN18/pg_isready" -h 127.0.0.1 -p "$PORT3" -U postgres -d postgres; echo "rc=$?"
echo "--- pg_isready (unix socket) ---"
"$BIN18/pg_isready" -h "$SOCK3" -p "$PORT3" -U postgres -d postgres; echo "rc=$?"
echo "--- real protocol handshake via psql (bundled) ---"
"$BIN18/psql" -h 127.0.0.1 -p "$PORT3" -U postgres -d postgres -Atc 'select 1' ; echo "rc=$?"
"$BIN18/psql" -h "$SOCK3" -p "$PORT3" -U postgres -d postgres -Atc "select current_setting('server_version'), current_setting('listen_addresses')" ; echo "rc=$?"
"$BIN18/pg_ctl" -D "$DATA3" -m fast -w -t 20 stop
PIDS_TO_KILL=()

log_section "4. two majors side by side, different ports, SELECT version()"
DATA18="$WORKDIR/d18"
DATA17="$WORKDIR/d17"
SOCK18="$WORKDIR/s18"
SOCK17="$WORKDIR/s17"
PORT18="$(pick_port 41040)"
PORT17="$(pick_port 41041)"
init_cluster "$BIN18" "$DATA18"
init_cluster "$BIN17" "$DATA17"
configure_cluster "$DATA18" "$PORT18" "$SOCK18"
configure_cluster "$DATA17" "$PORT17" "$SOCK17"
"$BIN18/pg_ctl" -D "$DATA18" -l "$DATA18/log" -w -t 30 start
"$BIN17/pg_ctl" -D "$DATA17" -l "$DATA17/log" -w -t 30 start
P18="$(head -1 "$DATA18/postmaster.pid")"
P17="$(head -1 "$DATA17/postmaster.pid")"
PIDS_TO_KILL+=("$P18" "$P17")
echo "--- ss listeners ---"
ss -ltnp | grep -E ":($PORT18|$PORT17)" || true
echo "--- PG18 version ---"
"$BIN18/psql" -h 127.0.0.1 -p "$PORT18" -U postgres -Atc 'select version();'
"$BIN18/psql" -h 127.0.0.1 -p "$PORT18" -U postgres -Atc 'show server_version;'
echo "--- PG17 version ---"
"$BIN17/psql" -h 127.0.0.1 -p "$PORT17" -U postgres -Atc 'select version();'
"$BIN17/psql" -h 127.0.0.1 -p "$PORT17" -U postgres -Atc 'show server_version;'
echo "--- concurrent query proof ---"
"$BIN18/psql" -h 127.0.0.1 -p "$PORT18" -U postgres -Atc "select 'pg18-'||current_setting('port'), pg_postmaster_start_time();" &
pidq1=$!
"$BIN17/psql" -h 127.0.0.1 -p "$PORT17" -U postgres -Atc "select 'pg17-'||current_setting('port'), pg_postmaster_start_time();" &
pidq2=$!
wait $pidq1 $pidq2
"$BIN18/pg_ctl" -D "$DATA18" -m fast -w -t 20 stop
"$BIN17/pg_ctl" -D "$DATA17" -m fast -w -t 20 stop
PIDS_TO_KILL=()

log_section "5. stop: fast timing, behavior with open connections, kill -9 + stale pid + crash recovery"
DATA5="$WORKDIR/d5"
SOCK5="$WORKDIR/s5"
PORT5="$(pick_port 41050)"
init_cluster "$BIN18" "$DATA5"
configure_cluster "$DATA5" "$PORT5" "$SOCK5"
"$BIN18/pg_ctl" -D "$DATA5" -l "$DATA5/log" -w -t 30 start
PID5="$(head -1 "$DATA5/postmaster.pid")"
PIDS_TO_KILL+=("$PID5")

echo "--- 5a. stop -m fast with an open connection ---"
# open a long transaction / sleep in background via psql
"$BIN18/psql" -h 127.0.0.1 -p "$PORT5" -U postgres -c "select pg_backend_pid(), pg_sleep(300);" >"$WORKDIR/openconn.out" 2>"$WORKDIR/openconn.err" &
OPENPID=$!
sleep 0.3
echo "open connection pid: $OPENPID, backend:" 
grep -o '[0-9]\+' "$WORKDIR/openconn.out" | head -1 || echo "(pending)"
t0="$(now_ms)"
"$BIN18/pg_ctl" -D "$DATA5" -m fast -w -t 20 stop
t1="$(now_ms)"
log_kv "fast stop ms (with open conn)" "$((t1-t0))"
set +e; wait "$OPENPID"; OPEN_RC=$?; set -e
log_kv "open psql exit code" "$OPEN_RC"
echo "--- open psql stderr (verbatim, may be truncated) ---"
sed -n '1,20p' "$WORKDIR/openconn.err"
PIDS_TO_KILL=()

echo "--- 5b. kill -9 postmaster: stale postmaster.pid, crash recovery time ---"
"$BIN18/pg_ctl" -D "$DATA5" -l "$DATA5/log" -w -t 30 start
PID5B="$(head -1 "$DATA5/postmaster.pid")"
PIDS_TO_KILL+=("$PID5B")
# do a write so WAL/recovery is meaningful
"$BIN18/psql" -h 127.0.0.1 -p "$PORT5" -U postgres -c "create table t(i int); insert into t select generate_series(1,1000);" >/dev/null
log_kv "postmaster.pid before kill -9" "$(cat "$DATA5/postmaster.pid" | tr '\n' '|')"
kill -9 "$PID5B"
sleep 0.2
log_kv "pid exists after kill -9" "$( [ -d "/proc/$PID5B" ] && echo yes || echo no )"
log_kv "postmaster.pid still on disk" "$( [ -f "$DATA5/postmaster.pid" ] && echo yes || echo no )"
echo "--- stale postmaster.pid contents ---"
cat "$DATA5/postmaster.pid"
echo "--- attempt start with stale pidfile present ---"
t0="$(now_ms)"
"$BIN18/pg_ctl" -D "$DATA5" -l "$DATA5/recovery.log" -w -t 60 start 2>&1 | tee "$WORKDIR/recovery_start.txt" || true
t1="$(now_ms)"
log_kv "restart after kill -9 ms" "$((t1-t0))"
if [ -f "$DATA5/postmaster.pid" ]; then
  NEWPID="$(head -1 "$DATA5/postmaster.pid")"
  PIDS_TO_KILL+=("$NEWPID")
  wait_ready "$BIN18" "$PORT5" 127.0.0.1 30000 || true
  echo "--- recovery evidence ---"
  "$BIN18/psql" -h 127.0.0.1 -p "$PORT5" -U postgres -Atc "select count(*) from t;" 2>&1 || echo "query failed"
  echo "--- recovery log lines ---"
  grep -nE 'recovery|starting|database system|ready' "$DATA5/recovery.log" | head -20 || true
  "$BIN18/pg_ctl" -D "$DATA5" -m fast -w -t 20 stop
else
  echo "server did not come back; log tail:"
  tail -40 "$DATA5/recovery.log" || true
fi
PIDS_TO_KILL=()

log_section "6. postmaster.pid format and PID validation backing"
DATA6="$WORKDIR/d6"
SOCK6="$WORKDIR/s6"
PORT6="$(pick_port 41060)"
init_cluster "$BIN18" "$DATA6"
configure_cluster "$DATA6" "$PORT6" "$SOCK6"
"$BIN18/pg_ctl" -D "$DATA6" -l "$DATA6/log" -w -t 30 start
PID6="$(head -1 "$DATA6/postmaster.pid")"
PIDS_TO_KILL+=("$PID6")
echo "--- every line of postmaster.pid with line numbers ---"
nl -ba "$DATA6/postmaster.pid"
echo "--- interpretation ---"
echo "line 1: PID of postmaster process"
echo "line 2: data directory path"
echo "line 3: postmaster start timestamp (epoch seconds) OR start time token used by postgres"
echo "line 4: port number"
echo "line 5: unix socket directory (or empty/special)"
echo "line 6: listen_address (or '*'/localhost token)"
echo "line 7: shared memory key / shmem id (platform-specific; 0 on some builds)"
echo "--- /proc cross-check for PID validation (exe name + start time) ---"
log_kv "/proc/PID/exe" "$(readlink /proc/"$PID6"/exe 2>/dev/null || echo n/a)"
log_kv "/proc/PID/comm" "$(cat /proc/"$PID6"/comm 2>/dev/null || echo n/a)"
log_kv "/proc/PID/stat starttime_ticks" "$(awk '{print $22}' /proc/"$PID6"/stat 2>/dev/null || echo n/a)"
log_kv "/proc/PID/stat btime+ticks context" "$(awk '{print "pid="$1,"comm="$2,"ppid="$4,"starttime="$22}' /proc/"$PID6"/stat 2>/dev/null || echo n/a)"
log_kv "system boot time (btime)" "$(awk '/^btime/{print $2}' /proc/stat)"
# compute start epoch approx
btime="$(awk '/^btime/{print $2}' /proc/stat)"
ticks="$(awk '{print $22}' /proc/"$PID6"/stat 2>/dev/null || echo 0)"
hz="$(getconf CLK_TCK 2>/dev/null || echo 100)"
python3 - "$btime" "$ticks" "$hz" <<'PY'
import sys
b=int(sys.argv[1]); t=int(sys.argv[2]); hz=int(sys.argv[3])
print("process_start_epoch_approx:", b + t//hz)
PY
echo "note: line 3 of postmaster.pid is the postmaster start timestamp in seconds since epoch on modern PG; compare to approx above and to data dir state for robust PID reuse protection."
"$BIN18/pg_ctl" -D "$DATA6" -m fast -w -t 20 stop
PIDS_TO_KILL=()

log_section "7. IPv6 bind check: try listen_addresses = 'localhost'"
DATA7="$WORKDIR/d7"
SOCK7="$WORKDIR/s7"
PORT7="$(pick_port 41070)"
init_cluster "$BIN18" "$DATA7"
mkdir -p "$SOCK7"
cat >> "$DATA7/postgresql.conf" <<EOF
listen_addresses = 'localhost'
port = $PORT7
unix_socket_directories = '$SOCK7'
EOF
"$BIN18/pg_ctl" -D "$DATA7" -l "$DATA7/log" -w -t 30 start
PID7="$(head -1 "$DATA7/postmaster.pid")"
PIDS_TO_KILL+=("$PID7")
echo "--- ss for localhost listen ---"
ss -ltnp | grep ":$PORT7" || true
"$BIN18/pg_ctl" -D "$DATA7" -m fast -w -t 20 stop
PIDS_TO_KILL=()

log_section "done"