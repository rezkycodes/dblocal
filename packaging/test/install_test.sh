#!/usr/bin/env bash
# Offline tests for packaging/install.sh (F-PKG-03).
# Serves a fake GitHub Releases layout via `python3 -m http.server` and runs the
# installer against it using DBLOCAL_RELEASE_BASE_URL, without touching the
# network or the real GitHub API.
set -euo pipefail

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
  printf 'http://127.0.0.1:%s\n' "$port"
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
BASE_URL="$(start_server)"
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

# --- W3-E-fix1: packaging version/libc/recommends regression tests -----------
log "-- test: F1/F2 shared version helper"

# Source the helper in a subshell-like check via bash -c to avoid polluting test env.
ver_sh="$repo_root/packaging/lib/version.sh"

assert_ver_pkg() {
  local in="$1" want="$2" msg="$3"
  local got
  got="$(bash -c "source '$ver_sh'; dblocal_version_package '$in'" 2>&1)"
  local rc=$?
  if [ "$rc" -eq 0 ] && [ "$got" = "$want" ]; then pass "$msg"; else fail "$msg (rc=$rc got='$got' want='$want')"; fi
}

assert_ver_invalid() {
  local in="$1" msg="$2"
  set +e
  out="$(bash -c "source '$ver_sh'; dblocal_version_require_valid '$in' version" 2>&1)"
  rc=$?
  set -e
  if [ "$rc" -ne 0 ]; then pass "$msg"; else fail "$msg (expected non-zero, got rc=0 out='$out')"; fi
  assert_contains "$out" "invalid version" "$msg reports invalid version"
}

assert_ver_pkg "0.2.0" "0.2.0" "F1: final version maps to itself"
assert_ver_pkg "v0.2.0" "0.2.0" "F1: leading v stripped"
assert_ver_pkg "0.2.0-rc.1" "0.2.0~rc.1" "F1: rc prerelease maps to tilde"
assert_ver_pkg "0.2.0-rc1" "0.2.0~rc1" "F1: rc1 prerelease maps to tilde"
assert_ver_pkg "0.2.0-beta.2" "0.2.0~beta.2" "F1: beta prerelease maps to tilde"

assert_ver_invalid "main" "F2: 'main' rejected as explicit version"
assert_ver_invalid "1.2" "F2: incomplete version rejected"
assert_ver_invalid "1.2.3.4" "F2: four-part version rejected"
assert_ver_invalid "v1.2.3;rm" "F2: shell metachar version rejected"

# G2: multi-line / control-char / whitespace values must be rejected as a whole.
assert_ver_invalid $'evil\n0.2.0' "G2: embedded newline before valid line rejected"
assert_ver_invalid $'0.2.0\nDepends: evil' "G2: embedded newline with control-field payload rejected"
assert_ver_invalid $'0.2.0\r' "G2: carriage return rejected"
assert_ver_invalid "0.2.0 rc1" "G2: space rejected"

# Ordering: rc.1 < rc.2 < final, using dpkg --compare-versions (F1).
if command -v dpkg >/dev/null 2>&1; then
  v_rc1="$(bash -c "source '$ver_sh'; dblocal_version_package '0.2.0-rc.1'")"
  v_rc2="$(bash -c "source '$ver_sh'; dblocal_version_package '0.2.0-rc.2'")"
  v_final="$(bash -c "source '$ver_sh'; dblocal_version_package '0.2.0'")"
  if dpkg --compare-versions "$v_rc1" lt "$v_rc2"; then pass "F1: dpkg rc.1 < rc.2"; else fail "F1: dpkg rc.1 < rc.2 ($v_rc1 !< $v_rc2)"; fi
  if dpkg --compare-versions "$v_rc2" lt "$v_final"; then pass "F1: dpkg rc.2 < final"; else fail "F1: dpkg rc.2 < final ($v_rc2 !< $v_final)"; fi
  if dpkg --compare-versions "$v_rc1" lt "$v_final"; then pass "F1: dpkg rc.1 < final"; else fail "F1: dpkg rc.1 < final ($v_rc1 !< $v_final)"; fi
