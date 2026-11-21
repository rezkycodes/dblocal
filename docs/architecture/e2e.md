# End-to-end on the real engines (W2-C)

This document describes `crates/core/tests/service_e2e.rs` and
`crates/core/tests/support/**`: the suite that proves Phase 1 against the shipped
catalog, the real HTTPS downloader, real PostgreSQL and Valkey binaries, and real
protocol clients — all through the public `DblService` surface only.

Run it:

```sh
export CARGO_TARGET_DIR="$HOME/.cache/dblocal-target"   # the one shared cache
DBLOCAL_E2E=1 cargo xtask e2e
```

One case at a time:

```sh
DBLOCAL_E2E=1 cargo test -p dblocal-core --test service_e2e -- --ignored --nocapture \
  postgres_happy_path_from_catalog_to_archive
```

`cargo xtask e2e` runs each target through `xtask::limit::run_with_limit` with a
45-minute wall-clock limit, so a hung e2e cannot block anything for an hour, then
scans `/proc` for a `postgres` or `valkey-server` process whose command line still
names a `dblocal-e2e-*` temp home. A survivor fails the run.

## What is real, and what is not

Everything the service does is production code:

- the bundled catalog (`catalog/engines.json`) and its artifact selection;
- the real HTTPS downloader, SHA-256 verification and archive extraction;
- real `initdb` / `valkey-server` processes, spawned detached;
- the protocol-level health checks and the `tokio-postgres` / `redis` clients;
- the SQLite store, reconciliation, supervision and the delete modes.

The only substitutions, each of which the PRD documents:

| Substitution | Why | Where it is justified |
|---|---|---|
| `HOME` and `DBLOCAL_HOME` point into a temp dir | the suite must never touch `~/.dblocal` or the developer's keychain | `F-CRD-01` AC2, AGENTS.md invariant 2 |
| `FileOnlySecrets`: the production `0600` file backend, without the keychain attempt | a headless run must not write to the real keychain | `docs/architecture/secrets.md` |
| tests 3-8 start from a runtime materialised from the cached archive | a 40 MB re-download per test is wasteful and the download path is proved once, in tests 1-2 | `F-RTM-03` AC3 ("runtime already installed skips the download") |

`FileOnlySecrets` is the only injected trait object. It routes the real
`FileBackend` for the temp home and reports `File` as the backend, which is
exactly what `SecretStore` does when the keychain is unavailable. No operation,
spawn, probe, terminator or adapter is faked.

The runtime cache lives in `$TMPDIR/dblocal-e2e-runtime-cache` (override with
`DBLOCAL_E2E_RUNTIME_CACHE`), outside every temp home and outside the repository.
Tests 1 and 2 populate it after a genuine download; the rest copy from it.

## The cases

| # | Test | Proves |
|---|---|---|
| 1 | `postgres_download_and_extract_from_the_catalog` | the real catalog + HTTPS download + verify + extract, and the exact stage order |
| 1 | `postgres_happy_path_from_catalog_to_archive` | create (installed runtime), start, `tokio-postgres` SQL, stop, restart, read back, `ArchiveData` delete, purge |
| 2 | `valkey_happy_path_requires_auth_and_persists` | same flow with `redis`: auth required, `SET`/`GET`, AOF persistence, `PurgeData` |
| 3 | `engines_outlive_the_app_and_are_adopted` | `D-02`, `F-INS-03` AC2, `F-TRY-02` AC3: engine survives a dropped service, is adopted as `Running` with the same identity, is stopped through the new service; then `kill -9` → supervisor `Failed` with a diagnosis, and Start works again |
| 4 | `a_port_taken_by_a_foreign_listener_is_a_typed_error` | `F-PRT-02`: create and start on a taken port fail `DBL-1005`, nothing is spawned, and the start succeeds once the port is free |
| 5 | `cancelling_a_real_download_leaves_nothing_behind` | cancel after the first progress sample: `DBL-7001`, no row, no data dir, no credential, no `.part`, no runtime |
| 5 | `a_wrong_checksum_fails_cleanly` | a copied catalog entry with a flipped digest: `DBL-2001`, nothing left behind, no `Extracting` stage |
| 6 | `two_services_start_blocking_identical_rounds_each_get_one_engine` | 10 rounds of two services racing to start one instance: one engine (counted by process group), one typed busy loser |
| 7 | `postgres_security_invariants_hold`, `valkey_security_invariants_hold` | loopback-only listeners, password absent from argv / logs / `state.db` / `-wal` / the home, `0700`/`0600` modes, nothing outside the temp home |
| 8 | `timings_and_sizes_are_reported` | prints create (download vs initialise), start-to-healthy, stop, runtime and empty-data sizes |
| — | `repro_*` | minimal reproductions of the defects this suite found (below) |

