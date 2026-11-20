# Upstream Source Table (metadata only)

Date: 2026-11-20. Metadata gathered via GitHub Releases API, MariaDB REST API, HTTP `HEAD` on vendor URLs, and `download.valkey.io` name probes. **No MySQL, MariaDB, macOS, or Windows engine archives were downloaded** (as required).

Legend: **exists** = HEAD 200 with size observed; **missing** = HEAD 404/403 or no artifact published; **n/a** = not published by vendor for this combo. Sizes are `Content-Length` in bytes from HEAD where available.

---

## 1. PostgreSQL — `theseus-rs/postgresql-binaries`

- **Source of truth:** GitHub Releases API `https://api.github.com/repos/theseus-rs/postgresql-binaries/releases` (per-release JSON lists every asset, `name`/`size`/`browser_download_url`).
- **URL pattern:** `https://github.com/theseus-rs/postgresql-binaries/releases/download/<version>/postgresql-<version>-<target>.{tar.gz,zip}`
- **SHA-256:** per-asset sidecar `...<asset>.sha256` (format `"<hex>  <filename>\n"`), published for every binary asset. Also published per asset: `.manifest.json`, `.provenance.json` (SLSA), `.sbom-attestation.json`, `.spdx.json`.
- **Archive format:** `tar.gz` on Linux/macOS; `tar.gz` **and** `zip` on Windows (MSVC targets). Musl targets are dynamically linked (per upstream README).
- **Layout/strip_prefix:** single top-level dir `postgresql-<version>-<target>/` containing `bin/`, `lib/`, `include/`, `share/`, etc. (verified 18.7.0/17.12.0 x86_64 gnu: 2895/2856 entries). Strip the top-level dir so `bin/` is one level inside the runtime root.
- **License/redistribution note:** binaries are upstream-built; DBLocal downloads and verifies, it does not redistribute (D-12).

Latest per-major (API, 2026-11-20): 18.7.0, 17.12.0, 16.16.0 (also 15.20.0, 14.25.0 present).

| OS | Arch | Target triple (asset token) | 18.7.0 asset / size (bytes) | 17.12.0 size | 16.16.0 size | Exists |
|---|---|---|---|---:|---:|---|
| linux | x86_64 | `x86_64-unknown-linux-gnu` | `.tar.gz` 40,530,262 | 40,118,562 | 39,431,025 | **yes** (all majors checked) |
| linux | x86_64 | `x86_64-unknown-linux-musl` | `.tar.gz` 103,481,839 | 104,667,994 | 103,410,244 | **yes** |
| linux | aarch64 | `aarch64-unknown-linux-gnu` | `.tar.gz` 39,931,153 | 39,514,574 | 38,844,516 | **yes** |
| linux | aarch64 | `aarch64-unknown-linux-musl` | `.tar.gz` 101,650,090 | 102,853,529 | 101,593,269 | **yes** |
| macos | x86_64 | `x86_64-apple-darwin` | `.tar.gz` 92,187,545 | 93,377,231 | 92,163,354 | **yes** |
| macos | aarch64 | `aarch64-apple-darwin` | `.tar.gz` 88,965,986 | 90,196,480 | 89,018,605 | **yes** |
| windows | x86_64 | `x86_64-pc-windows-msvc` | `.tar.gz` 49,777,647 / `.zip` 49,797,717 | 49,402,184 / 49,403,508 | 48,702,594 / 48,747,996 | **yes** |
| windows | aarch64 | `aarch64-pc-windows-msvc` | `.tar.gz` 48,504,439 / `.zip` 48,512,646 | 48,139,585 / 48,137,606 | 47,471,093 / 47,511,435 | **yes** (upstream README: Windows ARM64 packages require PG 16+) |

Notes: gnu linux is effectively self-contained (bundled libs, max GLIBC_2.34 on 18.7.0 x86_64, verified). Musl archives are dynamically linked per upstream but statically friendlier to host libc. macOS is a universal-ish pair (separate arm64/x86_64). Windows ships both `.tar.gz` and `.zip` — prefer `.zip` for native extraction on Windows.

## 2. MySQL 8.4 LTS

