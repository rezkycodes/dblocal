# W1-C — Resumable download, verification, safe extraction

Branch: `w1-download`. Read first: AGENTS.md, `docs/architecture/CONTRACT.md`, PRD §6 `F-RTM-01..04`, decisions D-12, D-19, **D-24**, and the
layout section of `docs/spikes/engines/postgres.md` and `valkey.md` (exact `strip_prefix` values and archive entry counts).

## Goal

Take an `Artifact` and produce a verified, extracted runtime directory, or a typed failure that leaves nothing half-installed.

## You own

`crates/core/src/download.rs`, `crates/core/src/download/**` (suggested split: `fetch.rs`, `verify.rs`, `extract.rs`, `space.rs`),
`docs/architecture/download.md`. Nothing else.

## Deliverables

- **Resumable download** with `reqwest` (rustls). Writes `<name>.part` in the downloads directory from `DblPaths`; resumes with `Range` and
  `If-Range`/ETag; if the server ignores `Range` it restarts cleanly and says so. Retries with backoff on network errors (consts). Cancellable
  (`Arc<AtomicBool>` or `tokio::sync::watch`, your choice, documented). Progress callback with bytes, total, speed and ETA from the first byte (F-RTM-01 AC3).
- **URL policy**: HTTPS only, including redirects (max 5, never https to http). A `UrlPolicy::AllowLoopbackHttp` exists for tests only and cannot be
  selected from production code paths. Generic `User-Agent: dblocal/<version>`; nothing identifying (invariants 1, 11).
- **SHA-256 verified before anything is extracted or executed.** Hash while downloading; on resume, hash the existing prefix first. Mismatch deletes the
  `.part`, returns a typed error naming expected and actual digests, and nothing is extracted (invariant 3). There is no skip flag.
- **Disk space**: refuse to start unless free space >= `size_bytes * 3` (F-RTM-02 edge case), with an error that states how much is needed.
- **Safe extraction** for `tar.gz` (`flate2`), `tar.xz` (`liblzma`) and `zip` (`zip` crate, deflate only): reject absolute paths, `..`, Windows drive or UNC
  prefixes, entries that escape through a previously extracted symlink, and hard links; allow only relative symlinks that resolve inside the root
  (PostgreSQL's archive has some); cap total extracted bytes and entry count (consts) against decompression bombs; preserve the Unix executable bit.
  Strip `Artifact::strip_prefix` exactly; if the archive does not have that top-level directory, fail.
- Extract into a temporary sibling directory, verify the expected binaries exist (the caller passes the list), then atomically rename to the final runtime path.
  On any failure remove the temporary directory. Windows antivirus locking: retry the rename with backoff, then a message naming antivirus as a likely cause.
- No network in tests: a local server on port 0 supports `Range`, can truncate the connection mid-body, can ignore `Range`, and can serve a wrong body.

## Tests (RED first)

Zip-slip archives for each format (`../x`, absolute, symlink escape, hard link); bomb cap; truncated download then resume yields the identical hash; ignored `Range`;
wrong SHA-256 deletes `.part`; cancel mid-download; not enough disk (inject the free-space function); strip-prefix mismatch; executable bit survives; the final
directory never exists after a failure.

## Done when

`cargo xtask ci` green, >= 80% lines on files you own, and the report lists each attack you tested and the outcome. Mark Windows-specific code as compiled but not run.
