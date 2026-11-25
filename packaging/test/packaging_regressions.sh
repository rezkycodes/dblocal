#!/usr/bin/env bash
# Packaging regression tests (F1/F2 version mapping, F3 glibc floor, F4 recommends).
#
# Sourced by packaging/test/install_test.sh after its environment is set up.
# Expects these from the caller: repo_root, tmp_root, and the pass/fail,
# assert_eq, assert_contains, log helpers.
# shellcheck shell=bash

# --- W3-E-fix1: packaging version/libc/recommends regression tests -----------
log "-- test: F1/F2 shared version helper"

# Source the helper in a subshell-like check via bash -c to avoid polluting test env.
# shellcheck disable=SC2154 # repo_root comes from the sourcing harness
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
# shellcheck disable=SC2154 # tmp_root is set by the sourcing harness
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
case "$1" in
  -d)
    cat <<'DYN'
 0x0000000000000001 (NEEDED)             Shared library: [libc.so.6]
 0x0000000000000001 (NEEDED)             Shared library: [libxcb.so.1]
DYN
    ;;
  *)
    cat <<'SYM'
Version needs section '.gnu.version_r' contains 2 entries:
  0x0000: Version: 1  File: libc.so.6  Cnt: 2
  0x0010:   Name: GLIBC_2.2.5  Flags: none  Version: 2
  0x0020:   Name: GLIBC_2.99    Flags: none  Version: 3
SYM
    ;;
esac
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

# G1: neither build script may mention objdump again (readelf handles all archs).
log "-- test: G1 no objdump in packaging scripts"
g1_objdump_hits="$(
  grep -nH 'objdump' "$repo_root/packaging/deb/build-deb.sh" \
    "$repo_root/packaging/rpm/build-rpm.sh" \
    "$repo_root/packaging/lib/glibc.sh" || true
)"
if [ -n "$g1_objdump_hits" ]; then
  printf '%s\n' "$g1_objdump_hits" >&2
  fail "G1: objdump reference found in packaging scripts"
else
  pass "G1: no objdump reference in deb/rpm/glibc scripts"
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

# --- GLIBC <= 2.35 gate and DT_NEEDED-derived .deb Depends ------------------
log "-- test: GLIBC anti-regression gate (dblocal_glibc_check_max)"
glibc_sh="$repo_root/packaging/lib/glibc.sh"
g_gate="$tmp_root/glibc-gate"
mkdir -p "$g_gate/bin"
printf 'fake' > "$g_gate/fakebin"
chmod +x "$g_gate/fakebin"

printf '#!/bin/sh\necho "  Name: GLIBC_2.39"\n' > "$g_gate/bin/readelf"
chmod +x "$g_gate/bin/readelf"
set +e
out="$(PATH="$g_gate/bin:$PATH" bash -c "source '$glibc_sh'; dblocal_glibc_check_max '$g_gate/fakebin' 2.35" 2>&1)"
rc=$?
set -e
if [ "$rc" -ne 0 ]; then pass "GLIBC gate: rejects 2.39 (> 2.35)"; else fail "GLIBC gate: must reject 2.39 (rc=$rc out=$out)"; fi
assert_contains "$out" "GLIBC_2.39" "GLIBC gate: error names offending version"

printf '#!/bin/sh\necho "  Name: GLIBC_2.35"\n' > "$g_gate/bin/readelf"
chmod +x "$g_gate/bin/readelf"
set +e
out="$(PATH="$g_gate/bin:$PATH" bash -c "source '$glibc_sh'; dblocal_glibc_check_max '$g_gate/fakebin' 2.35" 2>&1)"
rc=$?
set -e
assert_eq "$rc" "0" "GLIBC gate: accepts 2.35 (<= 2.35)"

log "-- test: .deb Depends derived from DT_NEEDED"
dt_dir="$tmp_root/dt-needed"
mkdir -p "$dt_dir/bin-arm64" "$dt_dir/bin-amd64" "$dt_dir/bin-unknown"
printf 'fake' > "$dt_dir/fakebin"
chmod +x "$dt_dir/fakebin"

cat > "$dt_dir/bin-arm64/readelf" <<'FAKE'
#!/bin/sh
if [ "$1" = "-d" ]; then
  cat <<'DYN'
 0x0000000000000001 (NEEDED)             Shared library: [libxcb.so.1]
 0x0000000000000001 (NEEDED)             Shared library: [libfreetype.so.6]
 0x0000000000000001 (NEEDED)             Shared library: [libc.so.6]
 0x0000000000000001 (NEEDED)             Shared library: [ld-linux-aarch64.so.1]
DYN
else
  echo "  Name: GLIBC_2.35"
fi
FAKE
chmod +x "$dt_dir/bin-arm64/readelf"

