#!/usr/bin/env bash
# Valkey spike: verbatim failure texts for F-LOG-02 (port in use, bad config, unwritable dir),
# glibc floor per Ubuntu variant, and startup-failure detection via log + exit code.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/../lib/common.sh"

VK_VER="${VK_VER:-9.2.0}"
WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/dblocal-vk-fail-XXXXXX")"
PIDS=()
cleanup() {
  set +e
  for pid in "${PIDS[@]:-}"; do
    [ -n "${pid:-}" ] && [ -d "/proc/$pid" ] && kill -9 "$pid" 2>/dev/null
  done
  rm -rf "$WORKDIR"
}
trap cleanup EXIT

for variant in noble jammy; do
  A="$CACHE_DIR/valkey-${VK_VER}-${variant}-x86_64.tar.gz"
  [ -f "$A" ] || { echo "missing $A, run 01_download_layout.sh" >&2; exit 1; }
  mkdir -p "$WORKDIR/$variant"
  tar -xzf "$A" -C "$WORKDIR/$variant"
done
BIN_N="$WORKDIR/noble/valkey-${VK_VER}-noble-x86_64/bin"
BIN_J="$WORKDIR/jammy/valkey-${VK_VER}-jammy-x86_64/bin"

log_section "glibc floor per variant (max GLIBC_ symbol required by valkey-server)"
for v in noble jammy; do
  b="$WORKDIR/$v/valkey-${VK_VER}-${v}-x86_64/bin/valkey-server"
  maxc="$(readelf -V "$b" 2>/dev/null | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -V | tail -1)"
  printf '%-6s needs GLIBC_%s\n' "$v" "$maxc"
done
echo "--- which GLIBC symbols differ (noble-only additions) ---"
comm -23 \
  <(readelf -V "$BIN_N/valkey-server" 2>/dev/null | grep -o 'GLIBC_[0-9.]*' | sort -Vu) \
  <(readelf -V "$BIN_J/valkey-server" 2>/dev/null | grep -o 'GLIBC_[0-9.]*' | sort -Vu) \
  | sed 's/^/noble-only: /'
echo "--- non-base shared libs (must exist on host) ---"
for v in noble jammy; do
  b="$WORKDIR/$v/valkey-${VK_VER}-${v}-x86_64/bin/valkey-server"
  echo "[$v]"; ldd "$b" | awk '/=>/ {print "  "$1" -> "$3}' | grep -vE '/lib64/lib(c|m|dl|pthread|rt|resolv|gcc_s|z|stdc\+\+)\.so' || true
done
echo "--- this host has libsystemd? ---"
ldconfig -p | grep -E 'libsystemd\.so\.0' || echo "libsystemd.so.0 NOT in ldconfig cache"
ls -la /lib64/libsystemd.so.0 2>/dev/null || true

log_section "exit code / text matrix"
run_case() {
  local title="$1"; shift
  local lines="${CASE_LINES:-1,30}"
  case "$lines" in *p) : ;; *) lines="${lines}p" ;; esac
  log_kv "case" "$title"
  set +e
  local out rc
  out="$("$@" 2>&1)"; rc=$?
  set -e
  log_kv "exit" "$rc"
  printf '%s\n' "$out" | sed -n "$lines"
  echo "---"
}

DATADIR="$WORKDIR/data"; RUNDIR="$WORKDIR/run"; mkdir -p "$DATADIR" "$RUNDIR"
PORT="$(pick_port 41500)"; assert_spike_port "$PORT"
BASE="$WORKDIR/base.conf"
cat > "$BASE" <<EOF
bind 127.0.0.1
protected-mode yes
port $PORT
daemonize no
pidfile $RUNDIR/valkey.pid
logfile $RUNDIR/valkey.log
dir $DATADIR
save ""
appendonly no
EOF

log_section "1. port already in use (log to file, then to stdout)"
python3 - "$PORT" <<'PY' &
import socket, sys, time
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", int(sys.argv[1]))); s.listen(1); time.sleep(25)
PY
HOLDER=$!; PIDS+=("$HOLDER")
wait_tcp 127.0.0.1 "$PORT" 5000 || true
run_case "port in use, logfile set" timeout 6 "$BIN_J/valkey-server" "$BASE"
echo "--- logfile contents (verbatim) ---"
cat "$RUNDIR/valkey.log" 2>/dev/null | sed -n '1,40p' || echo "(no logfile)"
echo "--- same case, logfile to stdout (no logfile directive) ---"
CONF2="$WORKDIR/nolog.conf"
grep -v '^logfile ' "$BASE" > "$CONF2"
CASE_LINES="1,60" run_case "port in use, stderr" timeout 6 "$BIN_J/valkey-server" "$CONF2"
kill "$HOLDER" 2>/dev/null || true; PIDS=()
rm -f "$RUNDIR/valkey.log"

