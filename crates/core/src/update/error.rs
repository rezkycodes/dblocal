//! Typed update outcomes with stable DBL codes and Indonesian messages.

use std::fmt;

use crate::DblError;

/// Stable code: update check found no newer release.
pub const CODE_UPDATE_UP_TO_DATE: &str = "DBL-8001";
/// Stable code: release metadata was malformed or incomplete.
pub const CODE_UPDATE_MALFORMED: &str = "DBL-8002";
/// Stable code: GitHub API rate limit was hit.
pub const CODE_UPDATE_RATE_LIMITED: &str = "DBL-8003";
/// Stable code: update network request failed.
pub const CODE_UPDATE_NETWORK: &str = "DBL-8004";
/// Stable code: checksum file or digest missing/invalid.
pub const CODE_UPDATE_CHECKSUM: &str = "DBL-8005";
/// Stable code: downloaded artifact failed verification.
pub const CODE_UPDATE_VERIFY_FAILED: &str = "DBL-8006";
/// Stable code: downgrade refused.
pub const CODE_UPDATE_DOWNGRADE: &str = "DBL-8007";
/// Stable code: install kind cannot self-update.
pub const CODE_UPDATE_UNSUPPORTED_KIND: &str = "DBL-8008";
/// Stable code: update asset missing for platform.
pub const CODE_UPDATE_ASSET_MISSING: &str = "DBL-8009";
/// Stable code: update apply/install failed.
pub const CODE_UPDATE_APPLY_FAILED: &str = "DBL-8010";

/// Result alias for update operations.
pub type UpdateResult<T> = std::result::Result<T, UpdateError>;

/// Distinct update outcomes. Every variant has a stable DBL code and an
/// Indonesian user-facing message. Payloads are diagnostic details for logs and
/// must never contain secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum UpdateError {
    /// Release metadata could not be parsed or was invalid.
    Malformed { reason: String },
    /// GitHub API rate limit was encountered.
    RateLimited { reason: String },
    /// Network request failed.
    Network { reason: String },
    /// Checksums metadata missing or invalid.
    ChecksumMetadata { reason: String },
    /// Downloaded file digest did not match checksums.txt.
    VerifyFailed { reason: String },
    /// Candidate is older than or equal to current and downgrade not allowed.
    DowngradeRefused { reason: String },
    /// Install kind is managed by a package manager or unknown.
    UnsupportedInstallKind { reason: String },
    /// No matching asset for this OS/arch in the release.
    AssetMissing { reason: String },
    /// Applying an update plan failed.
    ApplyFailed { reason: String },
}

impl UpdateError {
    /// Stable searchable error code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Malformed { .. } => CODE_UPDATE_MALFORMED,
            Self::RateLimited { .. } => CODE_UPDATE_RATE_LIMITED,
            Self::Network { .. } => CODE_UPDATE_NETWORK,
            Self::ChecksumMetadata { .. } => CODE_UPDATE_CHECKSUM,
            Self::VerifyFailed { .. } => CODE_UPDATE_VERIFY_FAILED,
            Self::DowngradeRefused { .. } => CODE_UPDATE_DOWNGRADE,
            Self::UnsupportedInstallKind { .. } => CODE_UPDATE_UNSUPPORTED_KIND,
            Self::AssetMissing { .. } => CODE_UPDATE_ASSET_MISSING,
            Self::ApplyFailed { .. } => CODE_UPDATE_APPLY_FAILED,
        }
    }

    /// Indonesian user-facing message. Safe to display; contains no secrets.
    pub fn user_message(&self) -> String {
        match self {
            Self::Malformed { .. } => {
                "Data rilis pembaruan tidak valid. Coba lagi nanti.".to_owned()
            }
            Self::RateLimited { .. } => {
                "Batas permintaan pemeriksaan pembaruan tercapai. Coba lagi nanti.".to_owned()
            }
            Self::Network { .. } => {
                "Gagal memeriksa pembaruan. Periksa koneksi lalu coba lagi.".to_owned()
            }
            Self::ChecksumMetadata { .. } => {
                "File checksum pembaruan tidak ditemukan atau tidak valid.".to_owned()
            }
            Self::VerifyFailed { .. } => {
                "Checksum pembaruan tidak cocok. File unduhan dihapus demi keamanan.".to_owned()
            }
            Self::DowngradeRefused { .. } => {
                "Versi pembaruan tidak lebih baru dari versi saat ini.".to_owned()
            }
            Self::UnsupportedInstallKind { .. } => {
                "Pembaruan otomatis tidak tersedia untuk cara pemasangan ini.".to_owned()
            }
            Self::AssetMissing { .. } => "Pembaruan tidak tersedia untuk platform ini.".to_owned(),
            Self::ApplyFailed { .. } => {
                "Gagal memasang pembaruan. Versi sebelumnya dipulihkan bila memungkinkan."
                    .to_owned()
            }
        }
    }

    /// Renders `[DBL-xxxx] message`.
    pub fn diagnostic(&self) -> String {
        format!("[{}] {}", self.code(), self.user_message())
    }

    /// Diagnostic detail for logs; does not include secrets.
    pub fn detail(&self) -> &str {
        match self {
            Self::Malformed { reason }
            | Self::RateLimited { reason }
            | Self::Network { reason }
            | Self::ChecksumMetadata { reason }
            | Self::VerifyFailed { reason }
            | Self::DowngradeRefused { reason }
            | Self::UnsupportedInstallKind { reason }
            | Self::AssetMissing { reason }
            | Self::ApplyFailed { reason } => reason,
        }
    }
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code(), self.detail())
    }
}

impl std::error::Error for UpdateError {}

impl From<UpdateError> for DblError {
    fn from(err: UpdateError) -> Self {
        match &err {
            UpdateError::Malformed { reason } => DblError::InvalidInput {
                field: "update_release",
                reason: reason.clone(),
            },
            UpdateError::RateLimited { reason } | UpdateError::Network { reason } => {
                DblError::Network {
                    reason: reason.clone(),
                }
            }
            UpdateError::ChecksumMetadata { reason } | UpdateError::VerifyFailed { reason } => {
                DblError::ChecksumMismatch {
                    artifact: reason.clone(),
                }
            }
            UpdateError::DowngradeRefused { reason }
            | UpdateError::UnsupportedInstallKind { reason } => DblError::Unsupported {
                reason: reason.clone(),
            },
            UpdateError::AssetMissing { reason } => DblError::NotFound {
                kind: "update_asset",
                name: reason.clone(),
            },
            UpdateError::ApplyFailed { reason } => DblError::Io {
                source: std::io::Error::other(reason.clone()),
            },
        }
    }
}
