//! A string that must never leak into logs, SQLite, argv or `Debug` output.

use std::fmt;

/// Secret material that redacts itself on every formatting path.
///
/// There is deliberately no `Serialize` implementation and no `Clone`-free
/// access: callers must ask for [`SecretString::expose`] explicitly, which
/// makes every use of the raw value easy to audit.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    /// Wraps a value that is already secret.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the raw value. Every call site is an audit point.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Length in bytes, safe to log for diagnostics.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the secret is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString(***)")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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

#[cfg(test)]
mod tests {
    use super::*;

    const CANARY: &str = "hunter2-super-secret";

    #[derive(Debug)]
    struct Wrapper {
        _secret: SecretString,
    }

    #[test]
    fn debug_and_display_redact() {
        let secret = SecretString::from(CANARY);
        assert_eq!(format!("{secret:?}"), "SecretString(***)");
        assert_eq!(format!("{secret}"), "***");
        assert!(!format!("{secret:?}").contains(CANARY));
        assert!(!format!("{secret}").contains(CANARY));
    }

    #[test]
    fn nested_debug_redacts() {
        let wrapper = Wrapper {
            _secret: SecretString::from(CANARY),
        };
        let rendered = format!("{wrapper:?}");
        assert!(!rendered.contains(CANARY));
        assert!(rendered.contains("***"));
    }

    #[test]
    fn expose_returns_raw_value() {
        let secret = SecretString::from(CANARY);
        assert_eq!(secret.expose(), CANARY);
        assert_eq!(secret.len(), CANARY.len());
        assert!(!secret.is_empty());
    }
}
