//! Why an engine failed to start, as far as the engine can tell.

use std::str::FromStr;

use crate::DblError;

/// A start failure the UI can turn into an actionable hint (PRD `F-LOG-02`).
///
/// Adapters classify the raw engine output; the GUI maps each variant to a
/// specific action such as "Pindah port" or "Pasang ulang runtime".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum StartFailure {
    /// The listening port was already bound by another process.
    PortInUse,
    /// The data directory was created by a different engine version.
    DataDirVersionMismatch,
    /// A path could not be read or written because of its permissions.
    PermissionDenied,
    /// An engine binary is missing or not executable.
    BinaryMissing,
    /// The output matched nothing the adapter knows about.
    #[default]
    Unknown,
}

impl StartFailure {
    /// Stable identifier used in events, logs and the CLI `--json` output.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PortInUse => "port_in_use",
            Self::DataDirVersionMismatch => "data_dir_version_mismatch",
            Self::PermissionDenied => "permission_denied",
            Self::BinaryMissing => "binary_missing",
            Self::Unknown => "unknown",
        }
    }

    /// Whether the UI can offer a one-click fix.
    pub const fn has_suggested_action(self) -> bool {
        matches!(
            self,
            Self::PortInUse | Self::DataDirVersionMismatch | Self::BinaryMissing
        )
    }
}

impl FromStr for StartFailure {
    type Err = DblError;

    fn from_str(value: &str) -> Result<Self, DblError> {
        match value {
            "port_in_use" => Ok(Self::PortInUse),
            "data_dir_version_mismatch" => Ok(Self::DataDirVersionMismatch),
            "permission_denied" => Ok(Self::PermissionDenied),
            "binary_missing" => Ok(Self::BinaryMissing),
            "unknown" => Ok(Self::Unknown),
            other => Err(DblError::InvalidInput {
                field: "start_failure",
                reason: format!("unknown start failure: {other}"),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [StartFailure; 5] = [
        StartFailure::PortInUse,
        StartFailure::DataDirVersionMismatch,
        StartFailure::PermissionDenied,
        StartFailure::BinaryMissing,
        StartFailure::Unknown,
    ];

    #[test]
    fn every_variant_round_trips_and_is_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for failure in ALL {
            let tag = failure.as_str();
            assert!(seen.insert(tag), "duplicate tag {tag}");
            assert_eq!(tag.parse::<StartFailure>().ok(), Some(failure));
            assert!(!tag.contains(' '), "tags stay machine readable: {tag}");
        }
        assert!("port-in-use".parse::<StartFailure>().is_err());
    }

    #[test]
    fn only_actionable_variants_offer_a_fix() {
        assert!(StartFailure::PortInUse.has_suggested_action());
        assert!(StartFailure::DataDirVersionMismatch.has_suggested_action());
        assert!(StartFailure::BinaryMissing.has_suggested_action());
        assert!(!StartFailure::PermissionDenied.has_suggested_action());
        assert!(!StartFailure::Unknown.has_suggested_action());
    }

    #[test]
    fn default_is_unknown() {
        assert_eq!(StartFailure::default(), StartFailure::Unknown);
    }
}
