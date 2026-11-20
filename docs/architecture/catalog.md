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
  (and per `variant` when the same target has more than one compatible build,
  as Valkey does with `jammy`/`noble`). Every artifact carries an absolute
  HTTPS `url`, a 64-character lowercase `sha256`, a non-zero `size_bytes`,
  an archive `format`, and optional `strip_prefix`, `min_glibc` and `variant`.

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
deterministically (stable ordering, explicit `generated_at`, trailing
newline). It never writes a partial file.

Offline fixture tests exercise the PostgreSQL, Valkey, MariaDB and MySQL
source paths without touching the network or downloading large archives.
