//! Stub with the real shape of `crates/core/src/secret.rs` (no zeroize needed
//! for a type check; the real file is compiled in the product build).
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretString(***)")
    }
}
impl std::fmt::Display for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("***")
    }
}
impl From<String> for SecretString {
    fn from(value: String) -> Self {
        Self(value)
    }
}
impl From<&str> for SecretString {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}
