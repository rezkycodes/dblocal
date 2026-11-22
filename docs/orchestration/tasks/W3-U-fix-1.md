# W3-U fix round 1 — review findings on the self-update module

Same branch `w3-update`, worktree `/home/rezkycodes/orca/workspaces/db-local/w3-update`. Read `docs/orchestration/tasks/_COMMON-RULES.md` (rules 10 and 11 apply: every claim must be pasted output) and `docs/orchestration/tasks/W3-U-update.md` again. Commit after every finding.

Your report was honest about coverage and the ETXTBSY flake, and its hash matched. The reviewer then ran the gates on a trial merge and wrote probe tests against your public API. Evidence is quoted below; probe output was produced on the reviewer's copy of `7e804d0`.

## Gate failures at the reported commit

- `cargo fmt --all -- --check` FAILS at `7e804d0` (diffs in `update/check/tests.rs`, `update/kind.rs:155`, `update/kind/tests.rs`), the report says PASS. Run it again at the end and paste.
- One `cargo test -p dblocal-core --lib` run (the whole library, not the `update` filter) ended `707 passed; 1 failed`; the failing name was not captured and it did not reproduce in 14 later runs (8 normal, 6 under six busy CPU loops). Run the FULL core lib 40 times and paste the pass count; if anything fails, fix the cause.

## Blocking findings