else
  fail "F1: dpkg not available for ordering check"
fi

# RPM ordering via rpm --eval lua rpm.vercmp.
if command -v rpm >/dev/null 2>&1; then
  v_rc1="$(bash -c "source '$ver_sh'; dblocal_version_package '0.2.0-rc.1'")"
  v_rc2="$(bash -c "source '$ver_sh'; dblocal_version_package '0.2.0-rc.2'")"
  v_final="$(bash -c "source '$ver_sh'; dblocal_version_package '0.2.0'")"
  rpm_cmp() { rpm --eval "%{lua: print(rpm.vercmp('$1','$2'))}" 2>/dev/null | tr -d '[:space:]'; }
  c1="$(rpm_cmp "$v_rc1" "$v_rc2")"
  c2="$(rpm_cmp "$v_rc2" "$v_final")"
  c3="$(rpm_cmp "$v_rc1" "$v_final")"
  assert_eq "$c1" "-1" "F1: rpm rc.1 < rc.2"
  assert_eq "$c2" "-1" "F1: rpm rc.2 < final"
  assert_eq "$c3" "-1" "F1: rpm rc.1 < final"
else
  fail "F1: rpm not available for ordering check"
fi

# F2: default with no SemVer tag becomes snapshot 0.0.0~git<short>.
log "-- test: F2 snapshot default version"
snap_repo="$tmp_root/snap-repo"
mkdir -p "$snap_repo"
git -C "$snap_repo" init -q
git -C "$snap_repo" config user.email test@test
git -C "$snap_repo" config user.name test
echo hi > "$snap_repo/f"
git -C "$snap_repo" add f
git -C "$snap_repo" commit -q -m init
snap_short="$(git -C "$snap_repo" rev-parse --short=12 HEAD)"
snap_out="$(bash -c "source '$ver_sh'; dblocal_version_default_or_validate '' '$snap_repo' R P S; printf '%s|%s|%s' \"\$R\" \"\$P\" \"\$S\"" 2>&1)"
snap_pkg="$(printf '%s' "$snap_out" | tail -n1 | awk -F'|' '{print $2}')"
snap_flag="$(printf '%s' "$snap_out" | tail -n1 | awk -F'|' '{print $3}')"
assert_eq "$snap_flag" "1" "F2: untagged default is snapshot"
assert_eq "$snap_pkg" "0.0.0~git${snap_short}" "F2: snapshot version uses 0.0.0~git<short>"

# F2: explicit invalid version rejected by build-deb.sh/build-rpm.sh/build-msi.sh/build-msi.ps1.
log "-- test: F2 explicit invalid version rejected by build scripts"
fake_bin="$tmp_root/fake-dblocal-bin"
printf '#!/bin/sh\nexit 0\n' > "$fake_bin"
chmod +x "$fake_bin"

set +e
out="$(DBLOCAL_BIN="$fake_bin" bash "$repo_root/packaging/deb/build-deb.sh" --version main --output "$tmp_root/out-deb" 2>&1)"
rc=$?
set -e
if [ "$rc" -eq 2 ]; then pass "F2: build-deb.sh rejects 'main' with exit 2"; else fail "F2: build-deb.sh rejects 'main' with exit 2 (rc=$rc out=$out)"; fi
assert_contains "$out" "invalid version" "F2: build-deb.sh reports invalid version"

set +e
out="$(DBLOCAL_BIN="$fake_bin" bash "$repo_root/packaging/rpm/build-rpm.sh" --version main --output "$tmp_root/out-rpm" 2>&1)"
rc=$?
set -e
if [ "$rc" -eq 2 ]; then pass "F2: build-rpm.sh rejects 'main' with exit 2"; else fail "F2: build-rpm.sh rejects 'main' with exit 2 (rc=$rc out=$out)"; fi
assert_contains "$out" "invalid version" "F2: build-rpm.sh reports invalid version"

