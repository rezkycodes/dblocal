#!/usr/bin/env bash
# Offline tests for packaging/install.sh (F-PKG-03).
# Serves a fake GitHub Releases layout via `python3 -m http.server` and runs the
# installer against it using DBLOCAL_RELEASE_BASE_URL, without touching the
# network or the real GitHub API.
set -euo pipefail
unset XDG_DATA_HOME

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
repo_root="$(CDPATH='' cd -- "$script_dir/../.." && pwd)"
installer="${INSTALLER_UNDER_TEST:-$repo_root/packaging/install.sh}"

pass_count=0
fail_count=0
server_pid=""
tmp_root=""

# shellcheck disable=SC2329 # cleanup is invoked via trap
cleanup() {
  if [ -n "${server_pid:-}" ]; then
    kill "$server_pid" >/dev/null 2>&1 || true
    wait "$server_pid" >/dev/null 2>&1 || true
  fi
  if [ -n "${tmp_root:-}" ] && [ -d "$tmp_root" ]; then
    rm -rf "$tmp_root"
  fi
}
trap cleanup EXIT INT TERM

log() { printf '%s\n' "$*" >&2; }
pass() { pass_count=$((pass_count + 1)); log "PASS: $*"; }
fail() { fail_count=$((fail_count + 1)); log "FAIL: $*"; }

assert_eq() {
  local got="$1" want="$2" msg="$3"
  if [ "$got" = "$want" ]; then pass "$msg"; else fail "$msg (got='$got' want='$want')"; fi
}
assert_contains() {
  local hay="$1" needle="$2" msg="$3"
  if printf '%s' "$hay" | grep -Fq -- "$needle"; then pass "$msg"; else fail "$msg (missing '$needle')"; fi
}
assert_not_contains() {
  local hay="$1" needle="$2" msg="$3"
  if printf '%s' "$hay" | grep -Fq -- "$needle"; then fail "$msg (unexpected '$needle')"; else pass "$msg"; fi
}