**B1. The download path never uses `checksums.txt` in production code.** `Checksums::parse` and `digest_for` are only called from tests; `MAX_CHECKSUMS_BODY_BYTES` is defined and exported but unused; `download_and_verify` takes `expected_hex` from the CALLER, so the one step the spec calls the security core (digest from the release's `checksums.txt` for THIS asset name, compared before install) is left for the future wiring task to reimplement. Also `ReqwestUpdateFetcher` only follows redirects to `api.github.com`, so it can never fetch a `github.com/.../releases/download/...` asset (GitHub redirects those to a CDN host). Build one function, for example `download_update(update: &AvailableUpdate, staging_dir: &Path, options) -> Result<StagedUpdate>`: fetch `update.checksums_asset()` (streaming, capped at `MAX_CHECKSUMS_BODY_BYTES`, same host policy as the asset), parse, `digest_for(update.asset().name())` or a typed missing-entry error, download the asset to a FIXED name inside `staging_dir` (never the asset name from the JSON), verify, return the staged path and version. Make the host policy ONE function (`validate_url` and the redirect closure currently duplicate it). Tests with the local server: success; checksums.txt without the asset; checksums.txt too large; asset oversized; truncated; wrong digest.

**B2. The redirect allowlist is not tested.** Mutation run by the reviewer: in `download.rs::build_client` change `if ok {` to `if true || ok {` (the policy always follows) and run `cargo test -p dblocal-core --lib update::` → `test result: ok. 101 passed; 0 failed`. The existing `download_rejects_redirect_to_disallowed_host` redirects to `http://127.0.0.1:9/evil`, which the test options ALLOW (loopback), so it passes only because nothing listens on port 9. Extract a pure `redirect_allowed(url, allow_loopback) -> bool`, unit-test it (other https host, `https://github.com.evil.example`, `https://github.com@evil.example/`, plain http to a public host, allowed CDN hosts, port variants, hop count), and add a server test where the redirect target is a SECOND local server that records whether it was contacted (policy must stop before connecting). The always-follow mutation must now fail a named test. Also make `UpdateDownloadOptions::allow_loopback_http` impossible to set from production code (`pub(crate)` field and a `#[cfg(test)]` constructor); today any caller can switch on plaintext http.

**B3. `apply` does not execute the plan, and uses the wrong target for an AppImage.** Probe: `plan_install(AppImage { appimage_path: A }, staged, current_exe = E)` produces steps whose `RenameOver.dst` is `A` (correct), but `apply` calls `apply_linux`, which ignores the steps and replaces `plan.current_exe()` instead:

```
P1 steps dst = ["/tmp/.tmpg9YUMh/DBLocal.AppImage"]
P1 appimage (the planned target) now: "#!/bin/sh\necho dblocal 0.1.0"      <- untouched
P1 current_exe now:                    "#!/bin/sh\necho dblocal 9.9.9"      <- replaced
```

For a real AppImage `current_exe` is inside the read-only mount, so the update would fail or hit the wrong file. Make the plan carry its target and make `apply` execute the plan's own steps (one source of truth), so a golden plan test equals what runs. Test with the AppImage case above.

**B4. The swap is not atomic, and the backup step loses the binary on failure paths.** `swap_with_backup` renames `target` to `.old` and then renames the new file to `target`: between the two renames `target` does not exist (a crash or power loss there leaves no application at its path). Create the backup as a hard link (copy as fallback), then ONE `rename(temp, target)` over the live file. `restore_backup` must rename over `target`, not delete it first. Test: a reader thread polling `target.exists()` and the version output while `apply` runs 200 times must never see a missing file; and an injected failure after the backup step leaves `target` intact.

**B5. A symlinked install is destroyed.** Probe: `exe_path` is a symlink to the real file; after `apply` the link is a regular file and the real file keeps the OLD content:

```
P2 link is still a symlink = false
P2 real file content = "#!/bin/sh\necho dblocal 0.1.0"
```

Canonicalize the target when the plan is built, replace the real file, keep the link. Before touching anything check that the real directory is writable; if not, a typed error with a clear Indonesian message.

**B6. Install kind is guessed on Linux; the spec says `Unknown` is refused.** Probe: with no dpkg/rpm owner, all of these are `PortableTarball` with `can_self_update = true`: `/home/u/.cargo/bin/dblocal`, `/nix/store/abc-dblocal/bin/dblocal`, `/home/u/src/dblocal/target/debug/dblocal`, `/snap/dblocal/current/bin/dblocal`. An update would overwrite a cargo-installed binary, a read-only store path, or the developer's own build. The release tarball contains only the bare `dblocal` binary (see `release.yml`: `tar -C stage -czf ... dblocal`), so there is no marker file to look for: use a deny-list (`/nix/store`, `/snap`, `/app` (flatpak), `/usr`, `/opt`, `*/.cargo/bin`, `*/target/debug`, `*/target/release`) plus "the directory is writable by the current user", everything else that is not recognised becomes `Unknown` with a message. Document the rule and its limits in `update.md`. Tests for all four paths above.

**B7. A foreign `$APPIMAGE` is trusted.** Probe: `exe = /usr/local/bin/dblocal` with `APPIMAGE=/home/u/Apps/SomeOtherApp.AppImage` set in the environment (AppImage apps export this variable to their child processes, for example a terminal that is itself an AppImage) gives `InstallKind::AppImage { appimage_path: <the OTHER app> }`, and an update would overwrite that other application. Require `APPDIR` to be set, `exe_path` to be inside `APPDIR`, and `APPIMAGE` to name an existing regular file. Test the probe case.

**B8. The MSI install directory is not recognised.** `packaging/msi/dblocal.wxs` (branch `w3-packaging`) installs per user under `LocalAppDataFolder\DBLocal`, but `detect_windows` only treats `\program files` as MSI, so a per-user MSI install is classified `WindowsZip` and would be replaced behind Windows Installer's back. Treat both `\program files` and `\appdata\local\dblocal` as MSI (refuse; a zip unpacked there is misclassified in the SAFE direction, say so in the doc). Unit test by value; mark unverified until the owner's Windows run.

**B9. The Stable channel offers semver prereleases.** Probe: a release `v0.3.0-rc.1` with GitHub `prerelease: false` (our workflow does not set the flag) is offered to the Stable channel: `P6 ... available = true`. In Stable skip any version with a non-empty `pre` part in addition to the GitHub flag. Test.

## Medium findings

**M1. `smoke_check` can hang forever and compares by substring.** `Command::output()` has no timeout: a new binary that hangs on `--version` leaves the old one already moved aside and the updater stuck. Add a 10-second timeout that kills the child and triggers the rollback (test with a fake binary that sleeps 30 s, and the whole test must finish in under 15 s). The version check is `text.contains(expected)`: probe `smoke_check(expect 1.2.3, binary says 11.2.30) = ok`. Compare against whitespace-separated tokens (accept a leading `v`).

**M2. `ReqwestUpdateFetcher`** buffers the whole body (`response.bytes()`) before `MAX_RELEASES_BODY_BYTES` is applied: stream it and stop at the cap. Its `Default` impl falls back to `reqwest::Client::new()` (no https-only, no redirect policy) when the builder fails: remove `Default` or make it fail closed.

**M3. `Checksums::parse`** accepts uppercase hex but the comparison is against lowercase: lowercase on parse. Duplicate entries for the same name must be an error (today the first wins silently). Test both.

**M4. Backup cleanup.** `KeepBackup` is only a label in the plan. Provide `cleanup_backup(current_exe)` for "the next successful start" (called later by the wiring task) and document it.

**M5. Coverage.** The report lists `check.rs` 76.16%, `download.rs` 75.00%, `kind.rs` 78.49% against the 80% bar. Paste the per-file table after this round: all of `update/**` at 80% or more, pure logic at 90% or more. Use `cargo llvm-cov -p dblocal-core --lib --summary-only` with its own cache (`CARGO_LLVM_COV_TARGET_DIR=$HOME/.cache/dblocal-target-cov-w3u`, delete it when done).

## Bar

`cargo fmt --all -- --check`, `cargo clippy -p dblocal-core --all-targets -- -D warnings`, full `cargo test -p dblocal-core --lib` x40, `cargo deny check`, `cargo xtask ci`: all pasted. Mutation checks, red output pasted for each, then restored and green, never leaving a mutated file in the tree (run them on a scratch copy or between commits and show `git status --short` clean): (a) redirect policy always follows; (b) host allowlist always true; (c) checksum comparison always true; (d) downgrade guard off; (e) Stable accepts semver prereleases; (f) APPDIR check removed; (g) `apply` uses `current_exe` instead of the plan's target; (h) backup created by rename instead of hard link (the continuity test must fail); (i) the kind deny-list removed.

Report with `worker_done` only when every item has pasted evidence; otherwise `ask` with the blocker. Update `docs/architecture/update.md` for the decisions above and the unverified list (Windows swap helper and MSI detection at runtime, macOS bundle replace, real GitHub rate limits and CDN redirects, provenance/attestation is not verified).
