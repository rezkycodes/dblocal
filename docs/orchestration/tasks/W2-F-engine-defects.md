# W2-F — Fix the three defects the real-engine e2e found (D-1 hard links, D-2 change_port, D-3 Valkey data dir mode)

Branch: `w2-engine-defects`, from `main` AFTER the W2-C merge (the e2e suite, `cargo xtask e2e` and `docs/architecture/e2e.md` must be in your tree; if they are not, stop and say so).
Read first, in this order: `AGENTS.md`, `docs/architecture/e2e.md` (section "defects": each has a `repro_*` test), `crates/core/tests/support/cases/repro_hardlink.rs`, `repro_change_port.rs`, `repro_data_dir_mode.rs`, `docs/architecture/download.md`, `docs/architecture/service.md`, `docs/architecture/engines.md`, `docs/architecture/secrets.md`. PRD: `F-INS-01`, `F-RTM-01..03`, `F-PRT-01..02`, `F-CRD-01..03`, decisions D-16, D-18, D-24.
Never open an image with the Read tool.

## Why this task exists

W2-C ran the whole product against the real PostgreSQL and Valkey artifacts for the first time and found three real defects. I reproduced all three myself (the four `repro_*` tests pass with `DBLOCAL_E2E=1`; I also downloaded the shipped PostgreSQL 17.12 tarball, sha256 matches the catalog, and counted: 166 dirs, 2428 files, 9 symlinks, **253 hard links, all under `share/timezone/`**, none in `bin/` or `lib/`). Phase 1 cannot be called done while they exist.

## You own

`crates/core/src/download/extract/**` and its tests, `crates/core/src/instance/**` (only what D-2 and D-3 need), `crates/core/src/engine/**` (only what D-2 and D-3 need), `crates/core/tests/support/cases/repro_*.rs` (you convert them, see the bar), `docs/architecture/{download,service,engines,e2e}.md`. You may NOT touch `crates/gui/**`, `crates/cli/**`, `crates/core/src/strings.rs`, the catalog, or any other module. Another worker (W2-B) is editing `crates/gui` and `strings.rs` at the same time. If you need something outside your files, use `ask` with the exact change.

## D-1 (blocking): `download::extract::tarball` rejects every tar hard link

`crates/core/src/download/extract/tarball.rs:75`: `EntryType::Link` is "rejected outright". Every shipped PostgreSQL tarball (theseus-rs) has 253 in-tree hard links (`share/timezone/America/Tijuana -> share/timezone/America/Santa_Isabel` and so on), so `create_instance` for PostgreSQL ALWAYS fails with DBL-2002 at Extracting. The rule was a deliberate defence (hard-link attacks), but a blanket refusal makes the product unusable. Replace it with a safe policy, and write that policy in `docs/architecture/download.md`:

A hard link entry is accepted only if ALL hold, otherwise the whole extraction fails with `CODE_UNSAFE_ARCHIVE` and nothing is left behind (the existing staging-directory discipline stays):
1. Its own path passes the same checks as any entry (no absolute path, no `..`, inside the staging root, no symlinked parent).
2. Its link target is a relative archive path that, after the same normalisation, stays inside the staging root (no absolute target, no `..` escape).
3. The target was ALREADY extracted earlier in this archive as a REGULAR file (keep a set of extracted regular-file paths; a forward reference, a directory, a symlink, a device or a path that does not exist is refused). Check with `symlink_metadata` that the target on disk is a regular file and not a symlink.
4. The link is created with `std::fs::hard_link` between two paths inside the staging root only; if the OS refuses hard links (some Windows volumes), fall back to a copy of the target's bytes. It never overwrites an existing entry.
5. It counts against the existing entry-count and total-size quotas (a hard link adds no bytes, but a bomb of millions of links must still hit the entry quota).
Tests (hostile first, then the happy path): absolute target; `..` target; target outside the root; target that is a symlink extracted earlier pointing outside; target that is a directory; forward reference (link before its target); link whose own path goes through a symlinked directory; a link chain (hard link to a hard link: allowed, it is a regular file); duplicate link name; quota exceeded by links. Then a synthetic tar built to mirror the real shape (a `share/timezone` tree with 253 links, bin/lib with symlinks) extracts correctly and the links share an inode on Unix. Finally the real artifact: the e2e PostgreSQL case must create an instance from the downloaded catalog tarball. Mutation-check the new rules: remove the containment check, remove the regular-file check, allow absolute targets: each must fail a named test (paste the red output in a comment next to the test).