pick_free_port() {
  python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

setup_release() {
  # Creates a fake release under $tmp_root/release for a given target/version.
  local version="$1" target="$2" marker="$3"
  local rel="$tmp_root/release"
  mkdir -p "$rel/stage"
  cat > "$rel/stage/dblocal" <<EOF
#!/bin/sh
echo "dblocal-fake ${version} ${target} ${marker}"
EOF
  chmod +x "$rel/stage/dblocal"
  local asset="dblocal-${version}-${target}.tar.gz"
  tar -C "$rel/stage" -czf "$rel/${asset}" dblocal
  rm -rf "$rel/stage"
  # (Re)generate checksums.txt covering every tarball present.
  (
    cd "$rel"
    : > checksums.txt
    for f in dblocal-*.tar.gz; do
      printf '%s  %s\n' "$(sha256_file "$f")" "$f" >> checksums.txt
    done
  )
}

start_server() {
  # Sets the globals BASE_URL and server_pid. It must NOT print the URL through a
  # command substitution: that runs in a subshell, so server_pid would be lost and
  # cleanup would never kill the server (one leaked python process per run).
  local port
  port="$(pick_free_port)"
  # Fully detach the HTTP server: redirect all stdio so it never holds the
  # caller's pipes open (otherwise `test.sh | tee` would never terminate).
  (
    cd "$tmp_root/release" || exit 1
    exec python3 -m http.server "$port" --bind 127.0.0.1
  ) </dev/null >/dev/null 2>&1 &
  server_pid=$!
  # Wait until it answers.
  for _ in $(seq 1 50); do
    if curl -fsS "http://127.0.0.1:${port}/checksums.txt" >/dev/null 2>&1; then
      break
    fi
    sleep 0.1
  done
  BASE_URL="http://127.0.0.1:${port}"
}

run_installer() {
  # Usage: run_installer <uname_s> <uname_m> <home_or_empty> <prefix_or_empty> <extra args...>
  local us="$1" um="$2" home="$3" prefix="$4"
  shift 4
  local -a cmd=(env
    "DBLOCAL_RELEASE_BASE_URL=${BASE_URL:-}"
    "DBLOCAL_TEST_UNAME_S=$us"
    "DBLOCAL_TEST_UNAME_M=$um"
  )
  if [ -n "$home" ]; then cmd+=("HOME=$home"); else cmd+=("HOME="); fi
  # Ensure PATH is predictable. Tests may prepend EXTRA_PATH_PREFIX to inject fakes.
  if [ -n "${EXTRA_PATH_PREFIX:-}" ]; then
    cmd+=("PATH=${EXTRA_PATH_PREFIX}:/usr/bin:/bin")
  else
    cmd+=("PATH=/usr/bin:/bin")
  fi
  cmd+=(sh "$installer")
  if [ -n "$prefix" ]; then cmd+=(--prefix "$prefix"); fi
  cmd+=("$@")
  "${cmd[@]}"
}

echo "== dblocal install.sh offline tests ==" >&2
[ -f "$installer" ] || { log "installer not found: $installer"; exit 1; }
[ -x "$installer" ] || chmod +x "$installer"

tmp_root="$(mktemp -d)"
VERSION="v0.1.0-test"

# Build a release containing assets for the targets we exercise.
setup_release "$VERSION" "linux-x86_64" "r1"
setup_release "$VERSION" "macos-universal" "r1"
BASE_URL=""
start_server
log "fake release server: $BASE_URL"

# --- 1) Platform detection: Linux x86_64 -> correct asset, dry-run ----------
log "-- test: detection linux-x86_64 (dry-run)"
home1="$tmp_root/home1"; mkdir -p "$home1"
set +e
out="$(run_installer Linux x86_64 "$home1" "" --version "$VERSION" --dry-run 2>&1)"
rc=$?
set -e
assert_eq "$rc" "0" "dry-run exits 0 (linux x86_64)"
assert_contains "$out" "dblocal-${VERSION}-linux-x86_64.tar.gz" "detects linux-x86_64 asset"
assert_contains "$out" "would install" "dry-run states intended install"
assert_contains "$out" "no files were downloaded or modified" "dry-run makes no changes"
if [ -e "$home1/.local/bin/dblocal" ]; then fail "dry-run did not install binary"; else pass "dry-run did not install binary"; fi

# --- 2) Platform detection: macOS arm64 (aarch64/arm64) -> universal asset ---
log "-- test: detection macos arm64 (dry-run)"
home2="$tmp_root/home2"; mkdir -p "$home2"
set +e
out="$(run_installer Darwin arm64 "$home2" "" --version "$VERSION" --dry-run 2>&1)"
rc=$?
set -e
assert_eq "$rc" "0" "dry-run exits 0 (macOS arm64)"
assert_contains "$out" "dblocal-${VERSION}-macos-universal.tar.gz" "detects macos-universal asset for arm64"

# --- 3) Detection via overridden uname vars (x86_64 mac) ---------------------
log "-- test: detection macos x86_64 (dry-run)"
set +e
out="$(run_installer Darwin x86_64 "$home2" "" --version "$VERSION" --dry-run 2>&1)"
rc=$?
set -e
assert_eq "$rc" "0" "dry-run exits 0 (macOS x86_64)"
assert_contains "$out" "dblocal-${VERSION}-macos-universal.tar.gz" "detects macos-universal asset for x86_64"

# --- 4) Unsupported OS/arch: clear message, installs nothing -----------------
log "-- test: unsupported OS"
set +e
out="$(run_installer FreeBSD amd64 "$home2" "" --version "$VERSION" --dry-run 2>&1)"
rc=$?
set -e
if [ "$rc" -ne 0 ]; then pass "unsupported OS exits non-zero"; else fail "unsupported OS should exit non-zero"; fi
assert_contains "$out" "unsupported operating system" "unsupported OS message is clear"
assert_not_contains "$out" "would install" "unsupported OS installs nothing"

log "-- test: unsupported arch"
set +e
out="$(run_installer Linux riscv64 "$home2" "" --version "$VERSION" --dry-run 2>&1)"
rc=$?
set -e
if [ "$rc" -ne 0 ]; then pass "unsupported arch exits non-zero"; else fail "unsupported arch should exit non-zero"; fi
assert_contains "$out" "unsupported architecture" "unsupported arch message is clear"

# --- 5) Real install into --prefix, checksum verified ------------------------
log "-- test: install + checksum verification"
prefix5="$tmp_root/prefix5"
set +e
out="$(run_installer Linux x86_64 "$home1" "$prefix5" --version "$VERSION" 2>&1)"
rc=$?
set -e
assert_eq "$rc" "0" "install exits 0"
assert_contains "$out" "verifying SHA-256" "checksum verification is reported"
assert_eq "$([ -x "$prefix5/dblocal" ] && echo yes || echo no)" "yes" "binary installed and executable"
installed_ver="$("$prefix5/dblocal")"
assert_contains "$installed_ver" "$VERSION" "installed binary runs and reports version marker"

# --- 6) Checksum mismatch rejection ------------------------------------------
log "-- test: checksum mismatch rejection"
# Corrupt the served asset (keep checksums.txt pointing at the original hash).
good_asset="$tmp_root/release/dblocal-${VERSION}-linux-x86_64.tar.gz"
cp "$good_asset" "$tmp_root/good-asset.tar.gz"
printf 'corrupted-bytes' >> "$good_asset"
prefix6="$tmp_root/prefix6"
set +e
out="$(run_installer Linux x86_64 "$home1" "$prefix6" --version "$VERSION" 2>&1)"
rc=$?
set -e
# Restore the good asset so later tests still work.
cp "$tmp_root/good-asset.tar.gz" "$good_asset"
if [ "$rc" -ne 0 ]; then pass "checksum mismatch exits non-zero"; else fail "checksum mismatch should exit non-zero"; fi
assert_contains "$out" "checksum mismatch" "checksum mismatch is reported clearly"
if [ -e "$prefix6/dblocal" ]; then fail "checksum mismatch must not install binary"; else pass "checksum mismatch installs nothing"; fi

# --- 7) Idempotent upgrade in place ------------------------------------------
log "-- test: idempotent upgrade"
prefix7="$tmp_root/prefix7"
# First install (r1, already served).
set +e
out="$(run_installer Linux x86_64 "$home1" "$prefix7" --version "$VERSION" 2>&1)"
rc=$?
set -e
assert_eq "$rc" "0" "first install exits 0"
before="$("$prefix7/dblocal")"
assert_contains "$before" "r1" "first install has r1 marker"

# Replace the release with a new build (r2) for the same tag, re-sign checksums.
setup_release "$VERSION" "linux-x86_64" "r2"
# Also refresh macOS asset (server dir changed content; keep both present).
setup_release "$VERSION" "macos-universal" "r1"

set +e
out="$(run_installer Linux x86_64 "$home1" "$prefix7" --version "$VERSION" 2>&1)"
rc=$?
set -e
assert_eq "$rc" "0" "second (idempotent) install exits 0"
after="$("$prefix7/dblocal")"
assert_contains "$after" "r2" "idempotent re-run upgrades binary in place"
assert_not_contains "$after" "r1" "upgraded binary replaced previous contents"

# --- 8) sudo is never invoked without --sudo ---------------------------------
log "-- test: never calls sudo without --sudo"
# Create a fake sudo that records any invocation. Run from an unwritable
# /usr/local/bin scenario by forcing HOME empty and a prefix that needs
# creation but is writable by us — we instead assert the fake sudo was not
# touched during a normal successful install.
fake_bin="$tmp_root/fakebin"
mkdir -p "$fake_bin"
sudo_marker="$tmp_root/sudo.called"
cat > "$fake_bin/sudo" <<'EOF'
#!/bin/sh
echo called >> "$SUDO_MARKER_FILE"
exit 99
EOF
chmod +x "$fake_bin/sudo"
prefix8="$tmp_root/prefix8"
set +e
out="$(env \
  DBLOCAL_RELEASE_BASE_URL="$BASE_URL" \
  DBLOCAL_TEST_UNAME_S=Linux \
  DBLOCAL_TEST_UNAME_M=x86_64 \
  HOME="$home1" \
  SUDO_MARKER_FILE="$sudo_marker" \
  PATH="$fake_bin:/usr/bin:/bin" \
  sh "$installer" --prefix "$prefix8" --version "$VERSION" 2>&1)"
rc=$?
set -e
assert_eq "$rc" "0" "install without --sudo succeeds when prefix is writable"
if [ -e "$sudo_marker" ]; then fail "sudo was invoked without --sudo"; else pass "sudo not invoked without --sudo"; fi

# --- D1) upgrade over a running binary (Text file busy) ----------------------
# DBLocal runs as a long-lived tray app (D-04). Replacing it must not fail when
# the existing binary is currently executing; the old cp-based install_file did.
log "-- test: D1 upgrade over a running binary (Text file busy)"
prefix_d1="$tmp_root/prefix_d1"
mkdir -p "$prefix_d1"

# Install r1 normally first.
set +e
out="$(run_installer Linux x86_64 "$home1" "$prefix_d1" --version "$VERSION" 2>&1)"
rc=$?
set -e
assert_eq "$rc" "0" "D1: initial install succeeds"

# Replace the installed file with a real long-running ELF/executable so that
# Linux applies ETXTBSY when something tries to overwrite it. Keep it running
# across the installer invocation.
busy_src="/bin/sleep"
if [ ! -x "$busy_src" ]; then busy_src="$(command -v sleep 2>/dev/null || true)"; fi
if [ -z "$busy_src" ] || [ ! -x "$busy_src" ]; then
  fail "D1: could not find a long-running executable to simulate a running dblocal"
else
  cp "$busy_src" "$prefix_d1/dblocal"
  chmod 0755 "$prefix_d1/dblocal"
  "$prefix_d1/dblocal" 60 >/dev/null 2>&1 &
  busy_pid=$!
  sleep 0.2

  # Serve an upgraded release (r2) for the same tag.
  setup_release "$VERSION" "linux-x86_64" "r2"
  setup_release "$VERSION" "macos-universal" "r1"

  set +e
  out="$(run_installer Linux x86_64 "$home1" "$prefix_d1" --version "$VERSION" 2>&1)"
  rc=$?
  set -e

  kill "$busy_pid" >/dev/null 2>&1 || true
  wait "$busy_pid" >/dev/null 2>&1 || true

  assert_eq "$rc" "0" "D1: upgrade succeeds even when the old binary is running"
  assert_not_contains "$out" "Text file busy" "D1: no 'Text file busy' error"
  if [ -x "$prefix_d1/dblocal" ]; then
    installed_d1="$("$prefix_d1/dblocal" 2>&1 || true)"
    assert_contains "$installed_d1" "r2" "D1: installed binary has upgraded contents"
  else
    fail "D1: installed binary missing or not executable after upgrade"
  fi

  # Restore server state for tests that follow.
  setup_release "$VERSION" "linux-x86_64" "r1"
  setup_release "$VERSION" "macos-universal" "r1"
fi

# --- D2) TLS/download safety -------------------------------------------------
log "-- test: D2 TLS downgrade prevented (no curl fallback)"
d2_bin="$tmp_root/d2bin"
mkdir -p "$d2_bin"
d2_log="$tmp_root/d2-curl.log"
cat > "$d2_bin/curl" <<'EOF'
#!/bin/sh
# Fake curl: record every invocation. Decide "strict" by the presence of BOTH a
# --proto flag (with =https in its value) and --tlsv1.2, whatever their shape.
# A call without them is the old weak fallback: record WEAK and succeed.
printf '%s\n' "$*" >> "$D2_LOG"
have_proto_https=0
have_tls12=0
prev=""
for a in "$@"; do
  case "$a" in
    --proto) [ "$prev" = "--proto" ] || true ;;  # placeholder; handled below
  esac
  case "$a" in
    =*https*) have_proto_https=1 ;;
    --tlsv1.2) have_tls12=1 ;;
  esac
  prev="$a"