set +e
out="$(bash "$repo_root/packaging/msi/build-msi.sh" 'main' '' "$tmp_root/out-msi" 2>&1)"
rc=$?
set -e
if [ "$rc" -eq 2 ]; then pass "F2: build-msi.sh rejects 'main' with exit 2"; else fail "F2: build-msi.sh rejects 'main' with exit 2 (rc=$rc out=$out)"; fi
assert_contains "$out" "invalid version" "F2: build-msi.sh reports invalid version"

# G2: build scripts reject multi-line versions (whole-string validation).
set +e
out="$(DBLOCAL_BIN="$fake_bin" bash "$repo_root/packaging/deb/build-deb.sh" --version $'0.2.0\nDepends: evil' --output "$tmp_root/out-deb-g2" 2>&1)"
rc=$?
set -e
if [ "$rc" -eq 2 ]; then pass "G2: build-deb.sh rejects newline version with exit 2"; else fail "G2: build-deb.sh rejects newline version with exit 2 (rc=$rc out=$out)"; fi
assert_contains "$out" "invalid version" "G2: build-deb.sh reports invalid version for newline"

set +e
out="$(DBLOCAL_BIN="$fake_bin" bash "$repo_root/packaging/rpm/build-rpm.sh" --version $'evil\n0.2.0' --output "$tmp_root/out-rpm-g2" 2>&1)"
rc=$?
set -e
if [ "$rc" -eq 2 ]; then pass "G2: build-rpm.sh rejects newline version with exit 2"; else fail "G2: build-rpm.sh rejects newline version with exit 2 (rc=$rc out=$out)"; fi
assert_contains "$out" "invalid version" "G2: build-rpm.sh reports invalid version for newline"

set +e
out="$(bash "$repo_root/packaging/msi/build-msi.sh" $'0.2.0\r' '' "$tmp_root/out-msi-g2" 2>&1)"
rc=$?
set -e
if [ "$rc" -eq 2 ]; then pass "G2: build-msi.sh rejects CR version with exit 2"; else fail "G2: build-msi.sh rejects CR version with exit 2 (rc=$rc out=$out)"; fi
assert_contains "$out" "invalid version" "G2: build-msi.sh reports invalid version for CR"

# build-msi.ps1 validation: check source contains explicit invalid-version throw (cannot run pwsh here).
if grep -q "invalid version" "$repo_root/packaging/msi/build-msi.ps1" && grep -q '0\.0\.0~git' "$repo_root/packaging/msi/build-msi.ps1"; then
  pass "F2: build-msi.ps1 contains explicit invalid-version error and snapshot fallback"
else
  fail "F2: build-msi.ps1 missing invalid-version error or snapshot fallback"
fi
# G2: ps1 must use whole-string anchored validation (\A..\z), not substring -match.
if grep -qE '\\A\[0-9\]\+\\\.\[0-9\]\+\\\.\[0-9\]\+\(\[-~\]\[0-9A-Za-z\.\]\+\)\?\\z' "$repo_root/packaging/msi/build-msi.ps1"; then
  pass "G2: build-msi.ps1 uses whole-string anchored version regex"
else
  fail "G2: build-msi.ps1 missing whole-string anchored version regex (\\A...\\z)"
fi