- **Source:** Oracle MySQL Community downloads. User-facing landing: `https://dev.mysql.com/downloads/mysql/` (select 8.4 LTS). Programmatic download redirector: `https://dev.mysql.com/get/Downloads/MySQL-8.4/<filename>` → `https://cdn.mysql.com/Downloads/MySQL-8.4/<filename>` (or `cdn.mysql.com//archives/...` for older point releases).
- **SHA-256:** **no `.sha256` / `.sha256sum` file found on CDN** (HEAD 404 for `mysql-8.4.11-linux-glibc2.28-x86_64.tar.xz.sha256{,sum}`). **MD5** sidecar exists (`.md5`, e.g. `c5a7dabc8c0d91b5974d2ca43872aeeb  mysql-8.4.11-linux-glibc2.28-x86_64.tar.xz`). **GPG signature** exists (`.asc`, HEAD 200, ~833 bytes). The downloads page shows checksums in HTML (scraping would be brittle). **Implication:** to satisfy DBLocal's "SHA-256 mandatory, no skip" invariant, the catalog builder must compute SHA-256 itself by downloading once in CI (or verify GPG and record the computed SHA-256) — upstream does not provide a SHA-256 to trust directly.
- **Archive format:** Linux `.tar.xz` (glibc build), macOS `.tar.gz`, Windows `.zip` and `.msi`.
- **Version probed:** 8.4.11 is the latest 8.4.x observed live (8.4.10 present, 8.4.12 → 404). CDN paths also keep older point releases under `/archives/mysql-8.4/`.

| OS | Arch | 8.4.11 asset (filename pattern) | Format | Size bytes (HEAD) | Exists |
|---|---|---|---|---:|---|
| linux | x86_64 | `mysql-8.4.11-linux-glibc2.28-x86_64.tar.xz` | tar.xz | 832,331,100 (~794 MiB) | **yes** |
| linux | aarch64 | `mysql-8.4.11-linux-glibc2.28-aarch64.tar.xz` | tar.xz | 821,708,368 (~784 MiB) | **yes** |
| macos | x86_64 | `mysql-8.4.11-macos15-x86_64.tar.gz` | tar.gz | 171,974,960 (~164 MiB) | **yes** (macos15 only; macos14/macOS13 names → 404 for 8.4.10 and 8.4.11) |
| macos | aarch64 | `mysql-8.4.11-macos15-arm64.tar.gz` | tar.gz | 167,977,240 (~160 MiB) | **yes** (macos15 only) |
| windows | x86_64 | `mysql-8.4.11-winx64.zip` / `mysql-8.4.11-winx64.msi` | zip/msi | 281,191,914 / 136,257,536 | **yes** |
| windows | aarch64 | `mysql-8.4.11-winx64-arm64.zip` (guessed) | zip | n/a | **missing** (HEAD 404 for 8.4.10) |

URL pattern: `https://cdn.mysql.com/Downloads/MySQL-8.4/mysql-<version>-<platform-arch>.<ext>` (redirector `https://dev.mysql.com/get/Downloads/MySQL-8.4/...`). **Terms/programmatic use:** dev.mysql.com requires accepting Oracle terms in-browser for some flows; the `dev.mysql.com/get/...` redirect and direct `cdn.mysql.com` URLs were reachable via HEAD/curl in this environment, but the catalog builder must respect Oracle's licensing/ToS for automated redistribution/download (OPEN-07 in PRD). macOS artifacts are labeled `macos15` — compatibility with older macOS versions is not guaranteed by the filename and must be validated. Linux glibc floor is encoded in the name (`glibc2.28`).

## 3. MariaDB (latest LTS: 11.8; also 11.4 is LTS)