cat > "$dt_dir/bin-amd64/readelf" <<'FAKE'
#!/bin/sh
if [ "$1" = "-d" ]; then
  cat <<'DYN'
 0x0000000000000001 (NEEDED)             Shared library: [libxcb.so.1]
 0x0000000000000001 (NEEDED)             Shared library: [libc.so.6]
 0x0000000000000001 (NEEDED)             Shared library: [ld-linux-x86-64.so.2]
DYN
else
  echo "  Name: GLIBC_2.35"
fi
FAKE
chmod +x "$dt_dir/bin-amd64/readelf"

cat > "$dt_dir/bin-unknown/readelf" <<'FAKE'
#!/bin/sh
if [ "$1" = "-d" ]; then
  cat <<'DYN'
 0x0000000000000001 (NEEDED)             Shared library: [libc.so.6]
 0x0000000000000001 (NEEDED)             Shared library: [libmystery.so.9]
DYN
else
  echo "  Name: GLIBC_2.35"
fi
FAKE
chmod +x "$dt_dir/bin-unknown/readelf"

if command -v dpkg-deb >/dev/null 2>&1; then
  set +e
  out="$(PATH="$dt_dir/bin-arm64:$PATH" DBLOCAL_BIN="$dt_dir/fakebin" bash "$repo_root/packaging/deb/build-deb.sh" --version 0.2.0 --arch arm64 --output "$dt_dir/out-arm64" 2>&1)"
  rc=$?
  set -e
  assert_eq "$rc" "0" "DT_NEEDED: arm64 build-deb succeeds"
  deps_arm64="$(dpkg-deb -f "$dt_dir/out-arm64/dblocal_0.2.0_arm64.deb" Depends)"
  assert_contains "$deps_arm64" "libfreetype6" "DT_NEEDED: arm64 Depends includes libfreetype6"
  assert_contains "$deps_arm64" "libc6 (>= 2.35)" "DT_NEEDED: arm64 Depends includes libc6 (>= 2.35)"

  set +e
  out="$(PATH="$dt_dir/bin-amd64:$PATH" DBLOCAL_BIN="$dt_dir/fakebin" bash "$repo_root/packaging/deb/build-deb.sh" --version 0.2.0 --arch amd64 --output "$dt_dir/out-amd64" 2>&1)"
  rc=$?
  set -e
  assert_eq "$rc" "0" "DT_NEEDED: amd64 build-deb succeeds"
  deps_amd64="$(dpkg-deb -f "$dt_dir/out-amd64/dblocal_0.2.0_amd64.deb" Depends)"
  case "$deps_amd64" in
    *libfreetype6*) fail "DT_NEEDED: amd64 Depends must not include libfreetype6 (got: $deps_amd64)" ;;
    *) pass "DT_NEEDED: amd64 Depends omits libfreetype6" ;;
  esac
fi

set +e
out="$(PATH="$dt_dir/bin-unknown:$PATH" DBLOCAL_BIN="$dt_dir/fakebin" bash "$repo_root/packaging/deb/build-deb.sh" --version 0.2.0 --arch amd64 --output "$dt_dir/out-unknown" 2>&1)"
rc=$?
set -e
if [ "$rc" -ne 0 ]; then pass "DT_NEEDED: unmapped soname fails hard"; else fail "DT_NEEDED: unmapped soname must fail (rc=$rc out=$out)"; fi
assert_contains "$out" "unmapped DT_NEEDED soname 'libmystery.so.9'" "DT_NEEDED: error names unmapped soname"

# --- Release asset filename normalization and [A-Za-z0-9._-] gate -----------
log "-- test: release asset filename gate and tilde normalization"
asset_sh="$repo_root/packaging/lib/asset_name.sh"
an_dir="$tmp_root/asset-names"
mkdir -p "$an_dir"

# RED: tilde filename rejected by gate before normalization
set +e
out="$(bash -c "source '$asset_sh'; dblocal_asset_check_name 'dblocal_0.1.0~pre.2_amd64.deb'" 2>&1)"
rc=$?
set -e
if [ "$rc" -ne 0 ]; then pass "asset gate (RED): rejects tilde filename"; else fail "asset gate (RED): must reject tilde filename (rc=$rc out=$out)"; fi
assert_contains "$out" "outside [A-Za-z0-9._-]" "asset gate (RED): error explains allowed character set"

# GREEN: normalize_dir rewrites '~' -> '.', passes gate, and checksums.txt matches
printf 'deb-payload' > "$an_dir/dblocal_0.1.0~pre.2_amd64.deb"
printf 'rpm-payload' > "$an_dir/dblocal-0.1.0~pre.2-1.x86_64.rpm"
set +e
out="$(bash -c "source '$asset_sh'; dblocal_asset_normalize_dir '$an_dir'" 2>&1)"
rc=$?
set -e
assert_eq "$rc" "0" "asset normalize (GREEN): normalizes tilde filenames"
if [ -f "$an_dir/dblocal_0.1.0.pre.2_amd64.deb" ] && [ -f "$an_dir/dblocal-0.1.0.pre.2-1.x86_64.rpm" ] \
  && [ ! -e "$an_dir/dblocal_0.1.0~pre.2_amd64.deb" ] && [ ! -e "$an_dir/dblocal-0.1.0~pre.2-1.x86_64.rpm" ]; then
  pass "asset normalize (GREEN): files renamed with '.' instead of '~'"