Tests 3 and 6 need no network beyond the cached runtime, and were run 25 times
each with no failure.

## Cleanup discipline

Every test goes through `TempHome`, which:

- creates the temp dir and points `HOME`/`DBLOCAL_HOME` at it;
- holds a process-global mutex for its whole lifetime, because env mutation is
  process-wide and `cargo test` is multi-threaded;
- on drop — success, failure **and panic** — scans `/proc` for every process whose
  command line names the temp home and whose resolved executable sits inside that
  home's `runtimes/` tree, `SIGKILL`s each, waits until none remain, and only then
  restores the environment. The two checks are deliberate (AGENTS.md invariant 7):
  the command line proves the process belongs to this temp home, and the
  executable path proves it is an engine this suite installed, not a `sh` whose
  argument happens to mention the path. A process that fails the second check is
  logged and left alone rather than blindly signalled.

The temp directory itself is removed by `TempDir`'s own drop. A separate
`cargo xtask e2e` check fails the run if any engine outlives the binary, so even a
double panic cannot hide a leak.

Ports always come from `support::ports` (20000-29999), which verifies each slot is
free on both loopback families before handing it out. No test binds 5432, 3306,
3307, 6379 or an ephemeral port.

## Why the process-group count for case 6

PostgreSQL forks several worker processes, all in the postmaster's process group.
Counting processes would therefore see "many", which is correct and useless.
Every engine is spawned into its own process group (`process_group(0)` in
`process::spawn`), and two engines never share a group, so
`/proc/<pid>/stat`'s `pgrp` field — read directly, without any engine helper —
gives "how many engines are running".

## Recorded runs (2026-11-21, Fedora, Linux x86_64, glibc 2.43)

Consecutive `DBLOCAL_E2E=1 cargo xtask e2e` runs, each on a clean temp home:

| Run | Result | Duration |
|---|---|---|
| 1 | 14 passed, 1 failed | 110.5 s |
| 2 | 14 passed, 1 failed | 138.8 s |
| 3 | 14 passed, 1 failed | 109.6 s |
| 4 (after the guard was tightened to validate the executable path) | 14 passed, 1 failed | 118.2 s |

The single failure is `postgres_download_and_extract_from_the_catalog`, which is
defect **D-1** below: every shipped PostgreSQL archive is rejected by the
extractor. All other cases passed in every run, and `cargo xtask e2e`'s leak scan
found no engine left behind in any of them.

25-run stability for the two network-free cases:

```text
test3 (engines_outlive_app): ok=25 fail=0
test6 (concurrency):         ok=25 fail=0
```

### Observed stage sequences

```text
postgres download (blocked at extraction)
  ["queued", "downloading", "verifying", "failed"]
  download samples: 49, last 40530262 of 40530262 bytes

postgres lifecycle (runtime already installed)
  ["queued", "initializing", "done"]

valkey (real HTTPS download)
  ["queued", "downloading", "verifying", "extracting", "initializing", "done"]

valkey delete + purge
  InstanceRemoved { id } observed; archive list empty afterwards

cancelled create
  ["queued", "downloading", "failed"] -> DBL-7001

wrong checksum
  ["queued", "downloading", "verifying", "failed"] -> DBL-2001

port in use
  create: ["queued", "failed"] -> DBL-1005
  start:  ["queued", "starting", "failed"] -> DBL-1005, nothing spawned

supervisor after kill -9
  InstanceChanged { status: Failed } after ~1.7 s (STATUS_POLL_SECS = 2)
```

### Timings and sizes (case 8)

```text
PostgreSQL: download=- initialize=0.52s create_total=0.52s
            start_to_healthy=0.14s stop=0.11s
            runtime=109.9MiB empty_data=39.6MiB
Valkey:     download=8.64s initialize=0.01s create_total=8.65s
            start_to_healthy=0.07s stop=0.01s
            runtime=106.8MiB empty_data=0.0MiB
```

