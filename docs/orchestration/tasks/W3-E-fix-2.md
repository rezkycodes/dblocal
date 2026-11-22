# W3-E fix round 2 — two small defects and one unverified path

Same branch `w3-packaging`. Rules in `docs/orchestration/tasks/_COMMON-RULES.md` apply (every claim pasted). The reviewer verified round 1 independently: `shellcheck`, `actionlint`, `xmllint` clean; `install_test.sh` 92/92; four mutations (no tilde mapping, no validation, hardcoded libc6 2.31, floor taken from the wrong symbol) each fail named tests; rebuilt `.deb`/`.rpm` show `0.2.0~rc.1` sorting before `0.2.0` under `dpkg --compare-versions` and `rpm.vercmp`, `Depends: libc6 (>= 2.43)`, Recommends `libvulkan1, libwayland-client0, libegl1`; the docs rows are present. Do not rework any of that.

## G1 (HIGH, CI-only risk) — the glibc floor uses `objdump`, which cannot read the arm64 binary

`release.yml` job `package-linux` runs on `ubuntu-24.04` (x86_64) for ALL four matrix entries, including the `arm64` and `aarch64` packages, whose input binary is an AArch64 ELF built on `ubuntu-24.04-arm`. A stock x86 `objdump` (GNU binutils without `--enable-targets=all`; Debian and Ubuntu ship that as the separate `binutils-multiarch`) does not recognise an AArch64 ELF (on the reviewer's Fedora `objdump -i | grep -ci aarch64` is 0). Both build scripts fail on purpose when the floor cannot be computed, so the arm64 package jobs would fail, `release` has `needs: package-linux`, and NO release would be published. Local tests cannot see this because the only binary here is x86_64.

Fix: compute the floor with `readelf` (it parses ELF itself and handles every architecture). On the reviewer's binary all three give the same answer: `objdump -T` -> GLIBC_2.43, `readelf -V` -> GLIBC_2.43, `readelf --dyn-syms -W` -> GLIBC_2.43. Use `readelf -V` (the "Version needs" section) or `--dyn-syms`, in BOTH `build-deb.sh` and `build-rpm.sh`, with one shared function in `packaging/lib/` (the two scripts duplicate the extraction today). Update the fake-objdump fixture tests to a fake `readelf`, keep the "real binary equals real maximum" test, and add a test that fails if either script mentions `objdump` again. Also add a CI log step in `package-linux` that prints `readelf -h` (Machine and Class) of the input binary so a wrong-architecture artifact is visible in the log. If you can find any non-x86 ELF on this machine (for example a cross-compiled test object), run `readelf -V` on it and paste the output; if not, say that the AArch64 path is unverified here.

Mutation checks (red output pasted, restored, green): switch the shared function back to `objdump`; take the minimum instead of the maximum.

## G2 (LOW) — the version validator accepts multi-line values

`dblocal_version_is_valid` pipes the value into `grep -Eq`, and grep matches line by line, so a value with a valid line anywhere passes:

```
dblocal_version_is_valid $'evil\n0.2.0'            -> ACCEPTED
dblocal_version_is_valid $'0.2.0\nDepends: evil'   -> ACCEPTED
```

Git refs cannot contain newlines, so CI is not exposed, but an explicit `--version` could be substituted into the deb `control` template. Use a bash anchored regex (`[[ $v =~ ^[0-9]+\.[0-9]+\.[0-9]+([-~][0-9A-Za-z.]+)?$ ]]`, which treats the whole string as one) and add the two inputs above plus one containing a carriage return and one containing a space as tests, for deb, rpm and msi. Mutation: put `grep -Eq` back, a named test fails.

## G3 (unverified path) — arm64 and aarch64 builds were never exercised

Run `build-deb.sh --arch arm64` and `build-rpm.sh --arch aarch64` once with the local x86 binary (the architecture label and `rpmbuild --target` path only; the package will be mislabelled, which is fine for a smoke run in a temp directory) and paste what happens, including any error. If `rpmbuild` refuses cross-arch packaging without extra flags, fix the script so the CI matrix entry would work, or say precisely what CI will need. Do not install anything on the host.

## Bar

`shellcheck` on every script, `actionlint`, `xmllint`, `install_test.sh` all pass (counts pasted), the 40 consecutive runs of `install_test.sh` (count pasted), no Rust edits, nothing installed on the host. `worker_done` only with pasted evidence; otherwise `ask`.
