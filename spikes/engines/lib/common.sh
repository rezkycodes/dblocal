#!/usr/bin/env bash
# Shared helpers for DBLocal engine spikes.
#
# Safety contract (see docs/orchestration/tasks/W0-B-engine-spikes.md):
#   * ports 41000-41999 only, never 5432/6379
#   * all mutable state in mktemp -d dirs, never ~/.dblocal
#   * downloads cached in $HOME/.cache/dblocal-dev
#   * every script cleans up via trap
#
# Source this file:  . "$(dirname "$0")/../lib/common.sh"

set -euo pipefail

CACHE_DIR="${DBLOCAL_SPIKE_CACHE:-$HOME/.cache/dblocal-dev}"
PORT_LO=41000
PORT_HI=41999

mkdir -p "$CACHE_DIR"

log_section() {
  printf '\n===== %s =====\n' "$*"
}

log_kv() {
  printf '%-28s %s\n' "$1" "$2"
}

now_ms() {
  date +%s%3N
}

# pick_port [preferred] -> echoes a port in 41000-41999 that binds on both 127.0.0.1 and ::1
pick_port() {
  local preferred="${1:-}"
  python3 - "$preferred" "$PORT_LO" "$PORT_HI" <<'PY'
import socket, sys
preferred = int(sys.argv[1]) if sys.argv[1] else 0
lo, hi = int(sys.argv[2]), int(sys.argv[3])

def free(port):
    socks = []
    try:
        for fam, addr in ((socket.AF_INET, "127.0.0.1"), (socket.AF_INET6, "::1")):
            s = socket.socket(fam, socket.SOCK_STREAM)
            s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 0)
            try:
                s.bind((addr, port))
            except OSError:
                s.close()
                return False
            socks.append(s)
        return True
    finally:
        for s in socks:
            s.close()

if preferred and lo <= preferred <= hi and free(preferred):
    print(preferred)
    raise SystemExit(0)
port = preferred if preferred and preferred >= lo else lo
for p in range(max(port, lo), hi + 1):
    if free(p):
        print(p)
        raise SystemExit(0)
raise SystemExit("no free port in %d-%d" % (lo, hi))
PY
}

# assert_spike_port <port> -> refuses anything outside 41000-41999 and the canonical ports
assert_spike_port() {
  local p="$1"
  case "$p" in
    5432|6379|3306|3307)
      echo "REFUSING reserved canonical port $p" >&2
      exit 1
      ;;
  esac
  if [ "$p" -lt "$PORT_LO" ] || [ "$p" -gt "$PORT_HI" ]; then
    echo "REFUSING port $p outside $PORT_LO-$PORT_HI" >&2
    exit 1
  fi
}

# fetch_verify <url> <sha256-url> <dest-file> -> downloads once, verifies every time
fetch_verify() {
  local url="$1" sha_url="$2" dest="$3"
  local expected actual
  if [ ! -f "$dest" ]; then
    echo "downloading $url" >&2
    curl -fSL --retry 3 --retry-delay 2 -o "$dest.part" "$url" >&2
    mv "$dest.part" "$dest"
  else
    echo "cached: $dest" >&2
  fi
  expected="$(curl -fsSL --retry 3 "$sha_url" | awk '{print $1}')"
  actual="$(sha256sum "$dest" | awk '{print $1}')"
  if [ "${#expected}" -ne 64 ]; then
    echo "bad sha256 payload from $sha_url: '$expected'" >&2
    exit 1
  fi
  if [ "$expected" != "$actual" ]; then
    echo "SHA-256 MISMATCH for $dest" >&2
    echo "  expected $expected" >&2
    echo "  actual   $actual" >&2
    exit 1
  fi
  log_kv "sha256 verified" "$actual"
}

# wait_tcp <host> <port> <timeout-ms> -> 0 when a TCP connect succeeds
wait_tcp() {
  local host="$1" port="$2" timeout_ms="${3:-10000}"
  local deadline=$(( $(now_ms) + timeout_ms ))
  while [ "$(now_ms)" -lt "$deadline" ]; do
    if (exec 3<>"/dev/tcp/$host/$port") 2>/dev/null; then
      exec 3>&- 3<&-
      return 0
    fi
    sleep 0.05
  done
  return 1
}

# pg_isready_client <bindir> <host> <port> [user] [dbname]
pg_ready() {
  local bindir="$1" host="$2" port="$3" user="${4:-}" db="${5:-}"
  local args=(-h "$host" -p "$port")
  [ -n "$user" ] && args+=(-U "$user")
  [ -n "$db" ] && args+=(-d "$db")
  "$bindir/pg_isready" "${args[@]}"
}

# stop_pid_verified <pid> <expected-exe-substring> [signal]
# Refuses to signal a PID that is not the expected executable (PID reuse guard).
stop_pid_verified() {
  local pid="$1" exe_substr="$2" sig="${3:-TERM}"
  [ -z "$pid" ] && return 0
  if [ ! -d "/proc/$pid" ]; then
    echo "pid $pid already gone"
    return 0
  fi
  local exe
  exe="$(readlink "/proc/$pid/exe" 2>/dev/null || true)"
  case "$exe" in
    *"$exe_substr"*) : ;;
    *)
      echo "REFUSING to signal pid $pid: exe is '$exe', expected *$exe_substr*" >&2
      return 1
      ;;
  esac
  kill -"$sig" "$pid" 2>/dev/null || true
  return 0
}

# wait_pid_gone <pid> <timeout-ms>
wait_pid_gone() {
  local pid="$1" timeout_ms="${2:-10000}"
  local deadline=$(( $(now_ms) + timeout_ms ))
  while [ -d "/proc/$pid" ] && [ "$(now_ms)" -lt "$deadline" ]; do
    sleep 0.05
  done
  [ ! -d "/proc/$pid" ]
}

# listen_report <port> -> the ss line for a port, TCP and unix
listen_report() {
  local port="$1"
  ss -ltnp 2>/dev/null | awk -v p=":$port" 'NR == 1 || index($4, p)'
}

require_cmd() {
  for c in "$@"; do
    command -v "$c" >/dev/null 2>&1 || {
      echo "missing required command: $c" >&2
      exit 1
    }
  done
}

require_cmd curl tar sha256sum python3 ss

trap_cleanup_note() {
  echo
  echo "cleanup: no processes left on spike ports; temp dirs removed by trap"
}