- **Source of truth:** REST API `https://downloads.mariadb.org/rest-api/mariadb/` (lists `major_releases`) and per-release `https://downloads.mariadb.org/rest-api/mariadb/<version>/` (JSON with `files[]`: `file_name`, `os`, `cpu`, `checksum.{md5,sha1,sha256,sha512}`, `file_download_url`, `signature`, checksum/signature URLs). This is the right machine-readable catalog source.
- **SHA-256:** **provided by the REST API per file** (`checksum.sha256sum`) and as separate checksum/signature resources/PGP signatures — strong integrity story.
- **Mirror/download:** `file_download_url` in the API is a REST redirector; mirrors serve the actual bytes (e.g. `https://mirror.citrahost.com/mariadb/mariadb-11.8.10/...`). Prefer the REST/API URL or a documented mirror; record the API checksum.
- **Archive format:** Linux `tar.gz` (generic bintar), Windows `.zip` and `.msi`. Source `.tar.gz` also listed.

Latest LTS lines (API, 2026-11-20): **11.8.10** (2026-11-17, current LTS) and **11.4.14** (2026-11-17, previous LTS). (13.x is rolling/newer; 10.11 still listed as major release.)

### 11.8.10 (LTS) — binary artifacts (from REST API + mirror HEAD)

| OS | Arch | Asset | Format | Size bytes (HEAD/mirror or API context) | Exists | SHA-256 source |
|---|---|---|---|---:|---|---|
| linux | x86_64 | `mariadb-11.8.10-linux-systemd-x86_64.tar.gz` | tar.gz | 413,068,258 (~394 MiB) | **yes** (`bintar-linux-systemd-x86_64/`) | REST API `checksum.sha256sum` = `15ca40274597af84fa8f89f8248b37740c5a712c003df80b4c7b32a4486e2027` |
| linux | aarch64 | `mariadb-11.8.10-linux-systemd-aarch64.tar.gz` (expected name) | tar.gz | n/a | **missing** (no `bintar-linux-systemd-aarch64/` dir on mirror for 11.8.10; REST probe path 404) | n/a |
| macos | x86_64 | `osx-x86_64` bintar | tar.gz | n/a | **missing** (no `bintar-osx-x86_64/` for 11.8.10) | n/a |
| macos | aarch64 | `osx-aarch64` bintar | tar.gz | n/a | **missing** (no `bintar-osx-aarch64/` for 11.8.10) | n/a |
| windows | x86_64 | `mariadb-11.8.10-winx64.zip` / `.msi` | zip/msi | 99,027,722 / 90,214,400 | **yes** (`winx64-packages/`) | REST API sha256 present |
| windows | aarch64 | — | — | n/a | **missing** (no win arm64 artifact in 11.8.10 file list) | n/a |

### 11.4.14 (previous LTS) — same availability pattern observed

| OS | Arch | Asset | Exists | Note |
|---|---|---|---|---|
| linux | x86_64 | `mariadb-11.4.14-linux-systemd-x86_64.tar.gz` | **yes** | mirror dir present |
| linux | aarch64 | — | **missing** | no aarch64 bintar dir |
| macos | x86_64/arm64 | — | **missing** | no osx bintar dirs |
| windows | x86_64 | `.zip`/`.msi` | **yes** | — |
| windows | aarch64 | — | **missing** | — |

**Implication:** MariaDB's official prebuilt generic tarballs for the LTS lines checked are **Linux x86_64 + Windows x86_64 only**. macOS and Linux aarch64 (and Windows arm64) bintars are not published for 11.8.10/11.4.14 in the locations probed — a significant gap for "four engines across linux/macos/windows × x86_64/aarch64". Packaging for those targets would require building from source or sourcing from distro packages (out of scope of this spike).

## 4. Valkey

- **Source:** `https://download.valkey.io/releases/` (binaries) and GitHub `valkey-io/valkey` releases (source tags only — **0 binary assets** on GitHub releases 9.2.0 down to 7.2.16). No public S3 directory listing.
- **SHA-256:** sidecar `https://download.valkey.io/releases/valkey-<version>-<variant>-<arch>.tar.gz.sha256` (format `"<hex>  <filename>\n"`, 99 bytes) — **present for x86_64 and arm64** (verified 200). `*.sha256sum` form returned 403 for arm64.
- **Archive format:** `.tar.gz` (Linux Ubuntu-targeted builds). **No macOS, no Windows** binary archives published at download.valkey.io (guessed names → 403) and none on GitHub releases.
- **Arch token:** **`x86_64`** and **`arm64`** (not `aarch64`; `aarch64` → 403).
- **Variants:** `noble` (Ubuntu 24.04 lineage, glibc >= 2.38 observed) and `jammy` (Ubuntu 22.04 lineage, glibc >= 2.34 observed). Not self-contained (links host `libc`, `libm`, `libssl.so.3`, `libcrypto.so.3`, `libsystemd.so.0`, `libz`).

