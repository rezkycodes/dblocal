# DBLocal engine catalog

This directory is the bundled source of truth for "what can I install on this
host". The application compiles `engines-v3.json` (`schema_version: 3`) into
the binary and refreshes it in the background. Older schema files are kept
frozen so already-released clients never break:

- `engines.json` (`schema_version: 1`) is kept frozen for early `v0.1.x` clients
  whose UI does not render EOL/status badges or the "Only show latest versions"
  filter.
- `engines-v2.json` (`schema_version: 2`) continues to be refreshed daily for
  released `v0.1.3` and `v0.1.4` clients, derived automatically from
  `engines-v3.json` with every `redis` entry, `deb` artifact, and `license`
  field stripped. Empirically verified against an archived `v0.1.4` tree
  (`cargo xtask catalog validate`): a `v0.1.4` client rejects the entire file on
  either an unknown engine (`unknown engine "redis"`) or an unknown archive
  format (`unknown archive format: deb`), causing daily refresh to degrade to
  `Stale`/`Offline`. Deriving v2 from v3 keeps released `v0.1.3`/`v0.1.4`
  clients receiving PostgreSQL, MySQL, MariaDB, and Valkey patch updates without
  seeing Redis or `.deb` entries (guarded by regression tests in `xtask`).

None of these files is edited by hand.

## Generating and validating

```sh
# Rebuild the v3 catalog and derived v2 catalog (HTTPS; needs network, and GITHUB_TOKEN for PostgreSQL).
cargo xtask catalog build --output catalog/engines-v3.json --compat-v2-output catalog/engines-v2.json

# Validate either catalog file offline (schema 2 or schema 3; used in CI).
cargo xtask catalog validate --file catalog/engines-v3.json
cargo xtask catalog validate --file catalog/engines-v2.json
```

The builder output is deterministic: entries are sorted, every artifact carries a
SHA-256 and a non-zero size, and the file ends with a trailing newline. A catalog
that fails validation is never published and never replaces a good cache.

## Scope

`engines.json` currently contains:

- **PostgreSQL** 18.7.0, 17.12.0 and 16.16.0 from
  `theseus-rs/postgresql-binaries`, for all six `(os, arch)` targets.
- **Valkey** 9.2.0, 8.1.12, 8.0.13 and 7.2.16 from `download.valkey.io`, for
  Linux `x86_64` and `aarch64`, in the **`jammy`** build only (glibc >= 2.34).
- **Redis** 7.4.12 from the official APT repository `https://packages.redis.io/deb`
  (`redis-tools` `7.4.12-1rl1~jammy1`, `.deb`, glibc >= 2.34), for Linux
  `x86_64` and `aarch64` only. Redis 7.0 is EOL and not offered; no Windows
  build exists. Redis 7.4 is dual-licensed **RSALv2 / SSPLv1** (recorded in the
  catalog `license` field); DBLocal does not host or redistribute Redis and
  points users' machines directly at Redis's official package.
- **MySQL** 26.10.0, 9.7.4, 8.4.13 and 8.0.46 (EOL), Linux `x86_64` (the minimal
  tarball), macOS and Windows `x86_64`.
- **MariaDB** 13.1.2, 12.3.4, 11.8.10, 11.4.14 and 10.11.20, Linux `x86_64` and
  Windows `x86_64` only.

### Why some targets are absent (never faked)

The catalog only lists a combination whose artifact upstream really publishes
(PRD D-24); everything else is reported as "not available on this platform" by
the read model, not by an invented URL.

- **MariaDB**: the project publishes no macOS build and no arm64 Linux tarball,
  so neither is listed. It also links `libcrypt.so.1`, which some current hosts
  lack; that is a runtime concern, not a catalog one.
- **MySQL**: Oracle's `minimal` Linux build exists only for `x86_64` (there is
  no `linux-glibc2.28-aarch64-minimal.tar.xz`), so arm64 Linux is not listed.
  The macOS builds are `macos15` only.

### Why Valkey `noble` is not published yet

`download.valkey.io` (CloudFront) has been observed serving **two different
objects for the same `noble` URL depending on `Accept-Encoding`**: a stale
cached object for `identity` and a fresh one for `gzip`, each with its own
matching `.sha256` sidecar. Pinning either hash would fail verification on every
install the moment the cache flips.

The builder reads the sidecar and the `HEAD` size under both encodings and fails
the build if they differ, so `noble` is excluded until upstream settles. Nothing
is lost: `jammy` has the lowest glibc floor (2.34) and runs wherever `noble`
(2.38) does, and its objects agree across encodings. Re-enabling `noble` is a data
change to `WAVE_ONE_VARIANTS` in `sources/valkey.rs` once the objects agree.