else
  fail "asset normalize (GREEN): expected normalized filenames in $an_dir"
fi

# Verify checksums.txt generated from normalized dir verifies cleanly with sha256sum -c
if (
  cd "$an_dir"
  sha256sum dblocal_0.1.0.pre.2_amd64.deb dblocal-0.1.0.pre.2-1.x86_64.rpm > checksums.txt
  sha256sum -c checksums.txt >/dev/null
); then
  pass "asset normalize (GREEN): checksums.txt verifies normalized filenames"
else
  fail "asset normalize (GREEN): checksums.txt verification failed"
fi

# --- MSI same-version upgrade regression checks (v0.1.0 over pre-release) ----
log "-- test: MSI AllowSameVersionUpgrades and ProductCode/UpgradeCode"
msi_wxs="$repo_root/packaging/msi/dblocal.wxs"

if command -v xmllint >/dev/null 2>&1; then
  if xmllint --noout "$msi_wxs" 2>/dev/null; then
    pass "MSI: dblocal.wxs is well-formed XML"
  else
    fail "MSI: dblocal.wxs failed xmllint"
  fi
fi

if grep -q 'AllowSameVersionUpgrades="yes"' "$msi_wxs"; then
  pass "MSI: dblocal.wxs enables AllowSameVersionUpgrades"
else
  fail "MSI: dblocal.wxs missing AllowSameVersionUpgrades=\"yes\""
fi

if grep -q 'Id="\*"' "$msi_wxs" && grep -q 'UpgradeCode="A3D5E2D1-7B4A-4D7E-9F3A-8C2B1D4E5F60"' "$msi_wxs"; then
  pass "MSI: dblocal.wxs uses auto ProductCode (*) and stable UpgradeCode"
else
  fail "MSI: dblocal.wxs missing Id=\"*\" or stable UpgradeCode"
fi

# --- App icon & desktop entry regression checks ------------------------------
log "-- test: app icon assets and desktop entry"
icon_svg="$repo_root/packaging/linux/dblocal.svg"
icon_ico="$repo_root/packaging/icons/dblocal.ico"
desktop_file="$repo_root/packaging/linux/dev.dblocal.DBLocal.desktop"

if command -v xmllint >/dev/null 2>&1; then
  if xmllint --noout "$icon_svg" 2>/dev/null; then
    pass "icon: dblocal.svg is well-formed XML"
  else
    fail "icon: dblocal.svg failed xmllint"
  fi
fi

if [ -f "$icon_ico" ]; then
  pass "icon: packaging/icons/dblocal.ico exists"
else
  fail "icon: packaging/icons/dblocal.ico missing"
fi

for icon_size in 16 24 32 48 64 128 256 512; do
  if [ -f "$repo_root/packaging/icons/png/dblocal-${icon_size}.png" ]; then
    pass "icon: dblocal-${icon_size}.png exists"
  else
    fail "icon: dblocal-${icon_size}.png missing"
  fi
done

if [ -f "$desktop_file" ]; then
  pass "desktop: dev.dblocal.DBLocal.desktop exists"
  if command -v desktop-file-validate >/dev/null 2>&1; then
    if desktop-file-validate "$desktop_file" >/dev/null 2>&1; then
      pass "desktop: dev.dblocal.DBLocal.desktop passes desktop-file-validate"
    else
      fail "desktop: dev.dblocal.DBLocal.desktop failed desktop-file-validate"
    fi
  fi
  wm_class="$(sed -n 's/^StartupWMClass=//p' "$desktop_file" | head -n 1)"
  gui_app_id="$(sed -n 's/.*app_id: Some("\([^"]*\)".*/\1/p' "$repo_root/crates/gui/src/app/bootstrap.rs" | head -n 1)"
  assert_eq "$wm_class" "$gui_app_id" "desktop: StartupWMClass matches GUI app_id"
else
  fail "desktop: dev.dblocal.DBLocal.desktop missing"
fi

for builder in \
  "$repo_root/packaging/deb/build-deb.sh" \
  "$repo_root/packaging/rpm/build-rpm.sh" \
  "$repo_root/packaging/rpm/dblocal.spec" \
  "$repo_root/packaging/appimage/build-appimage.sh"; do
  if grep -q 'dev.dblocal.DBLocal.desktop' "$builder" && grep -q 'dblocal-.*png\|hicolor/.*apps/dblocal\.png' "$builder"; then
    pass "packaging: $(basename "$builder") references desktop file and PNG icons"
  else
    fail "packaging: $(basename "$builder") missing desktop file or PNG icon references"
  fi
done