### 9.2.0 (latest stable, probed)

| OS | Arch | Asset | Variant | Size bytes | Exists | SHA-256 |
|---|---|---|---|---:|---|---|
| linux | x86_64 | `valkey-9.2.0-noble-x86_64.tar.gz` | noble | 45,651,721 | **yes** | `871066be...` |
| linux | x86_64 | `valkey-9.2.0-jammy-x86_64.tar.gz` | jammy | 46,759,925 | **yes** | `27ea653c...` |
| linux | aarch64 (arm64) | `valkey-9.2.0-noble-arm64.tar.gz` | noble | 46,430,275 | **yes** | sidecar `.sha256` 200 |
| linux | aarch64 (arm64) | `valkey-9.2.0-jammy-arm64.tar.gz` | jammy | 46,686,566 | **yes** | sidecar `.sha256` 200 |
| macos | x86_64 | — | — | n/a | **missing** | n/a |
| macos | aarch64 | — | — | n/a | **missing** | n/a |
| windows | x86_64 | — | — | n/a | **missing** | n/a |
| windows | aarch64 | — | — | n/a | **missing** | n/a |

9.1.4 shows the same Linux availability pattern (x86_64 + arm64, noble/jammy, with `.sha256`). No macOS/Windows binaries for any probed version.

## 5. Coverage matrix (summary)

| Engine | linux x86_64 | linux aarch64 | macos x86_64 | macos aarch64 | windows x86_64 | windows aarch64 |
|---|:---:|:---:|:---:|:---:|:---:|:---:|
| **PostgreSQL** (theseus-rs, 18.7.0) | ✅ gnu+musl | ✅ gnu+musl | ✅ | ✅ | ✅ (.zip/.tar.gz) | ✅ (PG16+) |
| **MySQL 8.4** (8.4.11) | ✅ glibc2.28 `.tar.xz` | ✅ glibc2.28 `.tar.xz` | ✅ macos15 | ✅ macos15 | ✅ zip+msi | ❌ |
| **MariaDB 11.8 LTS** (11.8.10) | ✅ systemd bintar | ❌ | ❌ | ❌ | ✅ zip+msi | ❌ |
| **Valkey** (9.2.0) | ✅ noble+jammy | ✅ noble+jammy (`arm64`) | ❌ | ❌ | ❌ | ❌ |

**Checksum availability:** PostgreSQL per-asset SHA-256 sidecar ✅; MariaDB SHA-256 in REST API (+ sha512/md5/sha1/PGP) ✅; Valkey per-asset `.sha256` sidecar ✅ (x86_64 & arm64); MySQL **SHA-256 not published** (MD5 `.md5` + GPG `.asc` only) — requires CI-computed SHA-256 to meet the product invariant.

## 6. Method and evidence

- PostgreSQL: `GET https://api.github.com/repos/theseus-rs/postgresql-binaries/releases?per_page=100` (sizes/names/URLs from JSON); checksum format confirmed by fetching `.sha256` files.
- MySQL: `HEAD` on `dev.mysql.com/get/...` (redirect target, code) and `cdn.mysql.com/Downloads/MySQL-8.4/...` (Content-Length, checksum sidecars).
- MariaDB: `GET https://downloads.mariadb.org/rest-api/mariadb/` and `/mariadb/<version>/` (JSON), `HEAD` on mirror file URLs and mirror directory listings.
- Valkey: `HEAD` on `https://download.valkey.io/releases/valkey-<ver>-{noble,jammy}-{x86_64,arm64}.tar.gz{,.sha256}` and negative probes for macOS/Windows/`aarch64` naming; GitHub releases API for asset presence.
- Script: `spikes/engines/upstream/probe_upstream.sh` (reproducible probe; captures to `/tmp/upstream-probe.txt`).
