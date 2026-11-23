//! Stub: only `generate_password` is reachable from `admin/mysql.rs`.
use crate::secret::SecretString;

pub fn generate_password() -> crate::error::Result<SecretString> {
    Ok(SecretString::new("probe-generated-password"))
}