PostgreSQL's download figure is absent because its runtime is materialised from
the cached archive: defect D-1 blocks the real path. Valkey's 8.6 s is a genuine
HTTPS download of 46.8 MB, so the `D-02` target (< 120 s for create → URI,
including the download) holds with a wide margin on this link.

Disk: `df -h /` showed 133-136 GB free before and after the whole run on a 462 GB
filesystem; the suite's own footprint is one ~110 MB runtime per temp home plus
the ~110 MB shared cache, all of which the guard deletes.

### Security invariants (case 7)

```text
PostgreSQL: pids=[…] listening_sockets=1, all loopback
            scanned 3709 home files for the password; the only carrier is the
            credentials/cred-… file; the real ~/.dblocal still does not exist
Valkey:     pids=[…] listening_sockets=2, all loopback
            allowed carriers: the credential file and run/valkey.conf (both 0600)
            KNOWN DEFECT: data directory 0755 (see D-3)
```

## Defects found (W2-C), fixed in W2-F

W2-C recorded three real defects; W2-F fixed them and converted the `repro_*`
cases into regression tests.

### D-1 (fixed) — safe in-tree tar hard links

- **Was:** `download::extract::tarball` rejected every tar hard link
  (`CODE_UNSAFE_ARCHIVE`). Every shipped PostgreSQL tarball carries 253 in-tree
  hard links under `share/timezone/`, so PostgreSQL could not be installed.
- **Fix:** accept a hard link only when its own path is safe, its target is a
  relative in-root path that normalises inside the staging root, the target was
  already extracted earlier as a regular file (verified with `symlink_metadata`),
  the destination does not already exist, and the link is created inside the
  staging root. If the OS/filesystem refuses hard links, extraction falls back to
  copying the target bytes. Hard-link entries count against the entry quota;
  copy fallback bytes count against the total-bytes quota. Any violation fails
  with `CODE_UNSAFE_ARCHIVE` and leaves no final runtime.
- **Regression:** `repro_a_safe_in_tree_hard_link_is_accepted`,
  `repro_the_shipped_postgres_archive_extracts`, and the hostile hard-link unit
  tests in `crates/core/src/download/tests/extraction.rs`.

### D-2 (fixed) — `change_port` reconfigures the engine

- **Was:** `change_port` updated the store but not `postgresql.conf`/`valkey.conf`,
  so the next start health-checked the new port while the engine listened on the
  old one.
- **Fix:** `change_port` rewrites the engine config before persisting the new
  port (atomic write in the same directory; failure leaves store and file
  unchanged). `start` re-syncs the engine config from the store before spawning,
  repairing a crash between config rewrite and store update. PostgreSQL rewrites
  the managed block in `postgresql.conf` (keeping other settings/comments);
  Valkey rewrites the `port` line in `valkey.conf`.
- **Regression:** `repro_change_port_reconfigures_the_engine` and unit tests for
  config rewrite/idempotence in engine and instance tests.

### D-3 (fixed) — private data/run/logs/instances dirs

- **Was:** Valkey data/run/logs dirs and some service-created dirs inherited the
  process umask (0755 observed for Valkey data and `instances/`).
- **Fix:** instance data/logs/run dirs are created/tightened to 0700 on Unix;
  Valkey `initialize`/`prepare_run`/`ensure_private_dir` enforce 0700; the
  service tightens `instances/`, `archive/`, `runtimes/`, `downloads/` at
  startup. PostgreSQL data dir remains 0700 (initdb), now also consistent with
  explicit service layout tightening.
- **Regression:** `repro_data_directory_mode_depends_on_the_engine` now asserts
  0700 for data/logs/run/instances for both engines.

## What this suite does not prove

- Windows and macOS. This host is Linux x86_64; the suite is `std`/`/proc` based
  and only builds there. The engine adapters' Windows paths remain
  compile-checked, not run.
- Other glibc versions. The host is glibc 2.43, so the `noble` Valkey build is
  the one selected; the `jammy` fallback (`select_artifact`) is unit-tested but
  not exercised end to end here.
- MySQL and MariaDB: their adapters are stubs until Phase 2, so the suite runs
  PostgreSQL and Valkey only.
- The keychain backends: the file backend is used deliberately, so the
  Secret Service / Keychain / Credential Manager paths are covered by their unit
  tests, not here.
- Live log streaming (`watch_logs`) and the `DotEnv` URI format: both are unit
  tested; the e2e focuses on the lifecycle, not the read models.