done
if [ "$have_proto_https" -eq 1 ] && [ "$have_tls12" -eq 1 ]; then
  # Strict invocation: simulate a failure so a fallback would be observable.
  exit 22
fi
echo WEAK >> "$D2_LOG"
out=""; prev=""
for a in "$@"; do if [ "$prev" = "-o" ]; then out="$a"; fi; prev="$a"; done
[ -n "$out" ] && printf 'weak' > "$out"
exit 0
EOF
chmod +x "$d2_bin/curl"
prefix_d2="$tmp_root/prefix_d2"
set +e
out="$(env \
  DBLOCAL_RELEASE_BASE_URL="$BASE_URL" \
  DBLOCAL_TEST_UNAME_S=Linux \
  DBLOCAL_TEST_UNAME_M=x86_64 \
  DBLOCAL_TEST_SKIP_CURL_CAPS=1 \
  HOME="$home1" \
  PATH="$d2_bin:/usr/bin:/bin" \
  D2_LOG="$d2_log" \
  sh "$installer" --prefix "$prefix_d2" --version "$VERSION" 2>&1)"
rc=$?
set -e
if [ "$rc" -ne 0 ]; then pass "D2: installer fails when strict TLS curl fails (no weak fallback)"; else fail "D2: installer must not silently downgrade TLS"; fi
if [ -f "$d2_log" ] && grep -q '^WEAK$' "$d2_log"; then
  fail "D2: weak curl fallback was invoked"
else
  pass "D2: no weak curl fallback invocation recorded"
fi
if [ -e "$prefix_d2/dblocal" ]; then fail "D2: nothing installed on TLS failure"; else pass "D2: nothing installed on TLS failure"; fi

log "-- test: D2 non-local http DBLOCAL_RELEASE_BASE_URL rejected"
set +e
out="$(env \
  DBLOCAL_RELEASE_BASE_URL="http://example.com/releases" \
  DBLOCAL_TEST_UNAME_S=Linux \
  DBLOCAL_TEST_UNAME_M=x86_64 \
  HOME="$home1" \
  PATH=/usr/bin:/bin \
  sh "$installer" --dry-run --version "$VERSION" 2>&1)"
