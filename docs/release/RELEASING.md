# Releasing DBLocal

This document describes how a DBLocal release is cut, every secret it needs, how to verify what
shipped, and the Phase 2 packaging plan. It matches `.github/workflows/release.yml`.

> **Status:** the release workflow is written but has **not** been executed on GitHub Actions yet.
> Build times, artifact names produced by the toolchain, and the macOS signing/notarization steps
> are the parts most likely to need adjustment on the first real tag. Treat a first run as a
> rehearsal (see "Dry run" below).

---

## 1. What a release produces (Phase 1)

| Target | Artifacts |
|---|---|
| `linux-x86_64` | `.AppImage`, `.tar.gz` |
| `linux-aarch64` | `.AppImage`, `.tar.gz` |
| `macos-universal` | `.dmg` (signed + notarized when Apple secrets are present, otherwise unsigned with a loud warning), `.tar.gz` |
| `windows-x86_64` | `.zip` (unsigned in v1) |

Plus:

- **`checksums.txt`** — SHA-256 for every artifact, generated in the `release` job.
- **GitHub build provenance attestations** via `actions/attest-build-provenance`.
- GitHub-generated release notes (`generate_release_notes: true`).

`.deb`, `.rpm` and `.msi` are **not** built in Phase 1 — see §7.

---

## 2. Trigger

The release workflow runs on a **tag push** matching `v*`:

```sh
git tag -a v0.1.0 -m "DBLocal 0.1.0"
git push origin v0.1.0
```

Only tags. There is no manual button and no release-from-branch path — every artifact is built by CI
from the tagged commit, never on a developer machine (F-PKG business rule).

---

## 3. Required secrets

Configure these under **Settings → Secrets and variables → Actions**.

### macOS signing and notarization (D-20, OPEN-04)

| Secret | Purpose |
|---|---|
| `APPLE_CERTIFICATE_P12_BASE64` | Base64 of your **Developer ID Application** certificate exported as `.p12`. |
| `APPLE_CERTIFICATE_PASSWORD` | Password for that `.p12`. |
| `APPLE_SIGNING_IDENTITY` | Full identity string, e.g. `Developer ID Application: Your Name (TEAMID)`. |
| `APPLE_TEAM_ID` | Your 10-character Apple Developer Team ID. |
| `APPLE_NOTARIZE_KEY_ID` | App Store Connect API key ID. |
| `APPLE_NOTARIZE_ISSUER_ID` | App Store Connect API issuer UUID. |
| `APPLE_NOTARIZE_KEY_P8_BASE64` | Base64 of the `AuthKey_<KEYID>.p8` private key. |

**Behavior when secrets are absent:** the macOS job still builds the `.dmg` but prints a loud
`::warning` and skips signing/notarization — you get an **unsigned** `.dmg`. This is deliberate
(D-20): a missing-secret run must never silently produce a "signed" release. When the secrets are
present the job:

1. imports the certificate into a temporary keychain,
2. signs the binary and the `.app` bundle with `codesign --options runtime --timestamp`,
3. rebuilds and signs the `.dmg`,
4. submits to `notarytool --wait`, then `stapler staple` and `stapler validate`,
5. runs `spctl -a -vv` as a sanity check.

`GITHUB_TOKEN` is provided automatically; no manual secret is needed for it.

> **OPEN-04:** who owns the Apple Developer account and pays the annual fee, and how the signing
> secrets are stored, is still an open question in the PRD. Until it is answered, macOS releases are
> unsigned.

### Windows

No secrets. Windows is **unsigned in v1**; users see a SmartScreen prompt. See
[`docs/install/windows-smartscreen.md`](../install/windows-smartscreen.md).

---

## 4. Size gate

`F-PKG-01 AC2`: the `release` job fails if **any** artifact is over **60 MB** (60 × 1024 × 1024
bytes). The check runs before checksums are written, so an oversized artifact fails the release
instead of shipping quietly. 60 MB is the PRD ceiling, raised from an earlier 25 MB because GPUI
requires Vulkan/GPU libraries.

---

## 5. Verifying a release

### Checksums

```sh
# Linux
sha256sum -c checksums.txt --ignore-missing

# macOS (no coreutils)
shasum -a 256 -c checksums.txt

# Windows PowerShell
Get-FileHash .\dblocal-<version>-windows-x86_64.zip -Algorithm SHA256
```

Compare against the `checksums.txt` from the **same** release.

### Build provenance

```sh
gh attestation verify <artifact> --repo rezkycodes/dblocal
```

This proves the artifact was produced by the `release.yml` workflow from this repository, not built
elsewhere and uploaded by hand.

