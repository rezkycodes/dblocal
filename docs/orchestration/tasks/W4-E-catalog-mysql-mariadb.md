# W4-E — real catalog entries for MySQL and MariaDB (F-CAT-03, F-ENG-01/02 need artifacts to download)

Start only after W3-B1 and W3-B2 are merged (the adapters decide which versions and layouts are valid). Files you own: `catalog/**`, `crates/xtask/src/catalog/**`, `docs/architecture/catalog.md`, `catalog/README.md`. New branch from `main`. Rules: `docs/orchestration/tasks/_COMMON-RULES.md` incl. 11-14. This task is mostly network and disk, little CPU: run the builder at low load and delete every download afterwards.

## Situation

`xtask catalog` already knows how to build MariaDB entries (REST API `checksum.sha256sum`) and MySQL entries (no upstream SHA-256: download once, compute SHA-256 ourselves, cross-check the `.md5` sidecar), but `catalog/engines.json` contains only PostgreSQL and Valkey. Without entries the service can never install either engine. Known facts from W0-B: MySQL archive ~832 MB; MariaDB official binaries exist for Linux x64 and Windows x64 only (no macOS, no arm64 Linux tarball from the project); a MariaDB 11.4 server needs `libcrypt.so.1`, which this host lacks.

## Deliverables

1. Select versions with the adapter authors' evidence, not by guess: MySQL 8.4 LTS line (the e2e uses 8.4.11) and the MariaDB versions the adapter e2e used (11.4 and 11.8). For each OS/arch the upstream really publishes, add an artifact; for the ones it does not, add no artifact and make the catalog view say why ("tidak tersedia untuk platform ini"), asserting that in a test. Never invent an artifact URL.
2. Regenerate `catalog/engines.json` with the builder (`xtask catalog build`), not by hand. MySQL SHA-256 computed from our own download, `.md5` cross-checked, both recorded in `catalog/README.md` with the date and the exact URL. A second independent download of one artifact (different tool, e.g. `curl` + `sha256sum`) must give the same SHA-256: paste it.
3. `xtask catalog validate` and the schema/version tests green; the bundled snapshot tests updated (counts, filters per platform) with the reason for every changed expectation in the commit message.
4. Prove one artifact end to end on this host: a real download of MySQL 8.4 (Linux x64) through the real downloader into a temp `DBLOCAL_HOME`, extraction layout accepted by the adapter, then `dblocal create --engine mysql ... --no-password`, start, connect on loopback, stop, delete (e2e lock; ports 20000+; engine must be gone, `ss -ltnp` clean). Same for MariaDB 11.8 with the `libcrypt.so.1` shim if 11.4 cannot run (say which). Delete the downloads and the temp home; report `df -h /` before and after.
5. Size/UX facts the GUI needs (download size, extracted size) must come from the artifact, with a test that a wrong declared size is fatal (the downloader already enforces it; show the existing test or add one).

## Bar

`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo deny check`, catalog tests x40, `cargo xtask ci` under the heavy lock. Not verifiable here, list it: macOS and Windows artifact downloads and extraction, arm64.
