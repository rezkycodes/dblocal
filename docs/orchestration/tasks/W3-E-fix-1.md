# W3-E fix round 1 — review findings on the packaging branch

Same branch `w3-packaging` (worktree `/home/rezkycodes/orca/workspaces/db-local/w3-packaging`). Read `docs/orchestration/tasks/_COMMON-RULES.md` and `docs/orchestration/tasks/W3-E-packaging.md` again; the same rules and bar apply. Commit after every item.

The reviewer rebuilt the `.deb` and `.rpm` from the real release binary and found real defects. Shellcheck, actionlint, xmllint, the file layout (only `/usr`), the maintainer scripts and the "keeps `~/.dblocal`" message are fine: do not rework them.

## F1 — prerelease versions sort AFTER the final release (deb and rpm)

Evidence: `build-deb.sh --version v0.2.0-rc.1` produced `dblocal_0.2.0-rc.1_amd64.deb`; `dpkg --compare-versions 0.2.0-rc.1 lt 0.2.0` is FALSE, so upgrading from the rc to the final looks like a downgrade and apt will not upgrade. The rpm got `Version 0.2.0.rc.1`, which has the same problem.

Fix: map a SemVer prerelease to the tilde form in the PACKAGE version only (`0.2.0-rc.1` → `0.2.0~rc.1`), for both deb and rpm (rpm accepts `~` in Version; if you prefer the `Release: 0.1.rc.1` convention, say why). Keep the release file name and the tag unchanged. Put the mapping in ONE small shared helper that both scripts source (for example `packaging/lib/version.sh`), not copied twice. Tests (in the existing test script): `0.2.0`, `v0.2.0`, `0.2.0-rc.1`, `0.2.0-rc1`, `0.2.0-beta.2`, and assert with `dpkg --compare-versions` that `rc.1 < rc.2 < final` and, for rpm, with `rpm --eval '%{lua: print(rpm.vercmp("a","b"))}'` (or `rpmdev-vercmp` if present) that the same ordering holds. Paste the output.

## F2 — invalid version input crashes with a dpkg parse error

Evidence: `build-deb.sh --version main` fails inside dpkg-deb with "parsing file .../control near line 2". The default (`git describe --tags --always`) can return a bare commit hash on an untagged checkout, which fails the same way.

Fix: validate before building: after stripping a leading `v`, require `^[0-9]+\.[0-9]+\.[0-9]+([-~][0-9A-Za-z.]+)?$`; otherwise exit 2 with a one-line clear message naming the bad value. For the DEFAULT with no tag, do not fail: use `0.0.0~git<shorthash>` and print that it is a snapshot build. Same validation in the rpm script, `build-msi.sh` and `build-msi.ps1` (the ps1 already falls back to `0.0.0` silently: make it an error for explicit input and the documented snapshot for the default). Tests for each.

## F3 — hardcoded `libc6 (>= 2.31)` is wrong

Evidence: the script justifies `libc6 (>= 2.31)` in a comment, but `objdump -T` on the real binary shows it needs symbols up to `GLIBC_2.43` (the local binary was built on Fedora 44); the CI binary is built on `ubuntu-24.04` (glibc 2.39). A `.deb` that claims 2.31 installs on a system where the program then fails at load time.

Fix: compute the libc floor from the binary at build time (`objdump -T | grep -o 'GLIBC_[0-9.]*'`, take the maximum with `sort -V`, emit `libc6 (>= X)`), fail with a clear error if the tool is missing or finds nothing, and print the computed value in the build log. Do the same check for the rpm and print what rpmbuild auto-generated (the rpm already carries `libc.so.6(GLIBC_2.x)` requirements: confirm and keep). Test with two fake ELF fixtures or with the real binary plus an assertion that the emitted floor equals the real maximum. Then write the CONSEQUENCE in `docs/install/` and `docs/release/PACKAGING.md`: which distributions can install the CI-built package (glibc 2.39 means Ubuntu 24.04+, Debian 13+, Fedora 40+ and similar; Ubuntu 22.04 and Debian 12 cannot), and that this applies to the AppImage and tarball too. If you can check the real glibc floor of each distribution from the repository index you know, cite the version numbers; if not, say you did not check.

## F4 — libraries the binary loads at runtime are not declared

Evidence: `strings` on the binary shows `libEGL.so.1`, `libvulkan.so.1`, `libwayland-client.so.0` (loaded with dlopen, so not in DT_NEEDED). Only `libvulkan1` is in Recommends.

Fix: add `libwayland-client0`, `libegl1` (deb) and `libwayland-client`, `mesa-libEGL` (rpm) as `Recommends` (not hard `Depends`, since GPUI can fall back to X11/xcb), with a comment saying why. Confirm the package names exist by looking at what is installed on this Fedora machine for the rpm names (`rpm -q --whatprovides libEGL.so.1`) and say you could not check the Debian names on this machine if that is the case.

## F5 — documentation

`docs/dev/windows-verification.md`: add, at the top of the matrix, one row "Built version", where the owner records `dblocal --version` and the commit; add a row for "Close while an engine is running" (quit dialog lists running engines; choose Stop all; no orphan processes). Add one sentence that rows depending on the tray and quit flow assume the GUI-wiring task (W2-B) has been merged into the commit under test, and the owner must record the commit. Nothing else in that file changes.

## Bar

Rerun your test script (it must stay green, with the new cases), `shellcheck` on every script, `actionlint`, `xmllint`. Rebuild the `.deb` and `.rpm` from `$HOME/.cache/dblocal-target-gui/release/dblocal` and paste `dpkg-deb -I`, `rpm -qpR` heads. Mutation checks (paste red output): remove the tilde mapping, remove the version validation, put the hardcoded 2.31 back; each must fail a named test. No Rust edits. Do not install anything on the host. Final report: what changed per finding, test output, and what stays unverified.
