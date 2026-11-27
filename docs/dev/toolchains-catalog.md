# Toolchains Catalog (`catalog/toolchains-v1.json`)

DBLocal v0.3.0 adds managed local **Toolchains** (PHP, Node.js, and Composer)
alongside database engines. Toolchains use their own independent catalog file,
`catalog/toolchains-v1.json`, so existing engine catalogs (`engines.json`,
`engines-v2.json`, `engines-v3.json`) remain untouched.

Never edit `catalog/toolchains-v1.json` by hand. Always regenerate it through
`cargo xtask toolchains-catalog`.

## Schema (`schema: 1`)

```json
{
  "schema": 1,
  "generated_at": "2026-11-27T15:51:15Z",
  "tools": [
    {
      "id": "php",
      "versions": [
        {
          "version": "8.4.23",
          "series": "8.4",
          "artifacts": [
            {
              "os": "linux",
              "arch": "x86_64",
              "profile": "common",
              "format": "tar.gz",
              "url": "https://dl.static-php.dev/static-php-cli/common/php-8.4.23-cli-linux-x86_64.tar.gz",
              "sha256": "<64 lowercase hex>",
              "size_bytes": 18234567,
              "source": "static-php-cli",
              "hash_origin": "pinned",
              "extensions": ["bcmath", "ctype", "curl", "mbstring", "pdo_mysql"]
            }
          ]
        }
      ]
    }
  ]
}
```

### Field rules

- **Top-level**:
  - `schema` (`u32`): must be `1`.
  - `generated_at` (`string`): ISO-8601 UTC timestamp (`YYYY-MM-DDTHH:MM:SSZ`).
  - `tools` (`array`): sorted alphabetically by `id` (`composer`, `node`, `php`).
- **Tool entry**:
  - `id`: `"php"`, `"node"`, or `"composer"`.
  - `versions`: sorted **newest first** by semantic version `(major, minor, patch)`.
- **Version entry**:
  - `version`: semver string without leading `v` (e.g. `"8.4.23"`, `"26.13.1"`, `"2.11.3"`).
  - `series`: release line (`"8.4"` for PHP, `"26"` for Node.js, `"2"` for Composer).
  - `lts` (`bool`, optional): present on Node.js versions only (`true` when upstream `lts` is a release codename, `false` otherwise).
  - `artifacts`: sorted by `(os, arch, profile, url)`. Every `(os, arch, profile)` tuple is unique within a version.
- **Artifact entry**:
  - `os`: `"linux"`, `"windows"`, or `"any"` (Composer).
  - `arch`: `"x86_64"` or `"any"` (Composer).
  - `profile`: `"common"`, `"bulk"`, or `null` (always serialized).
  - `format`: `"tar.gz"`, `"zip"`, or `"phar"`.
  - `url`: `https://` URL only.
  - `sha256`: mandatory 64-character lowercase hex digest. Artifacts without a verifiable hash are never published.
  - `size_bytes`: exact byte length (`> 0`).
  - `source`: upstream identifier (`"static-php-cli"`, `"windows.php.net"`, `"nodejs.org"`, `"getcomposer.org"`).
  - `hash_origin`: `"pinned"` (computed by the generator because upstream publishes no checksum) or `"upstream"` (published by upstream).
  - `extensions` (`array<string>`, optional): PHP Linux builds only; sorted, deduplicated lowercase module names captured from `php -n -m` (`Zend OPcache` normalized to `opcache`).

### Determinism

Two runs against unchanged upstream sources produce byte-identical JSON
(2-space indent, trailing newline) except for `generated_at`.

Client parsers in `dblocal-core` must be tolerant of unknown fields, unknown
tools, unknown OS/arch values, and unknown profiles (skip with a warning rather
than failing the whole file). The generator's own validator remains strict so
typos fail CI.

## Upstream sources

1. **PHP — Linux x86_64 (`static-php-cli`)**
   - Index: `https://dl.static-php.dev/static-php-cli/{common,bulk}/?format=json`
   - Series: `8.0` through `8.5` (latest patch per series; no 7.4 on Linux).
   - Profiles: `common` and `bulk`.
   - Verification: upstream publishes no checksums, so the generator downloads
     each tarball, computes SHA-256 and exact size (`hash_origin: "pinned"`),
     extracts safely into a temporary directory, runs `php -n -v` and `php -n -m`
     to record the real extension list, and deletes the tarball immediately.
   - Observed deviations from profile summaries:
     - `php 8.0.30` (`common`) does **not** include `pdo_pgsql` (series 8.1–8.5
       `common` all include `pdo_pgsql`).
     - `php 8.5.8` (`common`) includes `opcache` (built into PHP 8.5 core).
     - `static-php-cli` patch versions can lag `windows.php.net` (e.g. 8.4.23 on
       Linux vs 8.4.28 on Windows); when patch versions differ, they appear as
       separate version entries under `php`, and when they match (e.g. 8.0.30,
       8.1.34) their artifacts are merged under one version entry.