rc=$?
set -e
if [ "$rc" -ne 0 ]; then pass "D2: non-local http base URL is rejected"; else fail "D2: non-local http base URL must be rejected"; fi
assert_contains "$out" "https" "D2: rejection explains https requirement"

log "-- test: D2 localhost http base URL allowed (offline tests)"
localhost_base="$(printf '%s\n' "$BASE_URL" | sed 's#127\.0\.0\.1#localhost#')"
set +e
out="$(env \
  DBLOCAL_RELEASE_BASE_URL="$localhost_base" \
  DBLOCAL_TEST_UNAME_S=Linux \
  DBLOCAL_TEST_UNAME_M=x86_64 \
  HOME="$home1" \
  PATH=/usr/bin:/bin \
  sh "$installer" --dry-run --version "$VERSION" 2>&1)"
rc=$?
set -e
assert_eq "$rc" "0" "D2: localhost http base URL is allowed"
assert_contains "$out" "dry-run" "D2: localhost http dry-run proceeds"

# --- D3) trap behavior on signals --------------------------------------------
# POSIX sh defers INT/TERM while blocked in a foreground child, so we make the
# signal deterministic instead: a fake `curl` signals its parent and returns.
# The old handler returned (script continued, exit 0); the fixed one exits 143.
log "-- test: D3 TERM trap exits 143 and does not continue"
d3_bin="$tmp_root/d3bin"
mkdir -p "$d3_bin"
cat > "$d3_bin/curl" <<'EOF'
#!/bin/sh
# Simulate the installer receiving SIGTERM mid-download, then return.
kill -TERM "$PPID" 2>/dev/null || true
exit 0
EOF
chmod +x "$d3_bin/curl"
d3_prefix="$tmp_root/prefix_d3"
mkdir -p "$d3_prefix"
set +e
out="$(env \
  DBLOCAL_RELEASE_BASE_URL="$BASE_URL" \
  DBLOCAL_TEST_UNAME_S=Linux \
  DBLOCAL_TEST_UNAME_M=x86_64 \
  DBLOCAL_TEST_SKIP_CURL_CAPS=1 \
  HOME="$home1" \
  PATH="$d3_bin:/usr/bin:/bin" \
  timeout 20 sh "$installer" --prefix "$d3_prefix" --version "$VERSION" 2>&1)"
rc=$?
set -e
assert_eq "$rc" "143" "D3: TERM results in exit 143"
assert_not_contains "$out" "installed dblocal" "D3: script did not continue after TERM"
if [ -e "$d3_prefix/dblocal" ]; then fail "D3: nothing installed after TERM"; else pass "D3: nothing installed after TERM"; fi

# --- D4) extract only the expected member, reject symlink --------------------
# The old code ran a blanket `tar -xzf ... -C extract`, which happily materialises
# unexpected members. Two independent cases:
#   (a) 'dblocal' is a symlink -> must be refused (old code extracted it and
#       installed through the link).
#   (b) the archive also ships an extra member -> only 'dblocal' should land.
log "-- test: D4(symlink) archive whose 'dblocal' is a symlink is rejected"
d4a_rel="$tmp_root/d4a"
mkdir -p "$d4a_rel"
python3 - "$d4a_rel/dblocal-${VERSION}-linux-x86_64.tar.gz" <<'PY'
import io, sys, tarfile
with tarfile.open(sys.argv[1], "w:gz") as tf:
    data = b"#!/bin/sh\necho pwned\n"
    ti = tarfile.TarInfo("sub/evil"); ti.size = len(data); ti.mode = 0o755
    tf.addfile(ti, io.BytesIO(data))
    ti = tarfile.TarInfo("dblocal"); ti.type = tarfile.SYMTYPE; ti.linkname = "sub/evil"
    tf.addfile(ti)
PY
(
  cd "$d4a_rel"
  printf '%s  %s\n' "$(sha256_file "dblocal-${VERSION}-linux-x86_64.tar.gz")" "dblocal-${VERSION}-linux-x86_64.tar.gz" > checksums.txt
)
d4a_port="$(pick_free_port)"
(
  cd "$d4a_rel" || exit 1
  exec python3 -m http.server "$d4a_port" --bind 127.0.0.1
) </dev/null >/dev/null 2>&1 &
d4a_spid=$!
for _ in $(seq 1 50); do curl -fsS "http://127.0.0.1:${d4a_port}/checksums.txt" >/dev/null 2>&1 && break; sleep 0.1; done
prefix_d4a="$tmp_root/prefix_d4a"
set +e
out="$(env \
  DBLOCAL_RELEASE_BASE_URL="http://127.0.0.1:${d4a_port}" \
  DBLOCAL_TEST_UNAME_S=Linux DBLOCAL_TEST_UNAME_M=x86_64 \
  HOME="$home1" PATH=/usr/bin:/bin \
  sh "$installer" --prefix "$prefix_d4a" --version "$VERSION" 2>&1)"
rc=$?
set -e
kill "$d4a_spid" >/dev/null 2>&1 || true
wait "$d4a_spid" >/dev/null 2>&1 || true
if [ "$rc" -ne 0 ]; then pass "D4(symlink): symlinked dblocal is rejected"; else fail "D4(symlink): symlinked dblocal must be rejected"; fi
if [ -e "$prefix_d4a/dblocal" ] || [ -L "$prefix_d4a/dblocal" ]; then fail "D4(symlink): nothing installed"; else pass "D4(symlink): nothing installed"; fi

log "-- test: D4(extra members) only 'dblocal' is extracted"
d4b_rel="$tmp_root/d4b"
d4b_extract="$tmp_root/d4b-extract"
mkdir -p "$d4b_rel" "$d4b_extract"
python3 - "$d4b_rel/dblocal-${VERSION}-linux-x86_64.tar.gz" <<'PY'
import io, sys, tarfile
with tarfile.open(sys.argv[1], "w:gz") as tf:
    data = b"#!/bin/sh\necho only-me\n"
    ti = tarfile.TarInfo("dblocal"); ti.size = len(data); ti.mode = 0o755
    tf.addfile(ti, io.BytesIO(data))
    for name in ("extra-tool", "nested/thing"):
        ti = tarfile.TarInfo(name); ti.size = 4
        tf.addfile(ti, io.BytesIO(b"junk"))
