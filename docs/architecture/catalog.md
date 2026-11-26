# Engine catalog

The engine catalog answers one question: "what can I install on this host?"

It is the only place in DBLocal that knows where an engine comes from, which
builds exist, and how to verify them. Nothing in the app guesses a URL or skips
a checksum.

## Bundled snapshot and runtime refresh

`catalog/engines.json` is compiled into the binary and also cached under
`~/.dblocal/cache/catalog.json` after a successful refresh.

- The bundled snapshot keeps a fresh install usable offline.
- A background refresh runs at most once per 24 hours, uses the cached `ETag`
  with `If-None-Match`, and only replaces the cache with a payload that passes
  validation.
- A failed refresh is silent: the previous snapshot stays in use and the
  status becomes `stale` or `offline` (never an error surfaced to the user).
- A payload with an unknown, higher `schema_version` is rejected and the
  bundled snapshot remains authoritative.

## What the catalog contains

Each document records:

- `versions`: released engine versions, grouped per engine and sorted newest
  first, with `major_line` and `eol`.
- `artifacts`: one downloadable build per `(engine, version, os, arch)` target
  (and per `variant` when the same target has more than one compatible build;
  the schema carries `variant`/`min_glibc` for that, and the runtime selection
  that uses it is `select_artifact`). Every artifact carries an absolute
  HTTPS `url`, a 64-character lowercase `sha256`, a non-zero `size_bytes`,
  an archive `format`, and optional `strip_prefix`, `min_glibc` and `variant`.
  Valkey publishes `jammy` only; see `catalog/README.md` for why.

The bundled snapshot covers all four engines. On Linux `x86_64` every engine has
at least one installable version; the per-platform gaps are real and come from
upstream, not from the catalog hiding them:

- MariaDB publishes no macOS build and no arm64 Linux tarball.
- MySQL publishes no arm64 Linux `minimal` tarball (only `x86_64`), and its macOS
  builds are `macos15`.

A combination with no artifact is shown by the read model as "not available on this
platform" (F-CAT-02 AC2), never as an error and never with an invented URL.

Combinations with no artifact are reported as unavailable on that platform.
They are never hidden and never treated as an error.

## Validation

`cargo xtask catalog validate` (offline) and the runtime loader enforce the
same checks:

- supported `schema_version` and an HTTPS `source_url`;
- unique artifact ids and unique `(engine, version)` pairs;
- per-engine semver ordering, newest first;
- 64-character lowercase hex SHA-256, non-zero `size_bytes`, absolute HTTPS
  URL, and a known format/OS/architecture;
- every artifact targets a version listed in the document.

A corrupt bundled file is a build-time test failure, not a runtime panic.

## Builder

`cargo xtask catalog build --output catalog/engines.json` assembles the file
from upstream sources, verifies checksums, and writes the document
deterministically (stable ordering, trailing newline). It never writes a
partial file.

Trust boundaries:

- Requests are HTTPS-only and follow at most five redirect hops that stay on
  HTTPS; an `https` -> `http` redirect fails the build.
- Every request pins `Accept-Encoding: identity`, and each Valkey artifact is
  re-read under `gzip` and must describe the same object (same sidecar, same
  `HEAD` size) or the build fails naming the artifact.
- `generated_at` is today's UTC date only when the document actually changed;
  otherwise the existing stamp is kept, so a no-op refresh is a no-op diff.

Offline fixture tests exercise the PostgreSQL, Valkey, MariaDB and MySQL
source paths without touching the network or downloading large archives.

Three defects surfaced the first time the builder produced real MySQL and MariaDB
entries, all now fixed and covered by those fixture tests:

- Both adapters set `strip_prefix` to a name no archive contains
  (`mysql-{version}-minimal`, and `None` for MariaDB), so `bin/` extracted one
  level too deep and the runtime's expected-binary check would fail. It is now
  always the archive's real root (`archive_root`), which also keeps the MariaDB
  Windows zip correct.
- MariaDB's REST API reports `size: null` for the server tarballs, so the old
  code refused every real release; the builder now resolves the size with a
  `HEAD`, upgrades the API's `http://` URL to `https://`, and drops the plugin and
  debug-symbol tarballs that share the `(os, arch)` target.

The one MySQL artifact proved end to end on this host (Linux `x86_64`) is
recorded with its exact URL, size and SHA-256, plus a second independent
download that reproduces the hash, in `catalog/README.md`.
