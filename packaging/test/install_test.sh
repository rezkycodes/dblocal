#!/usr/bin/env bash
# Offline tests for packaging/install.sh (F-PKG-03).
# Serves a fake GitHub Releases layout via `python3 -m http.server` and runs the
# installer against it using DBLOCAL_RELEASE_BASE_URL, without touching the
# network or the real GitHub API.
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
repo_root="$(CDPATH='' cd -- "$script_dir/../.." && pwd)"
installer="$repo_root/packaging/install.sh"

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
    "DBLOCAL_RELEASE_BASE_URL=$BASE_URL"
    "DBLOCAL_TEST_UNAME_S=$us"
    "DBLOCAL_TEST_UNAME_M=$um"
  )
  if [ -n "$home" ]; then cmd+=("HOME=$home"); else cmd+=("HOME="); fi
  # Ensure PATH is predictable and never includes system locations we don't control.
  cmd+=("PATH=/usr/bin:/bin")
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

# --- summary -----------------------------------------------------------------
log ""
log "== results: ${pass_count} passed, ${fail_count} failed =="
if [ "$fail_count" -ne 0 ]; then
  exit 1
fi
exit 0