PY
# Assert the script's extraction strategy: only `dblocal` lands.
tar -xzf "$d4b_rel/dblocal-${VERSION}-linux-x86_64.tar.gz" -C "$d4b_extract" -- dblocal
if [ -f "$d4b_extract/dblocal" ] && [ ! -e "$d4b_extract/extra-tool" ] && [ ! -e "$d4b_extract/nested" ]; then
  pass "D4(extra members): tar extracts only 'dblocal'"
else
  fail "D4(extra members): unexpected members extracted"
fi

# End-to-end: a release whose archive carries extra members installs cleanly and
# leaves nothing else behind in the install prefix.
(
  cd "$d4b_rel"
  printf '%s  %s\n' "$(sha256_file "dblocal-${VERSION}-linux-x86_64.tar.gz")" "dblocal-${VERSION}-linux-x86_64.tar.gz" > checksums.txt
)
d4b_port="$(pick_free_port)"
(
  cd "$d4b_rel" || exit 1
  exec python3 -m http.server "$d4b_port" --bind 127.0.0.1
) </dev/null >/dev/null 2>&1 &
d4b_spid=$!
for _ in $(seq 1 50); do curl -fsS "http://127.0.0.1:${d4b_port}/checksums.txt" >/dev/null 2>&1 && break; sleep 0.1; done
prefix_d4b="$tmp_root/prefix_d4b"
set +e
out="$(env \
  DBLOCAL_RELEASE_BASE_URL="http://127.0.0.1:${d4b_port}" \
  DBLOCAL_TEST_UNAME_S=Linux DBLOCAL_TEST_UNAME_M=x86_64 \
  HOME="$home1" PATH=/usr/bin:/bin \
  sh "$installer" --prefix "$prefix_d4b" --version "$VERSION" 2>&1)"
rc=$?
set -e
kill "$d4b_spid" >/dev/null 2>&1 || true
wait "$d4b_spid" >/dev/null 2>&1 || true
assert_eq "$rc" "0" "D4(extra members): install succeeds"
if [ -x "$prefix_d4b/dblocal" ]; then pass "D4(extra members): binary installed"; else fail "D4(extra members): binary installed"; fi
leftover="$(find "$prefix_d4b" -mindepth 1 ! -name dblocal -print -quit 2>/dev/null || true)"
assert_eq "$leftover" "" "D4(extra members): nothing but 'dblocal' in prefix"

# --- D5) version tag validation and literal checksum match -------------------
log "-- test: D5 invalid --version with shell metachar rejected"
set +e
out="$(env \
  DBLOCAL_RELEASE_BASE_URL="$BASE_URL" \
  DBLOCAL_TEST_UNAME_S=Linux \
  DBLOCAL_TEST_UNAME_M=x86_64 \
  HOME="$home1" \
  PATH=/usr/bin:/bin \
  sh "$installer" --dry-run --version 'v1.2.3;rm -rf /' 2>&1)"
rc=$?
set -e
if [ "$rc" -ne 0 ]; then pass "D5: invalid --version with shell metachar is rejected"; else fail "D5: invalid --version must be rejected"; fi
assert_contains "$out" "version" "D5: rejection mentions version/tag"

log "-- test: D5 malformed --version (regex dot bypass) rejected"
set +e
out="$(env \
  DBLOCAL_RELEASE_BASE_URL="$BASE_URL" \
  DBLOCAL_TEST_UNAME_S=Linux \
  DBLOCAL_TEST_UNAME_M=x86_64 \
  HOME="$home1" \
  PATH=/usr/bin:/bin \
  sh "$installer" --dry-run --version 'v1x2x3' 2>&1)"
rc=$?
set -e
if [ "$rc" -ne 0 ]; then pass "D5: malformed version tag is rejected"; else fail "D5: malformed version tag must be rejected"; fi

log "-- test: D5 checksum lookup is literal (no regex wildcard match)"
# Real asset is dblocal-v0.1.0-test-linux-x86_64.tar.gz. checksums.txt lists a
# spoof name where '.' are replaced by 'X' — this would match only if the asset
# name were interpolated into a regex ('.' matches any char), not with a literal
# comparison.
d5_rel="$tmp_root/d5release"
mkdir -p "$d5_rel/stage"
printf '#!/bin/sh\necho ok\n' > "$d5_rel/stage/dblocal"; chmod +x "$d5_rel/stage/dblocal"
tar -C "$d5_rel/stage" -czf "$d5_rel/dblocal-${VERSION}-linux-x86_64.tar.gz" dblocal
real_hash="$(sha256_file "$d5_rel/dblocal-${VERSION}-linux-x86_64.tar.gz")"
spoof_name="dblocal-v0X1X0-test-linux-x86_64.tar.gz"
printf '%s  %s\n' "$real_hash" "$spoof_name" > "$d5_rel/checksums.txt"
d5_port="$(pick_free_port)"
(
  cd "$d5_rel" || exit 1
  exec python3 -m http.server "$d5_port" --bind 127.0.0.1
) </dev/null >/dev/null 2>&1 &
d5_spid=$!
for _ in $(seq 1 50); do curl -fsS "http://127.0.0.1:${d5_port}/checksums.txt" >/dev/null 2>&1 && break; sleep 0.1; done
set +e
out="$(env \
  DBLOCAL_RELEASE_BASE_URL="http://127.0.0.1:${d5_port}" \
  DBLOCAL_TEST_UNAME_S=Linux \
  DBLOCAL_TEST_UNAME_M=x86_64 \
  HOME="$home1" \
  PATH=/usr/bin:/bin \
  sh "$installer" --prefix "$tmp_root/prefix_d5" --version "$VERSION" 2>&1)"
rc=$?
set -e
kill "$d5_spid" >/dev/null 2>&1 || true
wait "$d5_spid" >/dev/null 2>&1 || true
if [ "$rc" -ne 0 ]; then pass "D5: checksum lookup is literal (regex-spoofed name rejected)"; else fail "D5: checksum lookup must not treat asset name as a regex"; fi

