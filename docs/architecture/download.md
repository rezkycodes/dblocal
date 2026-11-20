# `download` — resumable fetch, SHA-256 verification, safe extraction

Owner: W1-C. Module root `crates/core/src/download.rs`, implementation under
`crates/core/src/download/`.

This module is the only place in the codebase that downloads bytes from the network or unpacks an
archive. Everything else asks it for a verified runtime directory.

## Public surface

| Item | Purpose |
|---|---|
| `install_runtime(&Artifact, &DblPaths, &[&str], InstallOptions) -> PathBuf` | Production entry point. Derives the final directory from `DblPaths::runtime_dir`. |
| `install_runtime_to(&Artifact, downloads_dir, final_dir, &[&str], InstallOptions)` | Same pipeline into caller-chosen paths. |
| `install_source_to(&SourceSpec, downloads_dir, final_dir, &[&str], InstallOptions)` | Pipeline driven by a plain spec instead of an `Artifact`; lets the tests drive it from a loopback HTTP server. |
| `download_artifact(&Artifact, part_path, &DownloadOptions, Option<ProgressSink>)` | Download + verify only. |
| `download_from_url(...)` | Download + verify given a URL, id, digest, size. Used by `install_source_to` and tests. |
| `extract_runtime(archive, format, artifact_size, final_dir, &ExtractOptions)` | Extraction + validation + atomic finalisation. |
| `sha256_file`, `sha256_bytes` | Hashing helpers. |
| `check_disk_space`, `DiskSpace`, `FreeSpaceFn` | Free-space preflight with an injectable probe. |
| `CancellationToken` | `Arc<AtomicBool>` cooperative cancel flag. |
| `DownloadEvent { Progress, Restarted }`, `ProgressSink` | UI hook: bytes/total/speed/ETA from the first byte, and a notice when a restart happens. |
| `UrlPolicy`, `DownloadOptions`, `InstallOptions`, `SourceSpec`, `ExtractOptions` | Configuration. |

## Pipeline order (why it is safe)

`run_install` is the single path all three install entry points take:

1. Create the downloads directory (inside `~/.dblocal/downloads`).
2. **Free-space preflight** — require `size_bytes * 3` (`.part` + extracted tree + final runtime
   can coexist). Fails with an error that names the exact bytes needed and available.
3. **Download** to `<artifact-id>.part`, hashing as bytes arrive.
4. **Verify SHA-256.** A mismatch deletes the `.part` and its `.etag` sidecar and returns
   `DblError::ChecksumMismatch` naming both digests. Nothing is extracted. There is no skip flag.
5. **Extract** into a temporary sibling directory `.<name>.extracting-<nonce>` next to the final
   path, then verify the caller's expected binaries exist, then `rename` into place.
6. On success, remove the `.part` and its sidecar. On failure the staging directory is removed and
   the final directory is never created.

Because SHA-256 verification happens while streaming, the archive on disk is never read again
before extraction — and because extraction only starts after the hash matches, no unverified byte
is ever written to disk.

## Resumable transfer

- The partial file is `<downloads>/<sanitised-artifact-id>.part`. The sanitised id contains only
  `[A-Za-z0-9._-]`, so a hostile catalog id cannot escape the directory.
- On start, an existing non-empty `.part` is hashed first (`hash_prefix`), and the request carries
  `Range: bytes=<len>-` plus `If-Range: <stored ETag>` when an `.etag` sidecar exists. Flow resumes
  from the same hasher, so the final digest is over the whole file.
- A `200 OK` in response to a `Range` (server ignored it), a `416`, a `412` (validator mismatch), or
  a `206` whose `Content-Range` start does not match our offset all reset the `.part` and the
  hasher, and restart from byte 0. Each of those cases emits `DownloadEvent::Restarted { reason }`
  so the UI can say "the server ignored Range; restarting" instead of silently re-downloading.
- Each attempt reports whether it is worth retrying, so a permanent answer is never hammered:
  - **Retryable**: connect/DNS/timeout errors, connection reset, unexpected EOF, write I/O
    failures, HTTP `5xx`, `408 Request Timeout` and `429 Too Many Requests`.
  - **Fatal**: any other `4xx` (a `404` is a fact, not a blip), a checksum mismatch, URL-policy
    rejection, cancellation, a body longer than the catalog allows, and a content encoding other
    than `identity`.

  Retryable attempts are retried up to `MAX_RETRIES` with exponential backoff between
  `INITIAL_BACKOFF` and `MAX_BACKOFF` (total attempts = 1 + `MAX_RETRIES`).

### Body size is bounded by the catalog (C1)

The declared `size_bytes` bounds what is written, not just what is checked at the end:

