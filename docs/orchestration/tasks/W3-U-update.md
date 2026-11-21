# W3-U — Self-update engine (F-PKG-05), core only

Branch `w3-update` from `main`. Read `docs/orchestration/tasks/_COMMON-RULES.md` first. PRD: **F-PKG-05** (AC1..AC4), F-PKG business rules (checksums.txt, provenance), `docs/architecture/download.md` (reuse the downloader, its quotas and `Fetcher`/`UrlPolicy` abstractions), `docs/architecture/catalog.md` (how network access is injected for tests), `docs/architecture/secrets.md`. Core-only: `CARGO_TARGET_DIR=$HOME/.cache/dblocal-target`.

## Goal

The logic behind Settings → Updates, with no UI: find a newer release, download and verify it, and install it safely, or refuse and point to the package manager. The GUI and CLI wire it later.

## You own

New module `crates/core/src/update/**` (plus its tests) and `docs/architecture/update.md`; one `pub mod update;` line in `crates/core/src/lib.rs`; new dependencies only if needed and only if `cargo deny` accepts them (`semver` is fine). You may not edit `download/**`, `catalog/**`, `instance/**`, `engine/**`, `crates/gui/**`, `crates/cli/**`: reuse their public API; if you need a change there, `ask` with the exact change.

## Requirements

1. **Check** (AC1): `check(current: &Version, fetcher, options) -> Result<UpdateCheck>` reads the GitHub Releases API (`/repos/<owner>/<repo>/releases`), ignores drafts, ignores prereleases unless the channel allows them, picks the newest semver greater than `current`, returns version, release notes (bounded length, plain text) and the asset for this OS/arch (`linux-x86_64`, `linux-aarch64`, `macos-universal`, `windows-x86_64`; names as in `release.yml`). No update, a malformed payload, a rate limit and a network failure are distinct typed outcomes (Indonesian messages, stable DBL codes). No periodic timer here: the caller decides when to check.
2. **Install kind** (AC2): `detect_install_kind()` returns `AppImage`, `PortableTarball`, `WindowsZip`, `WindowsMsi`, `MacApp`, `DistPackage` (path owned by dpkg/rpm: ask the package database through a trait so tests inject the answer), `Homebrew`, or `Unknown`. For `DistPackage` and `Homebrew` self-update is DISABLED and the result carries the package-manager hint text. `Unknown` is also refused rather than guessed.
3. **Download and verify** (AC4): HTTPS only, the same `UrlPolicy::HttpsOnly` as runtimes; redirects only to `github.com` and `objects.githubusercontent.com` hosts (named constant allowlist); size cap from the release metadata and a hard ceiling (60 MB + slack); resumable only if the existing downloader already does it; the sha256 comes from the release's `checksums.txt` asset, compared in constant time BEFORE anything is installed; a mismatch deletes the download and returns a typed error. A downgrade (candidate <= current) is refused unless `allow_downgrade` is set.
4. **Install plan and apply** (AC3): `plan_install(kind, staged_file, current_exe) -> InstallPlan` is a pure value (steps), then `apply(plan)` executes it. Linux AppImage/tarball: write the new binary next to the old one with the same mode, fsync, atomically `rename` over it, keep one `.old` backup until the next successful start. Windows: the running exe cannot be replaced, so the plan is "spawn a detached helper that waits for this pid to exit, swaps the file, restarts the app"; implement the helper entry as a function the binary can call (`dblocal --apply-update ...` wiring is the CLI worker's job later) and unit-test the command-line construction. macOS: plan only (replace the `.app` bundle contents), unexecuted. Every path comes from the plan, never from the archive or the release JSON (no path traversal through asset names).
5. **Rollback**: if the new binary fails a smoke check (`--version` exits 0 and prints the expected version) the old one is restored and the error says so. Test it with a fake binary.

## Tests

Fixture JSON of real-shaped GitHub responses (prereleases, drafts, many assets, a huge notes body); a local test HTTP server (see how `download` tests do it) for the download path; hostile cases: asset name with `../`, redirect to another host, oversized body, truncated download, wrong checksum, `checksums.txt` missing the asset, version string garbage, downgrade; mutation checks on the checksum comparison, the host allowlist and the downgrade refusal (paste red output). Linux apply is tested for real in a temp dir; Windows/macOS plans only by value.

## Done when

Everything above, `docs/architecture/update.md`, and the unverified list (Windows swap helper at runtime, macOS bundle replace, real GitHub API rate limits, provenance/attestation verification is NOT done here: say so).