# --- Tarball desktop entry and icon installation -----------------------------
log "-- test: tarball with share/ installs desktop entry, icons, and rewrites Exec"
icon_rel="$tmp_root/icon-release"
mkdir -p "$icon_rel/stage/share/applications" "$icon_rel/stage/share/icons/hicolor/scalable/apps"
printf '#!/bin/sh\necho "dblocal-icon-test"\n' > "$icon_rel/stage/dblocal"
chmod +x "$icon_rel/stage/dblocal"
cp "$repo_root/packaging/linux/dev.dblocal.DBLocal.desktop" "$icon_rel/stage/share/applications/dev.dblocal.DBLocal.desktop"
cp "$repo_root/packaging/linux/dblocal.svg" "$icon_rel/stage/share/icons/hicolor/scalable/apps/dblocal.svg"
for sz in 16 24 32 48 64 128 256 512; do
  mkdir -p "$icon_rel/stage/share/icons/hicolor/${sz}x${sz}/apps"
  cp "$repo_root/packaging/icons/png/dblocal-${sz}.png" \
    "$icon_rel/stage/share/icons/hicolor/${sz}x${sz}/apps/dblocal.png"
done
tar -C "$icon_rel/stage" -czf "$icon_rel/dblocal-${VERSION}-linux-x86_64.tar.gz" dblocal share
rm -rf "$icon_rel/stage"
(
  cd "$icon_rel"
  printf '%s  %s\n' "$(sha256_file "dblocal-${VERSION}-linux-x86_64.tar.gz")" "dblocal-${VERSION}-linux-x86_64.tar.gz" > checksums.txt
)
icon_port="$(pick_free_port)"
(
  cd "$icon_rel" || exit 1
  exec python3 -m http.server "$icon_port" --bind 127.0.0.1
) </dev/null >/dev/null 2>&1 &
icon_spid=$!
for _ in $(seq 1 50); do curl -fsS "http://127.0.0.1:${icon_port}/checksums.txt" >/dev/null 2>&1 && break; sleep 0.1; done

home_icon_dry="$tmp_root/home-icon-dry"
prefix_icon_dry="$tmp_root/prefix-icon-dry"
mkdir -p "$home_icon_dry"
set +e
out="$(env \
  DBLOCAL_RELEASE_BASE_URL="http://127.0.0.1:${icon_port}" \
  DBLOCAL_TEST_UNAME_S=Linux DBLOCAL_TEST_UNAME_M=x86_64 \
  HOME="$home_icon_dry" PATH=/usr/bin:/bin \
  sh "$installer" --prefix "$prefix_icon_dry" --version "$VERSION" --dry-run 2>&1)"
rc=$?
set -e
assert_eq "$rc" "0" "tarball share/: dry-run exits 0"
if [ -e "$prefix_icon_dry/dblocal" ] || [ -e "$home_icon_dry/.local/share/applications/dev.dblocal.DBLocal.desktop" ]; then
  fail "tarball share/: dry-run must not write binary or desktop file"
else
  pass "tarball share/: dry-run writes nothing"
fi

home_icon="$tmp_root/home-icon"
prefix_icon="$tmp_root/prefix-icon"
mkdir -p "$home_icon"
set +e
out="$(env \
  DBLOCAL_RELEASE_BASE_URL="http://127.0.0.1:${icon_port}" \
  DBLOCAL_TEST_UNAME_S=Linux DBLOCAL_TEST_UNAME_M=x86_64 \
  HOME="$home_icon" PATH=/usr/bin:/bin \
  sh "$installer" --prefix "$prefix_icon" --version "$VERSION" 2>&1)"
rc=$?
set -e

assert_eq "$rc" "0" "tarball share/: install exits 0"
installed_desktop="$home_icon/.local/share/applications/dev.dblocal.DBLocal.desktop"
if [ -f "$installed_desktop" ]; then
  pass "tarball share/: desktop file installed"
  desktop_content="$(cat "$installed_desktop")"
  assert_contains "$desktop_content" "Exec=${prefix_icon}/dblocal" "tarball share/: Exec rewritten to absolute binary path"
  assert_contains "$desktop_content" "TryExec=${prefix_icon}/dblocal" "tarball share/: TryExec rewritten to absolute binary path"
else
  fail "tarball share/: desktop file missing at $installed_desktop"
fi
if [ -f "$home_icon/.local/share/icons/hicolor/scalable/apps/dblocal.svg" ] \
  && [ -f "$home_icon/.local/share/icons/hicolor/256x256/apps/dblocal.png" ]; then
  pass "tarball share/: SVG and PNG icons installed"
else
  fail "tarball share/: expected SVG and 256px PNG icons under $home_icon/.local/share/icons/hicolor"
fi

# Explicit XDG_DATA_HOME is honoured by install.sh.
log "-- test: tarball share/ honours explicit XDG_DATA_HOME"
home_xdg="$tmp_root/home-xdg"
xdg_data="$tmp_root/xdg-data"
prefix_xdg="$tmp_root/prefix-xdg"
mkdir -p "$home_xdg" "$xdg_data"
set +e
out="$(env \
  DBLOCAL_RELEASE_BASE_URL="http://127.0.0.1:${icon_port}" \
  DBLOCAL_TEST_UNAME_S=Linux DBLOCAL_TEST_UNAME_M=x86_64 \
  HOME="$home_xdg" XDG_DATA_HOME="$xdg_data" PATH=/usr/bin:/bin \
  sh "$installer" --prefix "$prefix_xdg" --version "$VERSION" 2>&1)"
rc=$?
set -e
assert_eq "$rc" "0" "tarball share/ (XDG_DATA_HOME): install exits 0"
if [ -f "$xdg_data/applications/dev.dblocal.DBLocal.desktop" ] \
  && [ -f "$xdg_data/icons/hicolor/scalable/apps/dblocal.svg" ] \
  && [ ! -e "$home_xdg/.local/share/applications/dev.dblocal.DBLocal.desktop" ]; then
  pass "tarball share/ (XDG_DATA_HOME): desktop file and icons installed under XDG_DATA_HOME"
