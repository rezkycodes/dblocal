//! Immutable value types for the update flow.

use std::fmt;
use std::path::PathBuf;

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::platform::{Arch, Os};

/// Release channel policy for update checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum ReleaseChannel {
    /// Stable releases only; prereleases are ignored.
    #[default]
    Stable,
    /// Accept prereleases when they are the newest semver.
    Preview,
}

/// Result of checking for updates.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum UpdateCheck {
    /// Current version is the newest applicable release.
    UpToDate {
        /// The current version that was checked.
        current: Version,
    },
    /// A newer applicable release is available.
    Available(AvailableUpdate),
}

impl UpdateCheck {
    /// Returns the available update, if any.
    pub fn available(&self) -> Option<&AvailableUpdate> {
        match self {
            Self::Available(update) => Some(update),
            Self::UpToDate { .. } => None,
        }
    }
}

/// A release that can be downloaded and applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableUpdate {
    version: Version,
    tag: String,
    release_notes: String,
    #[serde(default)]
    release_url: String,
    asset: ReleaseAsset,
    checksums_asset: ReleaseAsset,
}

impl AvailableUpdate {
    /// Creates an available update with a default release page URL derived from `tag`.
    pub fn new(
        version: Version,
        tag: impl Into<String>,
        release_notes: impl Into<String>,
        asset: ReleaseAsset,
        checksums_asset: ReleaseAsset,
    ) -> Self {
        let tag = tag.into();
        let release_url = format!(
            "https://github.com/{}/releases/tag/{}",
            crate::consts::UPDATE_REPO,
            tag
        );
        Self {
            version,
            tag,
            release_notes: release_notes.into(),
            release_url,
            asset,
            checksums_asset,
        }
    }

    /// Returns a copy with an explicit release notes page URL.
    pub fn with_release_url(mut self, url: impl Into<String>) -> Self {
        self.release_url = url.into();
        self
    }

    /// Candidate version.
    pub fn version(&self) -> &Version {
        &self.version
    }

    /// Release tag.
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// Bounded plain-text release notes.
    pub fn release_notes(&self) -> &str {
        &self.release_notes
    }

    /// Public release notes page URL on GitHub.
    pub fn release_url(&self) -> &str {
        &self.release_url
    }

    /// Platform-matching install asset.
    pub fn asset(&self) -> &ReleaseAsset {
        &self.asset
    }

    /// checksums.txt asset.
    pub fn checksums_asset(&self) -> &ReleaseAsset {
        &self.checksums_asset
    }
}

/// A release asset selected for update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseAsset {
    name: String,
    url: String,
    size_bytes: u64,
    kind: UpdateAssetKind,
}

impl ReleaseAsset {
    /// Creates a release asset.
    pub fn new(
        name: impl Into<String>,
        url: impl Into<String>,
        size_bytes: u64,
        kind: UpdateAssetKind,
    ) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
            size_bytes,
            kind,
        }
    }

    /// Asset file name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Download URL.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Declared size in bytes.
    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    /// Asset kind.
    pub fn kind(&self) -> UpdateAssetKind {
        self.kind
    }

    /// Returns a copy with a different kind.
    pub fn with_kind(&self, kind: UpdateAssetKind) -> Self {
        Self::new(self.name.clone(), self.url.clone(), self.size_bytes, kind)
    }
}

/// Supported update asset shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum UpdateAssetKind {
    /// Linux AppImage.
    AppImage,
    /// Linux tarball.
    TarGz,
    /// Debian/Ubuntu `.deb` package (manual update hint only).
    Deb,
    /// Fedora/RHEL `.rpm` package (manual update hint only).
    Rpm,
    /// Windows zip package.
    Zip,
    /// Windows MSI installer.
    Msi,
    /// macOS disk image.
    Dmg,
    /// Release checksums file.
    Checksums,
}

impl UpdateAssetKind {
    /// True when this kind is an installable application asset.
    pub fn is_installable(self) -> bool {
        !matches!(self, Self::Checksums)
    }
}

impl fmt::Display for UpdateAssetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::AppImage => "appimage",
            Self::TarGz => "tar.gz",
            Self::Deb => "deb",
            Self::Rpm => "rpm",
            Self::Zip => "zip",
            Self::Msi => "msi",
            Self::Dmg => "dmg",
            Self::Checksums => "checksums",
        };
        f.write_str(name)
    }
}

/// Progress events emitted while applying an update.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum UpdateProgress {
    /// Downloading the release asset.
    Downloading {
        /// Bytes received so far.
        received_bytes: u64,
        /// Total expected bytes, when known.
        total_bytes: Option<u64>,
    },
    /// Verifying SHA-256 (and future signature) of the downloaded asset.
    Verifying,
    /// Extracting, smoke-checking, and atomically swapping the installation.
    Installing,
    /// The update is installed or staged and ready to restart into the new version.
    ReadyToRestart {
        /// Version that will run after restart.
        version: String,
        /// Executable path that will be launched on restart.
        restart_target: PathBuf,
    },
    /// The caller cancelled the update while downloading.
    Cancelled,
    /// The update failed with a stable `DBL-xxxx` code and user-readable message.
    Failed {
        /// Stable `DBL-xxxx` error code.
        code: String,
        /// User-readable English explanation.
        message: String,
    },
}

/// How to restart into the updated version after [`super::apply_staged_update`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RestartAction {
    /// Relaunch the updated executable directly (Linux tarball/AppImage or swapped Windows zip).
    Relaunch {
        /// Path to the updated executable.
        exe_path: PathBuf,
    },
    /// Spawn the detached helper to wait for this PID, run `msiexec /i <msi> /passive`, then relaunch.
    RunWindowsMsi {
        /// Current `dblocal.exe` binary to invoke with `--apply-update`.
        helper_exe: PathBuf,
        /// Verified `.msi` file staged on disk.
        msi_path: PathBuf,
        /// Installed executable path to relaunch after MSI completes.
        relaunch_exe: PathBuf,
    },
    /// Spawn the detached helper to wait for this PID, swap the extracted exe, then relaunch.
    RunWindowsZipHelper {
        /// Current `dblocal.exe` binary to invoke with `--apply-update`.
        helper_exe: PathBuf,
        /// Verified extracted `dblocal.exe` staged on disk.
        staged_exe: PathBuf,
        /// Target `dblocal.exe` path to replace.
        target_exe: PathBuf,
        /// Expected version for post-swap verification.
        version: Version,
    },
}

impl RestartAction {
    /// The executable path that will run after restart completes.
    pub fn restart_target(&self) -> &std::path::Path {
        match self {
            Self::Relaunch { exe_path } => exe_path,
            Self::RunWindowsMsi { relaunch_exe, .. } => relaunch_exe,
            Self::RunWindowsZipHelper { target_exe, .. } => target_exe,
        }
    }
}

/// Maps platform to expected asset naming tokens.
pub fn platform_asset_tokens(os: Os, arch: Arch) -> Option<&'static [&'static str]> {
    match (os, arch) {
        (Os::Linux, Arch::X86_64) => Some(&["linux-x86_64", "linux-amd64", "_amd64.", ".x86_64."]),
        (Os::Linux, Arch::Aarch64) => {
            Some(&["linux-aarch64", "linux-arm64", "_arm64.", ".aarch64."])
        }
        (Os::Macos, _) => Some(&["macos-universal", "darwin-universal"]),
        (Os::Windows, Arch::X86_64) => Some(&["windows-x86_64", "windows-amd64"]),
        (Os::Windows, Arch::Aarch64) => Some(&["windows-aarch64", "windows-arm64"]),
    }
}