- **Before writing**, a response whose declared length contradicts the catalog fails fatally. On a
  `206` the full size comes from the `Content-Range` total; on a `200` it comes from
  `Content-Length`. The error names the server's declared size and the catalog's expected size, and
  nothing is written.
- **While streaming**, the first chunk that would take the written total past `expected_size` stops
  the transfer, deletes the `.part` and its `.etag` sidecar, and fails fatally with an "oversized
  response body" error naming the expected size and "at least" the bytes seen. It is checked before
  the write, so a rogue server or middlebox cannot fill the disk: the loop stops within one chunk of
  the limit rather than draining the connection. This is not retryable — a longer body is a
  server-side fact, not a transient fault — and the `size * 3` preflight cannot help against an
  endless body, so this guard is what does.
- **A clean early EOF** (no transport error, fewer bytes than expected) is a *truncation*: the
  `.part` is kept and the retry loop resumes from it with `Range`, instead of reporting a checksum
  mismatch for bytes that never arrived. A body of exactly the expected size with the wrong digest
  is still a fatal `ChecksumMismatch` that deletes the `.part`.

### Content encoding is identity (C2)

Catalog digests are computed over the **raw published bytes**. The Valkey CDN serves two different
objects for the same URL depending on `Accept-Encoding`, so:

- Every request (initial, resumed with `Range`, and the re-issued request after a restart) sends
  `Accept-Encoding: identity`.
- A response carrying a `Content-Encoding` other than absent or `identity` is rejected fatally
  **before any body byte is written**, with an error that names the header value and states that
  artifacts must be fetched as identity. It is not retried.
- `reqwest`'s decompression features are **not** enabled, so the body is never transparently
  decoded; a test asserts the workspace `reqwest` line has no `gzip`/`brotli`/`zstd`/`deflate`
  feature.

### Proxies (C3)