## D-2: `change_port` persists the port but never reconfigures the engine

`instance/read_ops.rs::change_port` (called from `instance/service.rs:666`) updates the store only. `postgresql.conf` still says the old `port`, `valkey.conf` the old `port`; the next start health-checks the new port, the engine is not on it, and start fails with DBL-4001. That breaks F-PRT-02 AC2 and the one-click "Pindah ke port berikutnya & start" button the GUI offers on a port conflict. Fix it so config can never disagree with the store:
- `start` regenerates the engine's port setting from the store before it spawns (idempotent; this also repairs a crash between the two writes). The writer is atomic (temp file in the same directory, fsync, rename) and changes only the port line, keeping every other setting and comment. PostgreSQL: `postgresql.conf` (see `engine/postgres.rs` around line 480, which already edits it); Valkey: `valkey.conf` (see `engine/valkey.rs`, `render_config`).
- `change_port` also rewrites the config when the instance is stopped and fails cleanly (store and file both unchanged, typed error) if the rewrite fails.
- Check what else embeds the port (unix socket file names, pid files, health-check target, the recorded connection URI) and say in the report what you found and handled.
Tests: unit tests with the fake adapter (write, rewrite keeps other lines, failure leaves both unchanged, start repairs a stale file); then the real e2e: change the port of a real PostgreSQL and a real Valkey instance, start, connect on the NEW port with the real client, and assert the old port is refused.

## D-3: the Valkey data directory is created 0755, not 0700

`engine/valkey.rs` lines ~195 to 204 use `create_dir_all` and inherit the umask. PostgreSQL is 0700 only because `initdb` chooses it. The security invariant (PRD, `docs/architecture/secrets.md`) is that data and secrets directories are 0700 on Unix. Create the Valkey data, run and logs directories with mode 0700 and set it explicitly after creation (do not rely on the umask), and audit every other directory the service creates for the same problem (`instance/create.rs:134`, runtimes, archive, secrets) and say what you found. I already saw one: the e2e prints `PostgreSQL: root=700 instances=755 data=700 secrets=700`, so the `instances` directory is 0755 (it lists every instance name to other local users); tighten it too unless you find a reason it must stay open, and say which. Test without depending on the process umask (do not change the global umask in tests): create under a deliberately permissive parent and assert the resulting mode, and assert it for a pre-existing 0755 directory that gets tightened. The W2-C security-invariants case must pass for both engines.

## Tests and bar

- Convert the four `repro_*` tests: they currently assert the DEFECT and pass today. After your fix they must assert the corrected behaviour (they are regression tests now); say in the report which you converted, and delete any that became redundant with a better test. Keep them `#[ignore]` only if they need the network.
- `DBLOCAL_E2E=1 cargo xtask e2e`: all 15 tests pass, THREE consecutive full runs on a clean temp home each (paste the three summaries). The PostgreSQL create must now go through the real downloader and extractor.
- `cargo xtask ci` and `cargo deny check` green. The instance and download test suites pass 40 times in a row (paste the counts). Every file you touched >= 80% lines (`cargo llvm-cov`, paste the numbers). Files < 800 lines, no `unwrap`/`expect` outside tests, functions < 50 lines.
- Security: list every hostile hard-link case you tested and its result. This is attack surface; if you are unsure whether a case is safe, refuse it and say so.

## Rules for this machine and this run

- RAM and disk are tight. ONE build cache: `CARGO_TARGET_DIR=$HOME/.cache/dblocal-target`; no other target directories; one cargo build at a time; the e2e temp homes are deleted by the guard; report `df -h /` before and after.
- `cargo xtask ci` and `cargo xtask e2e` run each step under a time limit and kill the whole process tree on timeout. If one reports "timed out", a test hung: find it, do not raise the limit.
- Do not edit other worktrees or type into other terminals. Every claim in the report is backed by pasted output. Commit before you report; give the real `git log --oneline -3`, `git status --short` and the largest `wc -l`.
- If you find a FOURTH defect, do not fix it silently: reproduce minimally, add it to the report, and carry on.

## Done when

The three defects are fixed and proven (unit, hostile, real e2e), the repro tests are converted, the docs say what the policy now is and why, and the report lists what you could not verify (Windows hard-link behaviour and the copy fallback, macOS, other glibc, MariaDB/MySQL archives, which have their own layouts).