else
  fail "tarball share/ (XDG_DATA_HOME): expected files under $xdg_data and not under $home_xdg/.local/share"
fi

# Icon theme cache handling: never create a user-level icon-theme.cache when
# none existed; touch the theme directory; refresh only when cache already exists.
log "-- test: tarball share/ icon-theme.cache handling (no cache vs existing cache)"
ic_bin="$tmp_root/ic-fakebin"
mkdir -p "$ic_bin"
cat > "$ic_bin/gtk-update-icon-cache" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >> "$IC_CACHE_LOG"
exit 0
EOF
chmod +x "$ic_bin/gtk-update-icon-cache"

# (a) No cache beforehand: tool not called, no cache created, hicolor mtime updated.
home_nocache="$tmp_root/home-nocache"
xdg_nocache="$tmp_root/xdg-nocache"
prefix_nocache="$tmp_root/prefix-nocache"
ic_log_no="$tmp_root/ic-nocache.log"
old_ref="$tmp_root/old-mtime-ref"
mkdir -p "$home_nocache" "$xdg_nocache/icons/hicolor"
touch -t 202001010000 "$old_ref" "$xdg_nocache/icons/hicolor"
set +e
out="$(env \
  DBLOCAL_RELEASE_BASE_URL="http://127.0.0.1:${icon_port}" \
  DBLOCAL_TEST_UNAME_S=Linux DBLOCAL_TEST_UNAME_M=x86_64 \
  HOME="$home_nocache" XDG_DATA_HOME="$xdg_nocache" \
  IC_CACHE_LOG="$ic_log_no" PATH="$ic_bin:/usr/bin:/bin" \
  sh "$installer" --prefix "$prefix_nocache" --version "$VERSION" 2>&1)"
rc=$?
set -e
assert_eq "$rc" "0" "icon-cache (no cache): install exits 0"
if [ ! -s "$ic_log_no" ]; then
  pass "icon-cache (no cache): gtk-update-icon-cache not called"
else
  fail "icon-cache (no cache): gtk-update-icon-cache should not be called (got: $(cat "$ic_log_no"))"
fi
if [ ! -e "$xdg_nocache/icons/hicolor/icon-theme.cache" ]; then
  pass "icon-cache (no cache): no icon-theme.cache created"
else
  fail "icon-cache (no cache): icon-theme.cache must not be created"
fi
if [ "$xdg_nocache/icons/hicolor" -nt "$old_ref" ]; then
  pass "icon-cache (no cache): hicolor directory mtime updated"
else
  fail "icon-cache (no cache): hicolor directory mtime was not updated"
fi

# (b) Existing cache: tool called once for that directory.
home_hascache="$tmp_root/home-hascache"
xdg_hascache="$tmp_root/xdg-hascache"
prefix_hascache="$tmp_root/prefix-hascache"
ic_log_yes="$tmp_root/ic-hascache.log"
mkdir -p "$home_hascache" "$xdg_hascache/icons/hicolor"
: > "$xdg_hascache/icons/hicolor/icon-theme.cache"
set +e
out="$(env \
  DBLOCAL_RELEASE_BASE_URL="http://127.0.0.1:${icon_port}" \
  DBLOCAL_TEST_UNAME_S=Linux DBLOCAL_TEST_UNAME_M=x86_64 \
  HOME="$home_hascache" XDG_DATA_HOME="$xdg_hascache" \
  IC_CACHE_LOG="$ic_log_yes" PATH="$ic_bin:/usr/bin:/bin" \
  sh "$installer" --prefix "$prefix_hascache" --version "$VERSION" 2>&1)"
rc=$?
set -e
assert_eq "$rc" "0" "icon-cache (existing cache): install exits 0"
ic_calls="$( [ -f "$ic_log_yes" ] && wc -l < "$ic_log_yes" | tr -d '[:space:]' || echo 0 )"
assert_eq "$ic_calls" "1" "icon-cache (existing cache): gtk-update-icon-cache called once"
if [ -f "$ic_log_yes" ]; then
  assert_contains "$(cat "$ic_log_yes")" "$xdg_hascache/icons/hicolor" "icon-cache (existing cache): called for hicolor dir"
else
  fail "icon-cache (existing cache): log missing"
fi

# (c) Tarball with desktop file but no icons, $XDG_DATA_HOME/icons exists without hicolor:
# must NOT create a regular file named hicolor.
log "-- test: tarball with desktop file but no icons does not create regular file 'hicolor'"
noicons_rel="$tmp_root/noicons-release"
mkdir -p "$noicons_rel/stage/share/applications"
printf '#!/bin/sh\necho "dblocal-noicons"\n' > "$noicons_rel/stage/dblocal"
chmod +x "$noicons_rel/stage/dblocal"
cp "$repo_root/packaging/linux/dev.dblocal.DBLocal.desktop" "$noicons_rel/stage/share/applications/dev.dblocal.DBLocal.desktop"
tar -C "$noicons_rel/stage" -czf "$noicons_rel/dblocal-${VERSION}-linux-x86_64.tar.gz" dblocal share
rm -rf "$noicons_rel/stage"
(
  cd "$noicons_rel"
  printf '%s  %s\n' "$(sha256_file "dblocal-${VERSION}-linux-x86_64.tar.gz")" "dblocal-${VERSION}-linux-x86_64.tar.gz" > checksums.txt
)
noicons_port="$(pick_free_port)"
(
  cd "$noicons_rel" || exit 1
  exec python3 -m http.server "$noicons_port" --bind 127.0.0.1
) </dev/null >/dev/null 2>&1 &
noicons_spid=$!
for _ in $(seq 1 50); do curl -fsS "http://127.0.0.1:${noicons_port}/checksums.txt" >/dev/null 2>&1 && break; sleep 0.1; done
home_noicons="$tmp_root/home-noicons"
xdg_noicons="$tmp_root/xdg-noicons"
prefix_noicons="$tmp_root/prefix-noicons"
mkdir -p "$home_noicons" "$xdg_noicons/icons"
set +e
out="$(env \
  DBLOCAL_RELEASE_BASE_URL="http://127.0.0.1:${noicons_port}" \
  DBLOCAL_TEST_UNAME_S=Linux DBLOCAL_TEST_UNAME_M=x86_64 \
  HOME="$home_noicons" XDG_DATA_HOME="$xdg_noicons" PATH=/usr/bin:/bin \
  sh "$installer" --prefix "$prefix_noicons" --version "$VERSION" 2>&1)"
