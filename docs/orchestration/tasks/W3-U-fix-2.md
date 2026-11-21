# W3-U fix round 2 — three surviving mutations and two small behaviours

Same branch `w3-update` (HEAD `3ccda7f` reviewed). Rules: `docs/orchestration/tasks/_COMMON-RULES.md` (rule 11: every claim pasted). Edit only `crates/core/src/update/**` and `docs/architecture/update.md`.

Round 1 verified by the reviewer on a trial merge with main: `cargo fmt`, `clippy -D warnings`, `cargo deny`, `cargo xtask ci` green; the FULL core lib 20 times: `758 passed; 0 failed` every time (your ETXTBSY explanation is plausible and the retry window is bounded: accepted). The new `download_update` (checksums.txt streamed and capped, digest by asset name, fixed staged name), `redirect_allowed` (and the real GitHub redirect host `release-assets.githubusercontent.com`, checked with a HEAD on a public release, is in `ALLOWED_HOSTS`), plan-driven `apply` with the AppImage target, hard-link backup then one rename, canonicalised symlinks, APPDIR validation, smoke timeout and token compare are all in place. Eleven of fourteen mutations are killed by a named test (the ones in your list: a, b, c, d, e, f, g, h, j, k, l). Do not rework any of that. Three survive:

## S1 — the managed-prefix deny rule is untested

Mutation: in `kind.rs::linux_deny_reason` disable the loop over `["/nix/store", "/snap/", "/app/", "/usr/", "/opt/"]` (`if false && (lower.starts_with(prefix) || lower.contains(prefix))`): `cargo test -p dblocal-core --lib update::` stays all green. `detect_linux_deny_list_paths` asserts only `linux_deny_reason(..).is_some()` for paths that do not exist, which the later "directory not writable" rule also returns, so the prefix rules, and `/usr` and `/opt` in particular (where an installed copy owned by the user is writable), are not pinned at all. Make the reason distinguishable (a typed `DenyReason` enum, or at least assert the message names the matched prefix) and test each rule separately: one case per prefix, one per suffix (`/.cargo/bin/`, `/target/debug/`, `/target/release/`), plus the "unwritable directory" case using a real temp directory with mode 0555. Rerun the mutation: red.

While there, fix the over-broad match: `lower.contains("/opt/")` also denies `/home/u/opt/dblocal/dblocal` and `contains("/app/")` denies `/home/u/work/app/dblocal`, so a portable copy under a user-chosen `~/opt` can never self-update. The managed prefixes must match at the START of the (canonicalised) path (`/usr/`, `/opt/`, `/app/`, `/snap/`, `/nix/store/`); only the three build/tooling suffixes may match anywhere. Tests: `/home/u/opt/dblocal/dblocal` and `/home/u/work/app/dblocal` are NOT denied by the prefix rules (use a real temp tree for the writable and file checks); `/opt/dblocal/dblocal` and `/usr/local/bin/dblocal` are.

## S2 — the writability pre-check in `apply_linux` is untested

Mutation: remove `ensure_target_writable(&target)?;` from `apply_linux`: all green. Round 1 required "before touching anything check that the real directory is writable; if not, a typed error with a clear Indonesian message". Add a test: a temp directory with mode 0555 containing the target and a staged file; `apply` returns `UnsupportedInstallKind` (or the typed error you chose), the target is byte-identical afterwards and no `.old`, no sibling temp and no probe file is left. Also: `ensure_target_writable` and `kind.rs::path_is_writable_by_current_user` are two copies of the same probe, and both CREATE a file in the install directory (`.dblocal-update-probe-<pid>`), so merely DETECTING the install kind writes into the user's `bin` directory. Replace both with one function that asks the OS without writing (`nix::unistd::access(dir, AccessFlags::W_OK)`; `nix` is already a unix dependency of the core, check the feature) and test that detection leaves the directory listing unchanged. Remove the no-op `let _ = meta.permissions().mode();` lines.

## S3 — `restore_backup` atomicity is untested

Mutation: in `restore_backup` put `let _ = std::fs::remove_file(target);` before the final `rename`: all green. Round 1 B4 said restore must rename over the target and never delete it first. Add the continuity test for the restore path (a reader thread polling `target.exists()` and the content while `restore_backup` runs 200 times must never see a missing file), and a test that `smoke_or_rollback` after a failing smoke check leaves `target` equal to the OLD bytes and `.old` removed or kept as documented. Rerun the mutation: red.

## Small

- `apply_linux`: if a step before the backup fails (`WriteNextToCurrent`, `Fsync`) the sibling temp file is left next to the binary. Remove it on every error path; test with an injected failure.
- Paste the per-file coverage table again after these tests (the reviewer is measuring it too: `install.rs` functions were 75.93% in your report).

## Bar

`cargo fmt --all -- --check`, `cargo clippy -p dblocal-core --all-targets -- -D warnings`, full `cargo test -p dblocal-core --lib` x40, `cargo deny check`, `cargo xtask ci`: all pasted. Mutations S1, S2, S3 red then restored (`git status --short` clean, run on a scratch copy or between commits). `worker_done` only with pasted evidence; otherwise `ask` and continue with what is unblocked.