`HTTP_PROXY` / `HTTPS_PROXY` / `NO_PROXY` are honoured (reqwest's default). That is deliberate:
corporate users behind a proxy need it, and it breaks no invariant — TLS and the HTTPS-only policy
still apply to every host dialled through the proxy.

- Cancellation is polled between chunks and inside the backoff sleep. On cancel the `.part` and
  `.etag` are deleted and `DblError::Cancelled` is returned, so `F-RTM-01 AC4` (< 1 s) holds even
  mid-stream.
- Progress is emitted at least every 200 ms and on the final byte, always with
  `bytes_done`, `bytes_total`, `bytes_per_sec` (from the bytes received in the last interval), and
  an `eta` derived from that speed. A first sample is emitted before the first chunk so the UI
  never shows an empty progress bar.

## URL policy (invariants 1 and 11)

- `UrlPolicy::HttpsOnly` is the default and the only variant constructible outside `#[cfg(test)]`.
  Both the initial URL and every redirect target are checked. `reqwest` is configured with
  `https_only(true)` as well, so the TLS layer refuses a downgrade even if the redirect policy were
  bypassed.
- Redirects are limited to `MAX_REDIRECTS = 5` via a custom `reqwest::redirect::Policy`; more hops
  is an error, not a silent truncation.
- The user agent is `dblocal/<CARGO_PKG_VERSION>` — the app name and version, nothing that
  identifies the machine, the OS user, or the instance.
- `UrlPolicy::AllowLoopbackHttp(TestOnlyLoopback)` exists only in test builds. It admits `http://`
  only when the host is `127.0.0.1`, `::1`, or `localhost`, and it cannot be constructed from
  production code because the enum variant is behind `#[cfg(test)]`.
- Diagnostics never include a query string, fragment, or userinfo (`redact_url`).

## Safe extraction

Every archive format is handled by the same pipeline:

| Format | Reader |
|---|---|
| `tar.gz` | `flate2::read::GzDecoder` over the file, then `tar::Archive` |
| `tar.xz` | `liblzma::read::XzDecoder` over the file, then `tar::Archive` |
| `zip` | `zip::ZipArchive` (deflate only by dependency features) |

For each entry:

1. **Name validation** (`extract::path`). The entry name is normalised and every component is
   checked. The rules reject:
   - absolute paths (`/etc/passwd`);
   - `..` anywhere, so `../x` and `a/../../x` both fail;
   - Windows drive prefixes (`C:`), UNC prefixes (`//server/share`), and backslash separators, on
     every host so an archive built on one OS cannot smuggle an escape on another;
   - NUL bytes.
2. **Containment.** `safe_join` re-joins the validated relative path onto the staging root and
   lexically verifies the result is a prefix of the staging root. Any entry that escapes is
   `UnsafeArchive`.
3. **Symlink ancestors.** Before writing a file, `ensure_parent_safe` walks every existing ancestor
   within the staging root and rejects the entry if any of them is a symlink. This is what stops
   the "extract a symlink, then extract a file through it" attack.
4. **Symlinks** are allowed only when the target is relative and, resolved against the link's
   directory, stays inside the staging root. Absolute targets are rejected. PostgreSQL's archive
   ships relative in-tree symlinks (for its shared libraries), and that is exactly the shape that
   is allowed.
5. **Hard links** (`tar` `Link` entries) are rejected outright. **Character/block devices and
   FIFOs** are rejected; `GNUSparse` and `Continuous` entries are treated as regular files because
   the `tar` crate has already reconstructed their contents.
6. **Entry count and total bytes** are capped. `max_entries` defaults to
   `MAX_EXTRACT_ENTRIES = 200_000`. The byte cap is
   `artifact_size_bytes * MAX_EXTRACT_BYTES_MULTIPLIER` clamped to `[256 MiB, 8 GiB]`; tests can
   lower it through `ExtractOptions`. Exceeding either cap is `UnsafeArchive` and removes the
   staging directory.
7. **Unix executable bit.** The stored mode is reapplied with `mode & 0o7777` (file-type bits
   stripped). A `0755` binary stays executable and a `0644` data file does not gain `+x`.
8. **Strip prefix.** `Artifact::strip_prefix` is matched **by path component**, not by string
   prefix, so a catalog entry `postgresql-18.7.0-linux-x86_64` cannot accidentally strip a sibling
   `postgresql-18.7.0-linux-x86_64-debug` entry. If any entry does not start with the required
   top-level directory, extraction fails. If strip prefix is set and no entry matched, extraction
   fails with `InvalidInput { field: "strip_prefix" }`.

## Atomic finalisation

- Extraction targets `.<runtime-name>.extracting-<random>` in the runtime's parent directory, so the
  staging tree is on the same filesystem as the final tree (which makes `rename` atomic).
- After extraction, the caller's `expected_binaries` are checked. A missing binary is
  `InvalidInput` and removes the staging directory.
- `rename` is called once. On Unix that is the whole story. On Windows the freshly extracted tree
  can be transiently locked by antivirus or an indexer, so the rename is retried up to
  `WINDOWS_RENAME_RETRIES` times with backoff between `WINDOWS_RENAME_INITIAL_BACKOFF` and
  `WINDOWS_RENAME_MAX_BACKOFF`. If it still fails, the error message names antivirus as the likely
  cause (Windows code is compiled but not run in this round).
- No code path writes directly into the final directory. If the final directory already exists,
  extraction refuses with `AlreadyExists { kind: "runtime" }` — the caller decides whether to skip
  (already installed) or remove it first.

## Disk-space preflight

`check_disk_space(path, artifact_size, probe)` requires `available >= artifact_size * 3` and fails
before any network request with an error that names the bytes needed and the bytes available (both
raw and human-friendly). `available_space_for_path` finds the deepest existing ancestor of the
downloads directory, then the longest-matching mount point from `sysinfo`, and reports that
filesystem's available space. Tests inject `FreeSpaceFn` to simulate a full disk without touching
the real one; `install_refused_before_any_request_when_disk_is_full` asserts the test HTTP server
received zero requests.

## Cancellation

`CancellationToken` is a clonable `Arc<AtomicBool>`. `cancel()` sets it; the transfer loop checks it
before every request and between chunks, and the backoff sleep polls it every 100 ms so cancelling
during a retry wait is also prompt. On cancel the `.part` and `.etag` sidecar are removed and
`DblError::Cancelled { operation: "download" }` (code `DBL-7001`) is returned.

## Test matrix (attack → outcome)

All tests run offline against an in-process TCP server on `127.0.0.1:0`.

| Attack / scenario | Outcome |
|---|---|
| `tar.gz` member `../evil.txt` | `UnsafeArchive`/`InvalidInput`, no final dir, no staging left |
| `tar.xz` member `../evil.txt` | same |
| `zip` member `../evil.txt` | same |
| `tar.gz` member `/etc/passwd` (absolute) | same |
| `zip` member `/abs/path.txt` (absolute) | same |
| `zip` member `C:/Windows/system32/evil.dll` (drive prefix) | same |
| `zip` member `//server/share/evil` (UNC) | same |
| `tar.gz` symlink with target `../../outside` | `UnsafeArchive` |
| `tar.gz` symlink target that walks out via `../..` | `UnsafeArchive` |
| File written through an already-extracted symlink directory | `UnsafeArchive` |
| `zip` symlink with target `../../outside` | `UnsafeArchive` |
| `tar.gz` symlink with absolute target `/etc/passwd` | `UnsafeArchive` |
| `tar.gz` symlink with no target field | `UnsafeArchive` |
| `tar.gz` hard link | `UnsafeArchive`, before any follow-up entry is written |
| `tar.xz` hard link | same |
| `tar.gz` FIFO / device entry | `UnsafeArchive` |
| `tar.gz` entry count over cap | `UnsafeArchive`, staging removed |
| `tar.gz` extracted bytes over cap | `UnsafeArchive`, staging removed |
| `strip_prefix` mismatch | `InvalidInput`, no final dir |
| `strip_prefix` set, archive has no matching root | `InvalidInput` |
| Truncated `tar.gz` / `tar.xz` body | `InvalidInput`/`Io`, no final dir |
| Non-zip bytes for a `zip` artifact | `InvalidInput`, no final dir |
| Expected binary missing after extraction | `InvalidInput`, no final dir, staging removed |
| Final runtime directory already exists | `AlreadyExists`, nothing overwritten |
| Relative symlink that stays inside the root | **allowed**, symlink created |
| `0755` binary in `tar.gz` | executable bit preserved |
| `0755` binary in `zip` | executable bit preserved |
| `0644` file in `tar.gz` | no `+x` bit added |
| Truncated connection, then resume | identical SHA-256 to a clean download |
| Server ignores `Range` | clean restart from byte 0; `Restarted` event; identical hash |
| Server sends mismatched `Content-Range` start | clean restart; `Restarted` event; identical hash |
| Pre-seeded `.part` + `.etag` | hashed first, resumed with one `Range` request |
| Wrong SHA-256 | `.part` and `.etag` deleted; error names expected and actual digests |
| Empty response body vs expected digest | `ChecksumMismatch`, `.part` deleted |
| Cancel mid-stream | `Cancelled`, `.part` and `.etag` deleted |
| Cancel before start | `Cancelled`, zero HTTP requests |
| HTTP 404 | typed `Network` error (code `DBL-3002`) |
| Transient 5xx twice, then 200 | retried, succeeds, exactly 3 requests |
| Connection dropped on first attempt | retried, succeeds |
| Persistent 500 with `max_retries = 2` | `Network` after 3 attempts |
| Free space < `size * 3` | `Io(StorageFull)` naming required and available bytes, zero requests |
| `http://` URL under `HttpsOnly` | `Network`, no socket opened |
| `http://` non-loopback under the test policy | rejected |
| Invalid URL in a `SourceSpec` | `InvalidInput` |
| HTTP 429 / 408 | retried, then succeeds |
| HTTP 404 | `Network`, exactly one request (no retry) |
| Unknown tar typeflag | `UnsafeArchive` |
| PAX / GNU global extension header | skipped, extraction succeeds |
| Symlink whose parent directory does not exist yet | parent created, link validated |
| Symlink with no target, absolute target | `UnsafeArchive` |
| Empty archive with `strip_prefix` | `InvalidInput { field: "strip_prefix" }` |
| Empty archive without `strip_prefix` | empty runtime tree, success |
| Overlong chunked body (no `Content-Length`) | fatal "oversized response body", `.part`+`.etag` deleted, one attempt, stops within one chunk of the limit |
| Overlong body on a resumed transfer | same, fatal |
| `Content-Length` larger than the catalog | fatal before writing, names both sizes, no `.part` |
| `Content-Range` total larger than the catalog (206) | fatal before writing |
| Oversized declared length on a resumed transfer | fatal before writing |
| Clean early EOF (short chunked body) | retried as a truncation, resumes, identical hash |
| Clean early EOF on the first response only | one short response then one resume, identical hash |
| Complete body with wrong digest | fatal `ChecksumMismatch`, `.part` deleted, not retried |
| `Content-Encoding: gzip` (server ignores identity) | fatal before writing, names the encoding, no `.part`, not retried |
| `Content-Encoding: gzip` on a resumed transfer | fatal before writing |
| `Content-Encoding: identity` explicit | accepted and verified |
| Server that gzips unless `identity` is requested | identity request gets raw bytes, hash matches |
| `reqwest` features in the workspace manifest | no `gzip`/`brotli`/`zstd`/`deflate` decompression |

## Notes and deviations

- The `downloads/` directory sits directly under the DBLocal root (`DblPaths::downloads_dir()`). It shares the runtimes filesystem, which is what makes the
  free-space check meaningful.
- Extraction runs on `tokio::task::spawn_blocking`; decompression is CPU-bound and must not stall
  the async runtime (PRD §9 rule 2).
- Windows-specific code (`rename_with_antivirus_retry`, `create_symlink` for Windows) is compiled
  behind `#[cfg(windows)]` and is **not run** in this round; it is reviewed for logic only.
