# W2-C — End-to-end on real PostgreSQL and Valkey through `LocalService`

Branch: `w2-e2e`. **Start only after W2-A is merged.** Independent of W2-B (no GUI).
Read first: `AGENTS.md`, `docs/architecture/service.md`, `docs/architecture/engines.md`, `docs/architecture/process.md`, `docs/architecture/secrets.md`, the existing real-binary test `crates/core/tests/engine_e2e.rs` (reuse its helpers and its cleanup discipline).
PRD (`docs/prd/dblocal/PRD.md`): `F-INS-01..05`, `F-RTM-01..03`, `F-PRT-01..02`, `F-CRD-01..03`, `F-LOG-01..02`, decisions D-02, D-16, D-18, D-24 and the §11 Phase 1 definition of done.
Never open an image with the Read tool.

## Goal

Prove Phase 1 on the real thing: the shipped catalog, the real HTTPS downloader, the real engines, the real clients, using only the public `DblService` surface. Nothing in the service may be faked except the home directory, which is a temp dir (`DBLOCAL_HOME` and `HOME` both pointed into it) so the test never touches the developer's `~/.dblocal` or keychain. Credentials use the file backend under the temp home.

## You own

`crates/core/tests/service_e2e.rs` and `crates/core/tests/support/**` (new), a `cargo xtask e2e` subcommand in `crates/xtask` (runs the ignored tests, prints a summary), and `docs/architecture/e2e.md`. You may add dev-dependencies already in the lockfile (`tokio-postgres`, `redis`). You may not change non-test code in `dblocal-core`. A defect you find is reported with a minimal reproduction and handed to me; do not patch the service.

## Tests

All `#[ignore]` by default (they need network and several hundred MB of disk) and run through `cargo xtask e2e`. Every test uses ports from 20000-29999 only (`ports::test_support`) and a guard that, on success, failure and panic, terminates every engine it started (by validated identity) and removes the temp home. After the whole run assert that no `postgres` or `valkey-server` process whose command line contains the temp home is left.

1. **PostgreSQL happy path.** Create the newest PostgreSQL the catalog offers for this host through `create_instance` with the production `UrlPolicy::HttpsOnly`; collect the real event sequence (Queued, Downloading with real progress samples, Verifying, Extracting, Initializing, Done) and assert its order and that `OperationFinished` arrives exactly once. Start, connect with `tokio-postgres` using the URI from `connection_uri` (password from the file backend), `select 1`, create a table and insert a row. Stop, start again, read the row (persistence). Delete with `ArchiveData`: the archive is listed with a non-zero size, the data is inside it, the instance is gone and `InstanceRemoved` was emitted. Purge the archive.
2. **Valkey happy path.** Same flow with the `redis` client: auth required (a connection without the password is refused), `SET`/`GET`, stop/start persistence as far as Valkey persists, delete with `PurgeData`.
3. **Engines outlive the app (D-02, F-TRY-02 AC3).** Start an instance, drop the whole `LocalService` (simulated app exit), assert the engine process is still alive and still answers queries, build a new service over the same home and assert the instance is adopted as `Running` with the same identity, then stop it through the new service. Then `kill -9` the engine and assert the supervisor reports `Failed` with a non-empty diagnosis within a bounded time, and that Start works again afterwards.
4. **Port in use (F-PRT-02).** Hold a listener on a chosen port; `create_instance` and `start_instance` on that port fail with the typed port-in-use error (stable DBL code, names the owner when known); `change_port` to a free port then start succeeds.
5. **Cancel and failure leave nothing behind.** Cancel a real download after the first progress sample: no instance row, no data directory, no credential, no `.part` file, runtime not recorded. Also create with a deliberately wrong catalog sha256 (a copy of the catalog entry, not the shipped one): `ChecksumMismatch`, nothing left behind.
6. **Concurrency on a real engine.** Two services over one home start the same stopped instance at the same instant, 10 rounds: exactly one engine process per round (count by identity), the loser gets the typed busy error.
7. **Security invariants on the real artifacts.** While engines run: every listening socket owned by an engine pid is bound to 127.0.0.1 (parse `/proc/net/tcp` and `/proc/net/tcp6`); the generated password does not appear in `/proc/<pid>/cmdline` of any engine or helper, in any log file, in `state.db`, its `-wal` file, or in the temp home outside the credential file; file modes are 0700 for the secrets and data directories and 0600 for the credential file; nothing was created outside the temp home (list `HOME` before and after). Run this once for PostgreSQL and once for Valkey.
8. **Numbers.** Print, do not assert, the timings of create (split download and initialise), start to healthy, stop, and the sizes on disk (runtime, empty data directory), so the PRD targets can be checked.

## Bar

The whole e2e suite passes 3 times in a row on a clean temp home each time (paste the three summaries); the tests that do not need the network (3 without the download, 6) are also run 25 times. `cargo xtask ci` and `cargo deny check` stay green (the ignored tests must at least compile in CI). Files < 800 lines. `unwrap` is acceptable in test code, but every failure message must say which step failed and with which value.

## Done when

Everything above, the event sequences actually observed, the timing table, any defect found with its reproduction (do not hide it, a defect is a good result), the real `git log --oneline -3` hash after committing, and a plain list of what you could not verify (Windows, macOS, other glibc, MariaDB/MySQL out of scope). Commit before you report.
