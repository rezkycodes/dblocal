//! Stub of `crate::service::UserFacingError`, the only service item `admin/`
//! names (`impl From<AdminError> for UserFacingError`).
#[derive(Debug, Clone)]
pub struct UserFacingError {
    pub code: String,
    pub message: String,
}

impl UserFacingError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}
