//! Domain types: engines, instance identity, lifecycle state and entities.
//!
//! Every type here is an immutable value type. Fields are private; updates
//! return a new value through `with_*` methods.

mod artifact;
mod catalog;
mod entity;
mod failure;
mod runtime;
mod scalar;
mod state;

pub use artifact::{ArchiveFormat, Artifact};
pub use catalog::{CatalogSnapshot, CatalogStatus, EngineVersion, VersionStatus, version_line};
pub use entity::{
    ArchivedData, CredentialBackend, CredentialRef, DeleteMode, Instance, InstanceSpec,
    PortAllocation,
};
pub use failure::StartFailure;
pub use runtime::{DownloadProgress, Runtime, RuntimeState};
pub use scalar::{AuthMode, EngineKey, Family, InstanceId, InstanceName, Port};
pub use state::{InstanceEvent, InstanceStatus};

/// Validates a version tag such as `18.7`, `8.0.40` or `7.4.1`.
///
/// Only ASCII alphanumerics, dots, dashes and underscores are accepted, and the
/// tag must start with a digit, which keeps it safe to use as a directory
/// component.
pub(crate) fn validate_version_tag(field: &'static str, value: &str) -> crate::Result<()> {
    let valid = !value.is_empty()
        && value.len() <= 32
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        && value.starts_with(|c: char| c.is_ascii_digit());
    if valid {
        Ok(())
    } else {
        Err(crate::DblError::InvalidInput {
            field,
            reason: "version must start with a digit and contain only [A-Za-z0-9._-]".to_owned(),
        })
    }
}

/// Derives the major line (`18` from `18.7.0`) of a version tag.
pub(crate) fn major_line_of(value: &str) -> String {
    value.split('.').next().unwrap_or(value).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_tag_rules() {
        for ok in ["18", "18.7", "18.7.0", "8.0.40", "7.4.1-rc1"] {
            assert!(validate_version_tag("version", ok).is_ok(), "{ok}");
        }
        for bad in ["", "..", "v18", "18/../x", "18 7"] {
            assert!(validate_version_tag("version", bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn major_line_is_first_component() {
        assert_eq!(major_line_of("18.7.0"), "18");
        assert_eq!(major_line_of("18"), "18");
    }
}