## Integrity

- **PostgreSQL**: the builder reads the per-asset `.sha256` sidecar published with
  each release asset.
- **Valkey**: the builder reads the per-asset `.sha256` sidecar from
  `download.valkey.io`, under both `identity` and `gzip`.
- **MariaDB**: the builder uses `checksum.sha256sum` from the
  `downloads.mariadb.org` REST API. The API reports `size: null` for the server
  tarballs, so the builder resolves the size with a `HEAD` on the HTTPS URL; the
  API's `http://` download URL is upgraded to `https://` before it is recorded.
- **MySQL**: Oracle publishes no SHA-256, only `.md5` and a GPG signature. The
  builder downloads the archive once, computes the SHA-256 and cross-checks the
  `.md5` sidecar before recording the hash. No checksum means no entry.
- **Redis**: the builder fetches `dists/jammy/InRelease` and
  `dists/jammy/main/binary-{amd64,arm64}/Packages.gz` over HTTPS (`rustls`),
  verifies that each `Packages.gz` matches the SHA-256 and byte size listed in
  `InRelease`, parses the `redis-tools` `7.4.12-1rl1~jammy1` stanza, and
  cross-checks the pinned `pool/` `.deb` by downloading and hashing it (reusing
  an existing catalog entry when `(engine, version, url)` and its verified hash
  and size are unchanged). Residual trust assumption: `InRelease` is verified
  via HTTPS/CA trust rather than offline OpenPGP signature verification, to
  avoid adding a heavy OpenPGP dependency.

### Redis 7.4.12 Linux (`jammy` APT `redis-tools`, 2026-11-26)

| Arch | URL | Size | SHA-256 |
|---|---|---|---|
| `x86_64` | `https://packages.redis.io/deb/pool/jammy/r/re/redis-tools_7.4.12-1rl1~jammy1_amd64.deb` | 1 586 598 bytes | `42727aa60b3595431055f11ff09c4761f52c237b013ca9d36f5c6328fe0657c2` |
| `aarch64` | `https://packages.redis.io/deb/pool/jammy/r/re/redis-tools_7.4.12-1rl1~jammy1_arm64.deb` | 1 597 148 bytes | `29ed3d4cad317ff75ddf4b1e62be5ccaefa6fbeabf0c166bca58fc4c57f00b50` |

Note: `redis-tools` links `libssl.so.3`, `libcrypto.so.3`, and `libsystemd.so.0`
(max glibc symbol `GLIBC_2.34`). DBLocal checks for those libraries before start
and prints actionable Debian/Ubuntu (`libssl3`, `libsystemd0`) and Fedora
(`openssl-libs`, `systemd-libs`) install instructions if any are missing.

### MySQL 8.4.11 Linux x86_64: recorded hash and cross-check (2026-11-22)

This is the one artifact proved end to end on this host (see
`docs/architecture/catalog.md`).

| Field | Value |
|---|---|
| URL | `https://cdn.mysql.com/Downloads/MySQL-8.4/mysql-8.4.11-linux-glibc2.28-x86_64-minimal.tar.xz` |
| Size | 79 777 440 bytes |
| SHA-256 | `383f54e124d5f325d67f0c6912a8f96814eedc761a17ea30112e52fa4cc6b143` |
| upstream `.md5` | `3358302178e35d893f14fc49be542847` |

The SHA-256 was reproduced by a **second, independent download of the same URL
with `curl`** plus `sha256sum`, giving the identical
`383f54e124d5f325d67f0c6912a8f96814eedc761a17ea30112e52fa4cc6b143`; its `md5sum`
also equals the upstream `.md5` above. The builder's own `.md5` cross-check
passes, so a corrupted or substituted archive is rejected before it is recorded.

### MariaDB 11.8.10 Linux x86_64 (2026-11-22)

| Field | Value |
|---|---|
| URL | `https://downloads.mariadb.org/rest-api/mariadb/11.8.10/mariadb-11.8.10-linux-systemd-x86_64.tar.gz` |
| Size | 413 068 258 bytes |
| SHA-256 | `15ca40274597af84fa8f89f8248b37740c5a712c003df80b4c7b32a4486e2027` (from the REST API `checksum.sha256sum`; the downloaded archive's own `sha256sum` matches) |

## Sizes the GUI shows

The `size_bytes` of every artifact is the real archive size reported by the
download source (MySQL/PostgreSQL `HEAD`/sidecar, MariaDB API `checksum`/`HEAD`),
and the downloader enforces it: a response that declares a different length, or a
body that runs past it, fails before extraction
(`crates/core/src/download/fetch/guard.rs`, `ensure_declared_length`).
