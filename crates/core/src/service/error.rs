//! The user-facing shape of a failure, shared by events and read models.
//!
//! [`UserFacingError`] carries exactly what the GUI needs to render an error
//! panel: the stable searchable code, the friendly sentence, and, when the
//! failure was a start, the engine-specific diagnosis that enables a fix button.

use crate::DblError;
use crate::model::StartFailure;

/// A failure rendered for a person, never for a developer log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserFacingError {
    /// Stable code shaped `DBL-NNNN`, as returned by [`DblError::code`].
    pub code: String,
    /// Friendly message from the `strings` module, safe to copy into an issue.
    pub message: String,
    /// Engine-specific start diagnosis, when the failure came from a start.
    pub diagnosis: Option<StartFailure>,
}

impl UserFacingError {
    /// Builds an error without a diagnosis.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            diagnosis: None,
        }
    }

    /// Returns a copy carrying a start diagnosis.
    pub fn with_diagnosis(mut self, diagnosis: StartFailure) -> Self {
        self.diagnosis = Some(diagnosis);
        self
    }
}

impl From<&DblError> for UserFacingError {
    fn from(err: &DblError) -> Self {
        Self {
            code: err.code().to_owned(),
            message: err.user_message(),
            diagnosis: None,
        }
    }
}

impl From<DblError> for UserFacingError {
    fn from(err: DblError) -> Self {
        Self::from(&err)
    }
}

impl std::fmt::Display for UserFacingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{
        CODE_ENGINE, CODE_INVALID_INPUT, CODE_PORT_IN_USE, CODE_SECRET,
    };

    #[test]
    fn conversion_keeps_code_and_message() {
        let err = DblError::PortInUse { port: 5432 };
        let facing = UserFacingError::from(&err);
        assert_eq!(facing.code, CODE_PORT_IN_USE);
        assert_eq!(facing.message, err.user_message());
        assert!(facing.diagnosis.is_none());
        assert_eq!(facing.to_string(), format!("[{}] {}", err.code(), err.user_message()));
    }

    #[test]
    fn conversion_never_leaks_a_payload() {
        let err = DblError::Secret {
            reason: "keychain locked".to_owned(),
        };
        let facing = UserFacingError::from(err);
        assert_eq!(facing.code, CODE_SECRET);
        assert!(!facing.message.contains("keychain"));
        assert!(!facing.message.is_empty());
    }

    #[test]
    fn diagnosis_is_attached_explicitly() {
        let facing = UserFacingError::from(&DblError::Engine {
            reason: "boom".into(),
        })
        .with_diagnosis(StartFailure::DataDirVersionMismatch);
        assert_eq!(facing.code, CODE_ENGINE);
        assert_eq!(facing.diagnosis, Some(StartFailure::DataDirVersionMismatch));
    }

    #[test]
    fn direct_construction_matches_error_shape() {
        let facing = UserFacingError::new(CODE_INVALID_INPUT, "bad input");
        assert_eq!(facing.code, CODE_INVALID_INPUT);
        assert_eq!(facing.message, "bad input");
        assert!(facing.diagnosis.is_none());
    }
}