rc=$?
set -e
kill "$noicons_spid" >/dev/null 2>&1 || true
wait "$noicons_spid" >/dev/null 2>&1 || true
assert_eq "$rc" "0" "icon-cache (no icons in tarball): install exits 0"
if [ ! -f "$xdg_noicons/icons/hicolor" ]; then
  pass "icon-cache (no icons in tarball): no regular file named hicolor created"
else
  fail "icon-cache (no icons in tarball): regular file 'hicolor' was created"
fi

# Path with space: desktop/icon install skipped with warning, binary still installed.
log "-- test: tarball share/ skips desktop install on path with space"
home_space="$tmp_root/home-space"
prefix_space="$tmp_root/prefix with space"
mkdir -p "$home_space"
set +e
out="$(env \
  DBLOCAL_RELEASE_BASE_URL="http://127.0.0.1:${icon_port}" \
  DBLOCAL_TEST_UNAME_S=Linux DBLOCAL_TEST_UNAME_M=x86_64 \
  HOME="$home_space" PATH=/usr/bin:/bin \
  sh "$installer" --prefix "$prefix_space" --version "$VERSION" 2>&1)"
rc=$?
set -e
assert_eq "$rc" "0" "tarball share/ (space path): install exits 0"
assert_contains "$out" "skipping desktop entry and icon install" "tarball share/ (space path): warns about skipped desktop install"
if [ -x "$prefix_space/dblocal" ] && [ ! -e "$home_space/.local/share/applications/dev.dblocal.DBLocal.desktop" ]; then
  pass "tarball share/ (space path): binary installed and desktop file skipped"
else
  fail "tarball share/ (space path): unexpected state"
fi

# Path with backslash: skipped cleanly without corrupting Exec, binary still installed.
log "-- test: tarball share/ skips desktop install on path with backslash"
home_bs="$tmp_root/home-bs"
prefix_bs="$tmp_root/prefix\\nbs"
mkdir -p "$home_bs"
set +e
out="$(env \
  DBLOCAL_RELEASE_BASE_URL="http://127.0.0.1:${icon_port}" \
  DBLOCAL_TEST_UNAME_S=Linux DBLOCAL_TEST_UNAME_M=x86_64 \
  HOME="$home_bs" PATH=/usr/bin:/bin \
  sh "$installer" --prefix "$prefix_bs" --version "$VERSION" 2>&1)"
rc=$?
set -e
assert_eq "$rc" "0" "tarball share/ (backslash path): install exits 0"
assert_contains "$out" "skipping desktop entry and icon install" "tarball share/ (backslash path): warns about skipped desktop install"
if [ -x "$prefix_bs/dblocal" ] && [ ! -e "$home_bs/.local/share/applications/dev.dblocal.DBLocal.desktop" ]; then
  pass "tarball share/ (backslash path): binary installed and desktop file skipped"
else
  fail "tarball share/ (backslash path): unexpected state"
fi

# Old v0.1.1 install.sh against the new tarball (with share/) still installs binary cleanly.
log "-- test: v0.1.1 install.sh installs cleanly from new tarball with share/"
if git -C "$repo_root" rev-parse --verify v0.1.1 >/dev/null 2>&1; then
  v011_installer="$tmp_root/install-v011.sh"
  git -C "$repo_root" show v0.1.1:packaging/install.sh > "$v011_installer"
  chmod +x "$v011_installer"
  home_v011="$tmp_root/home-v011"
  prefix_v011="$tmp_root/prefix-v011"
  mkdir -p "$home_v011"
  set +e
  out="$(env \
    DBLOCAL_RELEASE_BASE_URL="http://127.0.0.1:${icon_port}" \
    DBLOCAL_TEST_UNAME_S=Linux DBLOCAL_TEST_UNAME_M=x86_64 \
    HOME="$home_v011" PATH=/usr/bin:/bin \
    sh "$v011_installer" --prefix "$prefix_v011" --version "$VERSION" 2>&1)"
  rc=$?
  set -e
  assert_eq "$rc" "0" "v0.1.1 install.sh: exits 0 on new tarball with share/"
  if [ -x "$prefix_v011/dblocal" ]; then
    pass "v0.1.1 install.sh: binary installed from new tarball"
  else
    fail "v0.1.1 install.sh: binary missing"
  fi
fi

kill "$icon_spid" >/dev/null 2>&1 || true
wait "$icon_spid" >/dev/null 2>&1 || true

# Upgrade from an old tarball without share/ still works cleanly.
log "-- test: tarball without share/ (older release) installs cleanly"
home_old="$tmp_root/home-old"
prefix_old="$tmp_root/prefix-old"
mkdir -p "$home_old"
set +e
out="$(run_installer Linux x86_64 "$home_old" "$prefix_old" --version "$VERSION" 2>&1)"
rc=$?
set -e
assert_eq "$rc" "0" "tarball without share/: install exits 0"
if [ -x "$prefix_old/dblocal" ] && [ ! -e "$home_old/.local/share/applications/dev.dblocal.DBLocal.desktop" ]; then
  pass "tarball without share/: binary installed and share/ skipped quietly"
else
  fail "tarball without share/: unexpected state in $prefix_old or $home_old"
fi

# --- W3-E-fix1/fix2: packaging version/libc/recommends regression tests -------
# Sourced so this harness stays under the 800-line file limit.
# shellcheck source=packaging/test/packaging_regressions.sh
. "$script_dir/packaging_regressions.sh"

# --- summary -----------------------------------------------------------------
log ""
log "== results: ${pass_count} passed, ${fail_count} failed =="
if [ "$fail_count" -ne 0 ]; then
  exit 1
fi
exit 0
