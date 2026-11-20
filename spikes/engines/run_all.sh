#!/usr/bin/env bash
# Run every DBLocal engine spike in order. Safe: ports 41000-41999, mktemp dirs, traps.
#   bash spikes/engines/run_all.sh            # all scripts
#   SKIP_UPSTREAM=1 bash spikes/engines/run_all.sh
set -uo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
fail=0
for script in \
  "$ROOT/postgres/01_download_layout.sh" \
  "$ROOT/postgres/02_initdb.sh" \
  "$ROOT/postgres/03_lifecycle.sh" \
  "$ROOT/postgres/04_failures.sh" \
  "$ROOT/postgres/05_auth_pid.sh" \
  "$ROOT/postgres/06_pid_edge.sh" \
  "$ROOT/valkey/01_download_layout.sh" \
  "$ROOT/valkey/02_lifecycle.sh" \
  "$ROOT/valkey/03_failures.sh" ; do
  printf '\n########## %s\n' "${script#"$ROOT"/}"
  if ! timeout 600 bash "$script"; then
    printf 'FAILED: %s\n' "${script#"$ROOT"/}" >&2
    fail=1
  fi
done
if [ "${SKIP_UPSTREAM:-0}" != "1" ]; then
  printf '\n########## upstream/probe_upstream.sh\n'
  timeout 600 bash "$ROOT/upstream/probe_upstream.sh" || fail=1
fi
echo
echo "=== leftover check ==="
if ss -ltn | grep -E ':41[0-9]{3}\b'; then echo "LEFTOVER LISTENERS" >&2; fail=1; else echo "no listeners in 41000-41999"; fi
if pgrep -af '(^|/)(postgres|valkey-server)( |$)' >/tmp/_spike_leftover_ps 2>/dev/null; then
  cat /tmp/_spike_leftover_ps; echo "LEFTOVER PROCESSES" >&2; fail=1
else
  echo "no postgres/valkey-server processes"
fi
rm -f /tmp/_spike_leftover_ps
if ls -d /tmp/dblocal-pg-* /tmp/dblocal-vk-* 2>/dev/null; then echo "LEFTOVER TEMP DIRS" >&2; fail=1; else echo "no spike temp dirs"; fi
exit "$fail"