log_section "2. bad config: unknown directive"
BAD="$WORKDIR/bad1.conf"
{ cat "$BASE"; echo "nonexistent_directive_on_purpose 1"; } > "$BAD"
CASE_LINES="1,12" run_case "unknown directive" timeout 6 "$BIN_J/valkey-server" "$BAD"

log_section "3. bad config: wrong value type"
BAD2="$WORKDIR/bad2.conf"
{ cat "$BASE"; echo "maxmemory not-a-number"; } > "$BAD2"
CASE_LINES="1,12" run_case "maxmemory not-a-number" timeout 6 "$BIN_J/valkey-server" "$BAD2"

log_section "4. bad config: port out of range"
BAD3="$WORKDIR/bad3.conf"
sed "s/^port .*/port 99999/" "$BASE" > "$BAD3"
CASE_LINES="1,12" run_case "port 99999" timeout 6 "$BIN_J/valkey-server" "$BAD3"

log_section "5. bad config: bind to a non-local address"
BAD4="$WORKDIR/bad4.conf"
sed "s/^bind .*/bind 10.255.255.1/" "$BASE" > "$BAD4"
CASE_LINES="1,25" run_case "bind 10.255.255.1" timeout 6 "$BIN_J/valkey-server" "$BAD4"
echo "--- logfile for the bind case ---"
rm -f "$RUNDIR/valkey.log.b4"; cat "$RUNDIR/valkey.log" 2>/dev/null | sed -n '1,40p' || true; rm -f "$RUNDIR/valkey.log"

log_section "6. unwritable data dir"
DD="$WORKDIR/ro"; mkdir -p "$DD"; chmod 500 "$DD"
CONF6="$WORKDIR/ro.conf"
{ sed "s#^dir .*#dir $DD#" "$BASE"; echo "appendonly yes"; } > "$CONF6"
CASE_LINES="1,40" run_case "unwritable dir" timeout 6 "$BIN_J/valkey-server" "$CONF6"
echo "--- logfile for the unwritable-dir case (the ONLY place the cause appears) ---"
cat "$RUNDIR/valkey.log" 2>/dev/null | sed -n '1,40p' || true
echo "--- control: same dir writable, appendonly yes -> starts OK ---"
DDOK="$WORKDIR/okdir"; mkdir -p "$DDOK"
{ sed "s#^dir .*#dir $DDOK#" "$BASE"; echo "appendonly yes"; } > "$WORKDIR/ok.conf"
setsid "$BIN_J/valkey-server" "$WORKDIR/ok.conf" >/dev/null 2>&1 </dev/null &
OKPID=$!; disown "$OKPID" 2>/dev/null || true; PIDS+=("$OKPID")
for _ in $(seq 1 400); do "$BIN_J/valkey-cli" -h 127.0.0.1 -p "$PORT" PING 2>/dev/null | grep -q PONG && break; sleep 0.01; done
"$BIN_J/valkey-cli" -h 127.0.0.1 -p "$PORT" SHUTDOWN NOSAVE 2>/dev/null || true
wait_pid_gone "$OKPID" 6000 || true
chmod 700 "$DD"

log_section "7. missing data dir (dir does not exist)"
CONF7="$WORKDIR/missingdir.conf"
sed "s#^dir .*#dir $WORKDIR/does-not-exist#" "$BASE" > "$CONF7"
CASE_LINES="1,25" run_case "missing dir" timeout 6 "$BIN_J/valkey-server" "$CONF7"

log_section "8. requirepass with a bad ACL file / protected-mode no warning"
rm -f "$RUNDIR/valkey.log"
CONF8="$WORKDIR/prot.conf"
{ sed "s/^protected-mode .*/protected-mode no/" "$BASE"; echo "requirepass vk-spike-pw"; } > "$CONF8"
setsid "$BIN_J/valkey-server" "$CONF8" >"$WORKDIR/prot.out" 2>&1 </dev/null &
P8=$!; disown "$P8" 2>/dev/null || true; PIDS+=("$P8")
for _ in $(seq 1 400); do
  "$BIN_J/valkey-cli" -h 127.0.0.1 -p "$PORT" -a vk-spike-pw --no-auth-warning PING 2>/dev/null | grep -q PONG && break
  sleep 0.01
done
echo "--- startup log warnings (protected-mode no + no bind) ---"
grep -iE 'warning|protected|bind' "$RUNDIR/valkey.log" | sed -n '1,10p' || true
"$BIN_J/valkey-cli" -h 127.0.0.1 -p "$PORT" -a vk-spike-pw --no-auth-warning SHUTDOWN NOSAVE 2>/dev/null || true
wait_pid_gone "$P8" 5000 || true; PIDS=()

log_section "done"