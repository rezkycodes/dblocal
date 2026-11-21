# Update architecture (W3-U)

Owner: W3-U. Module: `crates/core/src/update/**` (plus `pub mod update;` in
`crates/core/src/lib.rs`).

This module implements the self-update logic behind Settings → Updates for
F-PKG-05. It has no UI and no periodic timer; callers decide when to check and
when to apply.

## Scope

- Check GitHub Releases for a newer version (`check`).
- Select the platform asset and the `checksums.txt` asset.
- Detect install kind and decide whether self-update is allowed.
- Download checksums + asset, verify SHA-256 from `checksums.txt` before
  install, stage asset under a fixed file name (`download_update`).
- Produce a pure install plan (`plan_install`) and apply Linux
  AppImage/portable-tarball replacements (`apply`).
- Build Windows swap-helper command line arguments for the CLI-owned
  `--apply-update` mode. The helper runtime wiring is not owned here.

## Public API

| Item | Purpose |
|---|---|
| `check(&Version, &dyn UpdateFetcher, &UpdateCheckOptions) -> UpdateResult<UpdateCheck>` | Reads GitHub Releases JSON, ignores drafts, ignores prereleases on the stable channel (including semver prerelease versions), picks the newest semver greater than current unless `allow_downgrade` is set. |
| `UpdateCheck::{UpToDate, Available}` | Distinct no-update vs available outcome. |
| `detect_install_kind()` / `detect_install_kind_with` | Detects AppImage, portable tarball, Windows zip/msi, macOS app, dist package (dpkg/rpm via trait), Homebrew, or unknown. |
| `PackageOwnership` | Test seam for package-manager ownership. |
| `download_update(&AvailableUpdate, &Path, &UpdateDownloadOptions) -> UpdateResult<StagedUpdate>` | Fetches `checksums.txt` (capped at `MAX_CHECKSUMS_BODY_BYTES`), parses it, finds the selected asset digest, downloads asset to a fixed staging file, verifies SHA-256. |
| `download_and_verify` | Low-level HTTPS-only asset download + SHA-256 verification. |
| `redirect_allowed` | Pure URL/redirect policy check used by download client redirect hooks and tests. |
| `plan_install` | Pure install plan. Resolves the real target path; refuses unsupported/managed/unknown kinds. |
| `apply` | Executes Linux AppImage/portable-tarball plan steps (one source of truth). Windows/macOS are plan-only/refused at apply time in this module. |
| `smoke_check` | Runs `--version` with a 10s timeout and requires the expected version token. |
| `cleanup_backup` | Removes leftover `.old` backup after a successful start (CLI/GUI wiring). |
| `windows_helper_command` | Builds `dblocal --apply-update ...` args (CLI wires the mode). |

## Update check

`UpdateFetcher` is an object-safe trait returning boxed futures; production uses
`ReqwestUpdateFetcher` (HTTPS-only, timeout, GitHub API accept header, redirect
policy restricted to `api.github.com` for the check client). The response body
is streamed and capped at `MAX_RELEASES_BODY_BYTES`; oversize bodies return a
network/malformed-style failure, not an unbounded buffer. Tests inject a mock
fetcher. There is no `Default` impl for the production fetcher.

Rules:

- Draft releases are ignored.
- Prereleases are ignored on `ReleaseChannel::Stable`; accepted on `Preview`.
- Stable also rejects semver prerelease versions (`1.2.3-rc.1`) even when
  GitHub `prerelease` is false.
- The selected version must be greater than `current`, unless
  `allow_downgrade=true`.
- Releases with unparseable versions are skipped (not fatal for the whole check).
- A release is selectable only when it has a platform-matching install asset and
  a `checksums.txt`/`sha256sums.txt` asset.
- Release notes are sanitized to plain text and bounded to 8 KiB.
- Malformed JSON, oversize body, and invalid repo format return
  `UpdateError::Malformed` (`DBL-8002`).
- HTTP 403/429 rate-limit responses return `UpdateError::RateLimited`
  (`DBL-8003`).
- Other transport/status failures return `UpdateError::Network` (`DBL-8004`).

Asset selection prefers AppImage over tarball on Linux, zip over msi on
Windows, dmg over tarball on macOS. Names are validated to reject path
traversal/control characters.

## Download and verification

`download_update` is the production path. It validates URLs before any request,
downloads checksums to a fixed staging name, parses and validates the checksum
entry for the selected asset, downloads the asset to a fixed staging name
(`dblocal-update.bin`), and verifies SHA-256 before returning.

`download_and_verify` remains available as the low-level primitive used by
`download_update` and tests.

- HTTPS required in production. Plain HTTP is allowed only by the test-only
  `loopback_for_tests*` constructors; `allow_loopback_http` is not a public
  mutable field.
- Host allowlist: `github.com`, `objects.githubusercontent.com`,
  `github-releases.githubusercontent.com`, `release-assets.githubusercontent.com`.
- `redirect_allowed` is the single pure policy function used by URL validation
  and redirect hooks. Redirect targets are followed only when the target passes
  the same scheme/host rules. Max redirect count is 5.
- Declared size must be > 0 and <= hard cap (65 MiB). A 1 MiB slack over
  declared size is allowed, bounded by hard cap. Checksums are additionally
  capped at `MAX_CHECKSUMS_BODY_BYTES`.
- Response `Content-Length`/`Content-Range` declared totals are checked before
  writing body bytes; streamed body bytes are also capped while reading.
  Oversize deletes the partial file and returns `UpdateError::Network`.
