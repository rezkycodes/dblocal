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
- Download and verify the selected asset (SHA-256 from `checksums.txt`,
  constant-time comparison, before any install step).
- Produce a pure install plan (`plan_install`) and apply Linux
  AppImage/portable-tarball replacements (`apply`).
- Build Windows swap-helper command line arguments for the CLI-owned
  `--apply-update` mode. The helper runtime wiring is not owned here.

## Public API

| Item | Purpose |
|---|---|
| `check(&Version, &dyn UpdateFetcher, &UpdateCheckOptions) -> UpdateResult<UpdateCheck>` | Reads GitHub Releases JSON, ignores drafts, ignores prereleases on the stable channel, picks the newest semver greater than current unless `allow_downgrade` is set. |
| `UpdateCheck::{UpToDate, Available}` | Distinct no-update vs available outcome. |
| `detect_install_kind()` / `detect_install_kind_with` | Detects AppImage, portable tarball, Windows zip/msi, macOS app, dist package (dpkg/rpm via trait), Homebrew, or unknown. |
| `PackageOwnership` | Test seam for package-manager ownership. |
| `download_and_verify` | HTTPS-only, host allowlist, declared-size/hard-cap guard, SHA-256 verification before install. |
| `plan_install` | Pure install plan. Refuses unsupported/managed kinds. |
| `apply` | Executes Linux AppImage/portable-tarball plan. Windows/macOS are plan-only/refused at apply time in this module. |
| `smoke_check` | Runs `--version` and requires the expected version string. |
| `windows_helper_command` | Builds `dblocal --apply-update ...` args (CLI wires the mode). |

## Update check

`UpdateFetcher` is an object-safe trait returning boxed futures; production uses
`ReqwestUpdateFetcher` (HTTPS-only, timeout, GitHub API accept header, redirect
policy restricted to `api.github.com` for the check client). Tests inject a mock
fetcher.

Rules:

- Draft releases are ignored.
- Prereleases are ignored on `ReleaseChannel::Stable`; accepted on `Preview`.
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

`download_and_verify` validates the URL before any request:

- HTTPS required in production. Plain HTTP is only allowed by the explicit
  test-only option for loopback.
- Host allowlist: `github.com`, `objects.githubusercontent.com`,
  `github-releases.githubusercontent.com`, `release-assets.githubusercontent.com`.
- Redirects are followed only when the target passes the same scheme/host rules.
- Declared size must be > 0 and <= hard cap (65 MiB). A 1 MiB slack over
  declared size is allowed, bounded by hard cap.
- Response body bytes are capped while streaming; oversize deletes the partial
  file and returns `UpdateError::Network`.
- `Accept-Encoding: identity` is sent.

`Checksums::parse` accepts `<sha64>  <filename>` lines, ignores blank/comment
lines, rejects invalid hashes, extra fields, and unsafe names (`..`, `/`, `\`).
`verify_sha256_file` compares digests in constant time and deletes the file on
mismatch (`DBL-8006`).

## Install kind

`detect_install_kind_with` is pure/testable and takes a `PackageOwnership`
trait. Production `SystemPackageOwnership` checks `APPIMAGE`, Homebrew path
markers, macOS `.app` bundle ancestry, Windows Program Files hint for MSI, and
Linux package ownership via `dpkg-query -S` / `rpm -qf` (command result only, no
shell).

Self-update allowed: AppImage, PortableTarball, WindowsZip, MacApp.
Self-update refused: WindowsMsi, DistPackage, Homebrew, Unknown. Refusal carries
a package-manager hint where applicable (`DBL-8008`).

## Install plan / apply

`plan_install` is pure. Linux plans write staged file next to target, fsync,
rename over target, and keep `.old` backup. Windows zip plans a detached helper
command. macOS plans bundle replacement and is not executed.

`apply` implements Linux AppImage/portable-tarball replacement in a temp dir /
same filesystem:

1. Copy staged file next to current binary; copy Unix mode from current.
2. fsync staged temp file.
3. Rename current to `.old` backup; rename temp over current; on rename failure
   restore backup.
4. Run smoke check (`--version` reports expected version). On smoke failure,
   restore backup and return `DBL-8010`.

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
- Host allowlist for update downloads; redirect targets validated.
- No path traversal from asset names (`..`, `/`, `\`, control chars rejected).
- Size bounded by declared metadata and hard cap; stream stops at cap.
- SHA-256 from `checksums.txt` compared in constant time before install.
- No secrets in errors/log messages; no password handling in this module.
- No writes outside caller-provided paths/temp siblings; tests use temp dirs.

## Not verified / out of scope

- Real GitHub API rate-limit behavior and live release parsing against the real
  repo (tests use local mock/fake HTTP servers).
- Windows swap helper runtime execution (`--apply-update` mode wiring is CLI
  work). Command-line construction is unit-tested.
- macOS bundle replacement execution. Plan shape is unit-tested only.
- Provenance/attestation verification (GitHub artifact attestations) is not
  implemented here; checksums.txt SHA-256 is the verification gate.
- End-to-end update through real package-manager installs (dpkg/rpm/Homebrew) is
  not executed; detection is trait-tested and command-probe logic is exercised by
  unit paths where possible.
