//! Immutable value types for the update flow.

use std::fmt;

use semver::Version;

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableUpdate {
    version: Version,
    tag: String,
    release_notes: String,
    asset: ReleaseAsset,
    checksums_asset: ReleaseAsset,
}

impl AvailableUpdate {
    /// Creates an available update.
    pub fn new(
        version: Version,
        tag: impl Into<String>,
        release_notes: impl Into<String>,
        asset: ReleaseAsset,
        checksums_asset: ReleaseAsset,
    ) -> Self {
        Self {
            version,
            tag: tag.into(),
            release_notes: release_notes.into(),
            asset,
            checksums_asset,
        }
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum UpdateAssetKind {
    /// Linux AppImage.
    AppImage,
    /// Linux tarball.
    TarGz,
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
            Self::Zip => "zip",
            Self::Msi => "msi",
            Self::Dmg => "dmg",
            Self::Checksums => "checksums",
        };
        f.write_str(name)
    }
}

/// Maps platform to expected asset naming tokens.
pub fn platform_asset_tokens(os: Os, arch: Arch) -> Option<&'static [&'static str]> {
    match (os, arch) {
        (Os::Linux, Arch::X86_64) => Some(&["linux-x86_64", "linux-amd64"]),
        (Os::Linux, Arch::Aarch64) => Some(&["linux-aarch64", "linux-arm64"]),
        (Os::Macos, _) => Some(&["macos-universal", "darwin-universal"]),
        (Os::Windows, Arch::X86_64) => Some(&["windows-x86_64", "windows-amd64"]),
        (Os::Windows, Arch::Aarch64) => Some(&["windows-aarch64", "windows-arm64"]),
    }
}