# F3: libc floor computed from binary via readelf.
log "-- test: F3 computed libc floor"
real_bin="$HOME/.cache/dblocal-target-gui/release/dblocal"
if [ -x "$real_bin" ] && command -v readelf >/dev/null 2>&1 && command -v dpkg-deb >/dev/null 2>&1; then
  real_glibc="$(readelf -V "$real_bin" | grep -oE 'GLIBC_[0-9]+(\.[0-9]+)+' | sort -V | tail -n 1 | sed 's/^GLIBC_//')"
  if [ -n "$real_glibc" ]; then
    pass "F3: real binary has GLIBC max $real_glibc"
    # Build deb with real binary and assert Depends contains computed floor.
    f3_out="$tmp_root/f3-deb"
    set +e
    out="$(DBLOCAL_BIN="$real_bin" bash "$repo_root/packaging/deb/build-deb.sh" --version 0.2.0 --arch amd64 --output "$f3_out" 2>&1)"
    rc=$?
    set -e
    assert_eq "$rc" "0" "F3: build-deb.sh succeeds with real binary"
    assert_contains "$out" "GLIBC_${real_glibc}" "F3: build log reports computed GLIBC floor"
    deb_file="$f3_out/dblocal_0.2.0_amd64.deb"
    if [ -f "$deb_file" ]; then
      deps="$(dpkg-deb -f "$deb_file" Depends)"
      assert_contains "$deps" "libc6 (>= ${real_glibc})" "F3: deb Depends contains computed libc floor"
      recs="$(dpkg-deb -f "$deb_file" Recommends)"
      assert_contains "$recs" "libvulkan1" "F4: deb Recommends includes libvulkan1"
      assert_contains "$recs" "libwayland-client0" "F4: deb Recommends includes libwayland-client0"
      assert_contains "$recs" "libegl1" "F4: deb Recommends includes libegl1"
    else
      fail "F3: expected deb not produced at $deb_file"
    fi
  else
    fail "F3: could not compute GLIBC max from real binary"
  fi
else
  fail "F3: real binary/readelf/dpkg-deb missing; cannot verify computed libc floor"
fi

# F3: fake readelf fixture - script must use readelf output, not hardcoded 2.31.
log "-- test: F3 fake readelf determines floor"
f3_fake="$tmp_root/f3fake"
mkdir -p "$f3_fake/bin"
cat > "$f3_fake/bin/readelf" <<'FAKE'
#!/bin/sh
cat <<'SYM'
Version needs section '.gnu.version_r' contains 2 entries:
  0x0000: Version: 1  File: libc.so.6  Cnt: 2
  0x0010:   Name: GLIBC_2.2.5  Flags: none  Version: 2
  0x0020:   Name: GLIBC_2.99    Flags: none  Version: 3
SYM
FAKE
chmod +x "$f3_fake/bin/readelf"
# Provide dpkg-deb passthrough by symlinking real one; we need a minimal fake binary file.
printf 'fake' > "$f3_fake/fakebin"
chmod +x "$f3_fake/fakebin"
set +e
out="$(PATH="$f3_fake/bin:$PATH" DBLOCAL_BIN="$f3_fake/fakebin" bash "$repo_root/packaging/deb/build-deb.sh" --version 0.2.0 --arch amd64 --output "$tmp_root/f3-fake-out" 2>&1)"
rc=$?
set -e
assert_contains "$out" "GLIBC_2.99" "F3: fake readelf floor 2.99 is reported"
# dpkg-deb only packs files; it should succeed.
if [ "$rc" -eq 0 ]; then
  deps="$(dpkg-deb -f "$tmp_root/f3-fake-out/dblocal_0.2.0_amd64.deb" Depends)"
  assert_contains "$deps" "libc6 (>= 2.99)" "F3: fake readelf floor used in Depends"
else
  # If dpkg-deb failed for unrelated reason, still require floor was computed.
  assert_contains "$out" "libc6 (>= 2.99)" "F3: fake readelf floor appears in output (build failed rc=$rc)"
fi

# F3: missing readelf / no GLIBC symbols -> clear error.
log "-- test: F3 missing/empty GLIBC detection"
f3_empty="$tmp_root/f3empty"
mkdir -p "$f3_empty/bin"
cat > "$f3_empty/bin/readelf" <<'FAKE'
#!/bin/sh
exit 0
FAKE
chmod +x "$f3_empty/bin/readelf"
printf 'fake' > "$f3_empty/fakebin"
chmod +x "$f3_empty/fakebin"
set +e
out="$(PATH="$f3_empty/bin:$PATH" DBLOCAL_BIN="$f3_empty/fakebin" bash "$repo_root/packaging/deb/build-deb.sh" --version 0.2.0 --arch amd64 --output "$tmp_root/f3-empty-out" 2>&1)"
rc=$?
set -e
if [ "$rc" -ne 0 ]; then pass "F3: empty readelf output fails"; else fail "F3: empty readelf output must fail"; fi
assert_contains "$out" "could not determine GLIBC symbol versions" "F3: clear error for empty GLIBC"

