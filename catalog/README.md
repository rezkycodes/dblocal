# DBLocal engine catalog

This directory is the bundled source of truth for "what can I install on this
host". The application compiles `engines.json` into the binary and refreshes it
in the background; the file here is never edited by hand.

## Generating and validating

```sh
# Rebuild the catalog (HTTPS, needs network and GITHUB_TOKEN for PostgreSQL).
cargo xtask catalog build --output catalog/engines.json

# Validate any catalog file offline (used in CI).
cargo xtask catalog validate --file catalog/engines.json
```

The builder output is deterministic: entries are sorted, every artifact carries a
SHA-256 and a non-zero size, and the file ends with a trailing newline. A catalog
that fails validation is never published and never replaces a good cache.

## Scope of this wave

`engines.json` currently contains:

- PostgreSQL 18.7.0, 17.12.0 and 16.16.0 from
  `theseus-rs/postgresql-binaries`, for all six `(os, arch)` targets.
- Valkey 9.2.0 from `download.valkey.io`, for Linux `x86_64` and `aarch64`, in
  the **`jammy`** build only (glibc >= 2.34).

### Why Valkey `noble` is not published yet

`download.valkey.io` (CloudFront) has been observed serving **two different
objects for the same `noble` URL depending on `Accept-Encoding`**: a stale
cached object for `identity` and a fresh one for `gzip`, each with its own
matching `.sha256` sidecar. Pinning either hash would fail verification on every
install the moment the cache flips.

The builder now reads the sidecar and the `HEAD` size under both encodings and
fails the build if they differ, so `noble` is excluded until upstream settles.
Nothing is lost: `jammy` has the lowest glibc floor (2.34) and runs wherever
`noble` (2.38) does, and its objects agree across encodings. Re-enabling `noble`
is a data change to `WAVE_ONE_VARIANTS` in `sources/valkey.rs` once the objects
agree; the `min_glibc` selection code and its tests stay in place.

MySQL and MariaDB source adapters are implemented and covered by fixture tests,
but their real artifacts are generated in a later wave (MySQL upstream does not
publish SHA-256, and its archives are large).

## Integrity

- PostgreSQL: the builder reads the per-asset `.sha256` sidecar published with
  each release asset.
- Valkey: the builder reads the per-asset `.sha256` sidecar from
  `download.valkey.io`.
- MariaDB (code path): the builder uses `checksum.sha256sum` from the MariaDB
  REST API.
- MySQL (code path): the builder computes SHA-256 over the downloaded archive
  and cross-checks the `.md5` sidecar before recording the hash. No checksum
  means no catalog entry, ever.