### macOS notarization

```sh
spctl -a -vv /Volumes/DBLocal/DBLocal.app
# expect: "accepted", "source=Notarized Developer ID"
```

---

## 6. Runner label notes

Verified against GitHub's runner documentation on **2026-11-20**:

| Task asked for | What the workflow uses | Why |
|---|---|---|
| `macos-14` | `macos-15` (arm64) and `macos-15-intel` (x86_64) | `macos-14` was **fully retired on 2026-11-02** (`actions/runner-images#13518`). No `macos-14` label runs on GitHub Actions anymore. |
| Intel macOS runner | `macos-15-intel` | Intel macOS labels are now `macos-15-intel` / `macos-26-intel`. |
| `ubuntu-24.04-arm` | unchanged | Available as an arm64 Linux runner. |

If project policy genuinely requires the macOS **14** SDK, that must be done by selecting an Xcode
version on a supported runner image, not by using a retired label.

Linux build dependencies come from **Zed's `script/linux`**, read at commit
[`a19e1944`](https://github.com/zed-industries/zed/blob/a19e19440d4102668830a9da579c3ee71d2fbf1a/script/linux)
(the Ubuntu/Debian branch, plus `libstdc++-14-dev` for Ubuntu 24.04). Zed's script is the source of
truth for GPUI's Linux build prerequisites.

---

## 7. AppImage and the Vulkan driver

The `.AppImage` bundles the `dblocal` binary, the desktop file, the icon and an `AppRun` shim; it is
assembled by `packaging/appimage/build-appimage.sh`. It does **not** bundle a GPU driver.

GPUI renders through **Vulkan**, so the AppImage requires a working **host** Vulkan driver:

- Mesa (`radv`, `ANV`, or `lavapipe` as a software fallback) is normally present on desktop Linux.
- On a machine without Vulkan, the app is expected to exit with an actionable message naming the
  missing driver and a docs link (F-SPK-01 AC2) — not to crash or show a black window.
- We intentionally do not ship a driver: bundling Mesa is large, fragile across distros, and against
  the "small installer" goal.

If you are testing the AppImage on a headless/CI box, install `mesa-vulkan-drivers` (or use
`lavapipe`) on the host first.

---

## 8. Phase 2 packaging plan (not built yet)

Phase 2 adds native packages. Each is driven from the same tagged commit as the Phase 1 artifacts.

| Package | Tool | Notes |
|---|---|---|
| `.deb` (Debian/Ubuntu) | [`cargo-deb`](https://github.com/kornelski/cargo-deb) | Metadata in `Cargo.toml` `[package.metadata.deb]`; depends on `libvulkan1`, keyring backend libs; recommends, does not require, engine binaries. |
| `.rpm` (Fedora/RHEL) | [`cargo-generate-rpm`](https://github.com/cat-in-void/cargo-generate-rpm) | Generated from the same binary and a spec-like config; `Requires: vulkan-loader`. |
| `.msi` (Windows) | [WiX Toolset](https://wixtoolset.org/) | Per-user install (no elevation), Start-menu shortcut, optional tray autostart entry. |

Uninstall rule (F-PKG-04 AC2): removing a package deletes **only** the app files. `~/.dblocal/`
(engine runtimes, instance data, archives, `state.db`) is deliberately **kept**, and the uninstall
message must say so.

Self-update (F-PKG-05, also Phase 2) is **disabled** for package-manager installs — the app must
direct the user to `apt`/`dnf`/`winget` instead of replacing files behind the package manager's
back.

---

## 9. Dry run / rehearsal checklist

Before the first real tag:

1. Push a throwaway tag such as `v0.0.0-rc1` to a fork and let `release.yml` run end to end.
2. Confirm each job's artifact names match what `packaging/install.sh` expects
   (`dblocal-<tag>-<target>.tar.gz`, `dblocal-<tag>-macos-universal.tar.gz`, etc.).
3. Confirm the size gate actually runs and passes.
4. Confirm `checksums.txt` lists every artifact and that `install.sh --dry-run` resolves the right
   asset for each target.
5. If Apple secrets are configured, confirm the `.dmg` passes `spctl -a -vv`; if not, confirm the
   unsigned path still produces a usable `.dmg` and a visible warning.
6. Delete the rehearsal tag and its release.

## 10. Source correspondence (GPLv3 §6)

Every release binary is built from the tagged commit by CI, so the tag is the corresponding source.
Do **not** build release artifacts locally and upload them by hand — that breaks the correspondence
between binary and source tag that GPLv3 §6 requires.