- `Accept-Encoding: identity` is sent.

`Checksums::parse` accepts `<sha256>  <filename>` lines, ignores blank/comment
lines, rejects invalid hashes, extra fields, unsafe names (`..`, `/`, `\`),
and duplicate entries for the same name. Hashes are normalized to lowercase
before comparison. `verify_sha256_file` compares digests in constant time and
deletes the file on mismatch (`DBL-8006`).

## Install kind

`detect_install_kind_with` is pure/testable and takes a `PackageOwnership`
trait. Production `SystemPackageOwnership` checks env/path markers and Linux
package ownership via `dpkg-query -S` / `rpm -qf` (command result only, no
shell).

Rules:

- AppImage is detected only when `APPIMAGE` names an existing regular file,
  `APPDIR` is set, and the executable path resolves inside `APPDIR`. A foreign
  `$APPIMAGE` in a child process is not trusted.
- Linux paths in deny-list locations are `Unknown` (refused): `/nix/store`,
  `/snap`, `/app`, `/usr`, `/opt`, `*/.cargo/bin`, `*/target/debug`,
  `*/target/release`. Linux executables must also be writable by the current
  user and look like a portable `dblocal` binary; otherwise `Unknown`.
- Windows MSI is detected for `\Program Files`, `\Program Files (x86)`, and
  `\AppData\Local\DBLocal` (per-user MSI install location from
  `packaging/msi/dblocal.wxs`). A zip unpacked there is classified MSI/refused,
  which is the safe direction.
- macOS `.app` bundle ancestry is recognised as `MacApp`; other macOS paths are
  `Unknown`.
- DistPackage and Homebrew are refused with package-manager hints.

Self-update allowed: AppImage, PortableTarball, WindowsZip, MacApp.
Self-update refused: WindowsMsi, DistPackage, Homebrew, Unknown (`DBL-8008`).

## Install plan / apply

`plan_install` is pure. It resolves the real target path (AppImage uses
`appimage_path`; symlinked executables resolve to the real file) and refuses
when the target directory is not writable.

Linux plans are the single source of truth for apply: write staged temp next to
target, fsync, create backup as a hard link (copy fallback), atomically rename
temp over target, keep `.old` backup. `apply` executes the plan steps in order.
Windows zip plans a detached helper command. macOS plans bundle replacement and
is not executed.

`apply` implements Linux AppImage/portable-tarball replacement:

1. Ensure target directory is writable.
2. Copy staged file next to target; copy Unix mode from target/src fallback.
3. fsync staged temp file.
4. Create backup by hard link, or copy fallback when hard link is unsupported.
5. Atomically `rename(temp, target)` over the live file. No delete-then-rename
   window for restore.
6. Run smoke check (`--version`, 10s timeout, version token match). On smoke
   failure, restore backup and return `DBL-8010`.

`restore_backup` renames a temp copy over the target (no delete-first window).
`cleanup_backup` removes the `.old` backup and is intended for "next successful
start" wiring owned by CLI/GUI.

Windows and macOS apply return plan-only/apply-failed errors; callers must use
the plan. The Windows helper command shape is tested but the helper process is
not executed here.

## Error codes

| Code | Meaning |
|---|---|
| `DBL-8001` | No newer release / up-to-date sentinel code (reserved). |
| `DBL-8002` | Malformed release metadata / invalid repo / bad version. |
| `DBL-8003` | GitHub API rate limited. |
| `DBL-8004` | Network failure / disallowed URL / bad status. |
| `DBL-8005` | Checksums metadata missing or invalid. |
| `DBL-8006` | SHA-256 verification failed; download deleted. |
| `DBL-8007` | Downgrade refused. |
| `DBL-8008` | Install kind cannot self-update. |
| `DBL-8009` | No matching asset for platform. |
| `DBL-8010` | Apply/smoke-check failure; rollback attempted. |

## Security invariants

- HTTPS-only for production download/check; no insecure transport.
- Host allowlist for update downloads; redirect targets validated by one pure
  policy function.
- No path traversal from asset names (`..`, `/`, `\`, control chars rejected).
- Staged asset name is fixed; release JSON never supplies filesystem paths.
- Size bounded by declared metadata and hard cap; stream stops at cap.
- SHA-256 from `checksums.txt` compared in constant time before install.
- No secrets in errors/log messages; no password handling in this module.
- No writes outside caller-provided paths/temp siblings; tests use temp dirs.

## Not verified / out of scope

- Real GitHub API rate-limit behavior and live release parsing against the real
  repo (tests use local mock/fake HTTP servers).
- Real GitHub CDN redirect hosts beyond the explicit allowlist are not followed
  by `download_update`; production wiring must confirm release asset URLs stay
  within allowlisted hosts or extend policy deliberately.
- Windows swap helper runtime execution (`--apply-update` mode wiring is CLI
  work). Command-line construction is unit-tested.
- Windows MSI detection is path-based by value; not verified on a real Windows
  MSI install in this environment.
- macOS bundle replacement execution. Plan shape is unit-tested only.
- Provenance/attestation verification (GitHub artifact attestations) is not
  implemented here; checksums.txt SHA-256 is the verification gate.
- End-to-end update through real package-manager installs (dpkg/rpm/Homebrew) is
  not executed; detection is trait-tested and command-probe logic is exercised by
  unit paths where possible.
