//! Stub of the error module: only the `Result` alias is reachable here.
pub type Result<T> = std::result::Result<T, ProbeError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeError;

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("probe error")
    }
}
