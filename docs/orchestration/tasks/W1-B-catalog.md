# W1-B — Engine catalog and `xtask catalog`

Branch: `w1-catalog`. Read first: AGENTS.md, `docs/architecture/CONTRACT.md`, PRD §6 `F-CAT-01..03`, decisions D-12, D-19, **D-24**, and
`docs/spikes/engines/upstream-sources.md` (the measured upstream facts you build on; do not guess URLs).

## Goal

(1) The runtime catalog: load a bundled snapshot, refresh it in the background, and answer "what can I install on this host". (2) The
builder that produces `catalog/engines.json` from upstream sources, with SHA-256 for every artifact.

## You own

`crates/core/src/catalog.rs`, `crates/core/src/catalog/**`, `crates/xtask/**` (you may add subcommands; keep `cargo xtask ci` working),
`catalog/**` (data and `catalog/README.md`), `docs/architecture/catalog.md`. You may add **only additive, backwards-compatible** fields
(with `serde`/builder defaults) to `model/catalog.rs` and `model/artifact.rs`; say so in your report. Do not change existing signatures.

## Deliverables

### Runtime (`crates/core`)

- Bundled snapshot via `include_str!("../../../catalog/engines.json")`, parsed and validated at load (schema version, https URLs, 64-hex
  SHA-256, `size_bytes > 0`, unique artifact ids, semver ordering). A corrupt bundled file is a build-time test failure, not a runtime panic.
- Cache under the path `DblPaths` gives you; atomic write (temp file plus rename); never replace a good cache with data that fails validation.
- Refresh: HTTPS only, `If-None-Match` with the stored ETag, timeout from a const, TTL 24 h (`consts`). **A failed refresh is silent**: the
  status becomes `Stale` (or `Offline`), never an error to the user (D-19, F-CAT-01 AC3). The HTTP source is behind a small trait so tests inject a
  fake; no test touches the network.
- `CatalogView` for a host `(Os, Arch)`: versions grouped per engine and major line, newest first, EOL flagged, and **combinations without an
  artifact are listed as "unavailable on this platform"**, never hidden and never an error (F-CAT-02 AC2, D-24). Provide
  `select_artifact(view, engine, version, host_glibc: Option<(u32,u32)>)` that honours `min_glibc` (Valkey: prefer the `jammy` build when the
  host glibc is below the `noble` floor; see W0-B finding C-03).

### Builder (`cargo xtask catalog ...`)

- `catalog build --output catalog/engines.json` (this exact command is already used by `.github/workflows/catalog.yml`), deterministic output
  (stable ordering, trailing newline, no timestamps in the data except an explicit `generated_at`).
- `catalog validate [--file path]` works fully offline and is what CI runs on every push.
- Sources (see the upstream table): PostgreSQL from the `theseus-rs/postgresql-binaries` releases API using each asset's `.sha256` sidecar;
  Valkey from `download.valkey.io` with `.sha256` sidecars and **arch token `arm64`** mapped to our `aarch64`; MariaDB from
  `downloads.mariadb.org/rest-api` (`checksum.sha256sum`); MySQL has **no SHA-256 upstream**, so the builder downloads once, computes SHA-256, cross-checks
  the `.md5` sidecar, records the hash, and prefers the `-minimal` tarball on Linux x86_64 (79.8 MB, verified to contain `mysqld` and the client tools).
  Reject any artifact whose checksum cannot be established. Never skip verification.
- **Scope for this wave:** the real `catalog/engines.json` contains PostgreSQL 18.7.0 / 17.12.0 / 16.16.0 (all six OS/arch targets) and Valkey 9.2.0
  (Linux `jammy` and `noble`, x86_64 and aarch64). The MySQL and MariaDB code paths are implemented and tested against recorded fixtures,
  but their real data is generated in Phase 2; do **not** download the 832 MB MySQL archives now.
- Offline tests with recorded fixture responses served by a local server on port 0 (ephemeral). `catalog build` never writes a partial file.

## Done when

`cargo xtask ci` green, >= 80% lines on files you own, `cargo xtask catalog validate` passes on the generated file, and your report shows the
generated file's summary table (engine, version, targets, sizes) plus every command you ran to produce it.