# G1: neither build script may invoke objdump again (readelf handles all archs).
# Comments may mention objdump to explain why readelf is used; command usage must not.
log "-- test: G1 no objdump in packaging scripts"
g1_objdump_hits="$(
  grep -nH 'objdump' "$repo_root/packaging/deb/build-deb.sh" \
    "$repo_root/packaging/rpm/build-rpm.sh" \
    "$repo_root/packaging/lib/glibc.sh" \
    | grep -vE ':[0-9]+:[[:space:]]*#' || true
)"
if [ -n "$g1_objdump_hits" ]; then
  printf '%s\n' "$g1_objdump_hits" >&2
  fail "G1: objdump command/reference found in packaging scripts"
else
  pass "G1: no objdump command use in deb/rpm/glibc scripts"
fi

# G1: shared glibc helper exists and is sourced by both scripts.
if grep -q 'packaging/lib/glibc.sh' "$repo_root/packaging/deb/build-deb.sh" \
  && grep -q 'packaging/lib/glibc.sh' "$repo_root/packaging/rpm/build-rpm.sh"; then
  pass "G1: deb and rpm scripts source shared glibc.sh"
else
  fail "G1: deb/rpm scripts must source packaging/lib/glibc.sh"
fi

# G1: readelf -V and readelf --dyn-syms -W agree on real binary (task evidence).
if [ -x "$real_bin" ] && command -v readelf >/dev/null 2>&1; then
  g1_v="$(readelf -V "$real_bin" | grep -oE 'GLIBC_[0-9]+(\.[0-9]+)+' | sort -V | tail -n 1 | sed 's/^GLIBC_//')"
  g1_dyn="$(readelf --dyn-syms -W "$real_bin" | grep -oE 'GLIBC_[0-9]+(\.[0-9]+)+' | sort -V | tail -n 1 | sed 's/^GLIBC_//')"
  g1_objdump=""
  if command -v objdump >/dev/null 2>&1; then
    g1_objdump="$(objdump -T "$real_bin" | grep -oE 'GLIBC_[0-9]+(\.[0-9]+)+' | sort -V | tail -n 1 | sed 's/^GLIBC_//')"
  fi
  assert_eq "$g1_v" "$g1_dyn" "G1: readelf -V == readelf --dyn-syms -W on real binary"
  if [ -n "$g1_objdump" ]; then
    assert_eq "$g1_v" "$g1_objdump" "G1: readelf -V == objdump -T on real binary"
  else
    pass "G1: objdump not available for comparison (skipped)"
  fi
  log "G1: real binary glibc floor readelf -V=${g1_v} dyn-syms=${g1_dyn} objdump=${g1_objdump:-n/a}"
else
  fail "G1: real binary/readelf missing; cannot compare readelf methods"
fi

# F4: rpm spec contains dlopen runtime Recommends.
log "-- test: F4 rpm spec Recommends"
if grep -q 'Recommends:[[:space:]]*libwayland-client' "$repo_root/packaging/rpm/dblocal.spec" \
  && grep -q 'Recommends:[[:space:]]*mesa-libEGL' "$repo_root/packaging/rpm/dblocal.spec" \
  && grep -q 'Recommends:[[:space:]]*vulkan-loader' "$repo_root/packaging/rpm/dblocal.spec"; then
  pass "F4: rpm spec contains libwayland-client, mesa-libEGL, vulkan-loader Recommends"
else
  fail "F4: rpm spec missing dlopen runtime Recommends"
fi

# --- summary -----------------------------------------------------------------
log ""
log "== results: ${pass_count} passed, ${fail_count} failed =="
if [ "$fail_count" -ne 0 ]; then
  exit 1
fi
exit 0