2. **PHP — Windows x86_64 (`windows.php.net`)**
   - Index: `https://windows.php.net/downloads/releases/releases.json` (302
     redirects over HTTPS to `https://downloads.php.net/~windows/releases/releases.json`).
   - Series: `7.4` through `8.6`, selecting `nts-*-x64` (`nts-vc15-x64`,
     `nts-vs16-x64`, `nts-vs17-x64`, `nts-vs18-x64`) zip builds.
   - Verification: SHA-256 comes from `releases.json` (`hash_origin: "upstream"`);
     exact `size_bytes` and HTTP 200 reachability are verified via HTTPS `HEAD`
     without downloading the Windows archives. Windows binaries are not executed
     on the Linux build host.

3. **Node.js (`nodejs.org`)**
   - Index: `https://nodejs.org/dist/index.json` and per-version
     `https://nodejs.org/dist/v<ver>/SHASUMS256.txt`.
   - Lines: `26`, `24`, and `22` only (latest stable patch per line; pre-releases
     ignored).
   - Artifacts: `linux-x64` (`tar.gz`) and `win-x64` (`zip`), `hash_origin: "upstream"`.
   - Verification: exact sizes from HTTPS `HEAD`; Linux tarballs are additionally
     downloaded, verified against `SHASUMS256.txt` and `HEAD` size, extracted
     safely into a temp dir, smoke-tested with `bin/node -v` and `bin/npm -v`,
     and deleted immediately.

4. **Composer (`getcomposer.org`)**
   - Index: `https://getcomposer.org/versions` (latest stable `2.x` entry) and
     `https://getcomposer.org/download/<ver>/composer.phar.sha256sum`.
   - Artifact: `os: "any"`, `arch: "any"`, `profile: null`, `format: "phar"`,
     `hash_origin: "upstream"`.
   - Verification: `composer.phar` is downloaded, its computed SHA-256 is
     cross-checked against `.sha256sum`, and it is smoke-tested with
     `php -n composer.phar --version` using a verified Linux PHP binary from the
     same run (with `COMPOSER_HOME` and `HOME` pointed at a temp directory).

## Trust model and safety

- **HTTPS only**: plain HTTP URLs and any redirect to a non-HTTPS scheme are
  refused. Redirects are capped at 5 hops.
- **Polite client**: `User-Agent: dblocal-catalog-generator` with no host or
  user metadata; retries up to 3 times with exponential backoff on network
  errors, HTTP 429, and 5xx.
- **Resource bounds**: free disk check (`df`, minimum 512 MiB) before downloads;
  256 MiB per-artifact download cap; 2 GiB cumulative run cap; 512 MiB
  extraction cap; 30-second timeout per smoke-test command.
- **Safe extraction**: tar extraction rejects absolute paths, `..` traversal,
  symlinks/hardlinks escaping the extraction root, device/FIFO entries, and
  setuid/setgid mode bits.
- **Pinned-hash tamper detection**: if an existing `catalog/toolchains-v1.json`
  already records a `hash_origin: "pinned"` SHA-256 for a URL and a fresh
  download of that exact same URL hashes differently, the generator fails loudly
  instead of silently updating the hash.
- **Partial failure handling**: an individual artifact that fails (404, hash
  mismatch, failed smoke test) is omitted and reported under `skipped`. The
  command exits non-zero only if an entire tool ends up with zero versions (or
  a pinned hash changes for an unchanged URL).

## Regenerating and checking

```sh
# Generate catalog/toolchains-v1.json from live upstream sources:
cargo xtask toolchains-catalog --output catalog/toolchains-v1.json

# Verify that the committed catalog matches live upstream (ignoring generated_at):
cargo xtask toolchains-catalog --output catalog/toolchains-v1.json --check
```

## Daily refresh workflow (proposal)

The existing engine catalog refresh lives in `.github/workflows/catalog.yml`
(`jobs.refresh`). To include toolchains in the daily PR:

1. Add a step after `Build catalog`:
   ```sh
   cargo xtask toolchains-catalog --output catalog/toolchains-v1.json
   ```
   Because the generator checks existing pinned hashes before overwriting, an
   unchanged URL returning different bytes fails the workflow loudly instead of
   opening a silent hash-update PR. New patch releases get new URLs and pin
   cleanly.
2. Add `catalog/toolchains-v1.json` to `add-paths` in the
   `peter-evans/create-pull-request` step.

## How to add a new tool

1. Add the tool's `id` to `VALID_TOOLS` in
   `crates/xtask/src/toolchains_catalog/schema.rs` (and any new `format`, `os`,
   or `arch` values if applicable).
2. Create a new adapter module `crates/xtask/src/toolchains_catalog/<tool>.rs`
   implementing discovery, checksum verification, and optional host smoke tests
   through `ToolchainSource` and `SmokeRunner`.
3. Wire the adapter into `generate()` in
   `crates/xtask/src/toolchains_catalog/mod.rs`.
4. Add offline upstream fixtures under `crates/xtask/tests/fixtures/toolchains/`
   and unit tests in `crates/xtask/src/toolchains_catalog/tests.rs`.
5. Run `cargo test -p xtask`, regenerate `catalog/toolchains-v1.json`, and
   verify with `--check`.
