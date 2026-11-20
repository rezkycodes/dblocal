//! Upstream source adapters used by `cargo xtask catalog build`.
//!
//! Every adapter answers one question per engine: "which builds exist, and what
//! is the SHA-256 of each". The HTTP transport is behind [`Source`] so the
//! fixture tests serve recorded responses from a local socket and never touch
//! the network.

mod helpers;
mod mariadb;
mod mysql;
mod postgresql;
mod valkey;

pub use helpers::{assemble, require_https};
pub use mariadb::{MariadbRelease, mariadb_from_api};
pub use mysql::{MysqlBuild, mysql_artifact};
pub use postgresql::{GithubRelease, postgresql_from_releases};
pub use valkey::valkey_from_sidecars;

pub use dblocal_core::catalog::{
    CATALOG_SCHEMA_VERSION, CatalogArtifact, CatalogDocument, CatalogVersion,
};
pub use dblocal_core::model::ArchiveFormat;
pub use dblocal_core::platform::{Arch, Os};

/// Target this wave actually publishes.
pub const WAVE_ONE_POSTGRESQL: [&str; 3] = ["18.7.0", "17.12.0", "16.16.0"];
/// Valkey version this wave publishes.
pub const WAVE_ONE_VALKEY: &str = "9.2.0";

/// Where the runtime refreshes the catalog from.
pub const CATALOG_SOURCE_URL: &str =
    "https://raw.githubusercontent.com/rezkycodes/dblocal/main/catalog/engines.json";

/// Failure of a source adapter.
#[derive(Debug)]
pub struct SourceError(String);

impl SourceError {
    /// Builds an error from any displayable message.
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SourceError {}

impl From<serde_json::Error> for SourceError {
    fn from(err: serde_json::Error) -> Self {
        Self(format!("invalid JSON from upstream: {err}"))
    }
}

/// Minimal HTTP transport, injectable for tests.
pub trait Source {
    /// Fetches `url` as text.
    fn get_text(&self, url: &str) -> Result<String, SourceError>;
    /// Fetches `url` as bytes.
    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, SourceError>;
}

/// HTTPS transport used by the real build.
#[derive(Debug, Clone)]
pub struct HttpSource {
    client: reqwest::blocking::Client,
    github_token: Option<String>,
}

impl HttpSource {
    /// Builds a client with a bounded timeout and an optional GitHub token.
    pub fn new(github_token: Option<String>) -> Result<Self, SourceError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .user_agent("dblocal-catalog-builder")
            .build()
            .map_err(|err| SourceError::new(format!("could not build HTTP client: {err}")))?;
        Ok(Self {
            client,
            github_token,
        })
    }
}

impl Source for HttpSource {
    fn get_text(&self, url: &str) -> Result<String, SourceError> {
        let bytes = self.get_bytes(url)?;
        String::from_utf8(bytes)
            .map_err(|err| SourceError::new(format!("{url} is not UTF-8: {err}")))
    }

    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, SourceError> {
        require_https(url)?;
        self.request(url)
    }
}

impl HttpSource {
    /// Performs the GET itself.
    ///
    /// Split from [`Source::get_bytes`] so the request/response handling can be
    /// exercised against an ephemeral local server without relaxing the
    /// HTTPS-only boundary.
    fn request(&self, url: &str) -> Result<Vec<u8>, SourceError> {
        let mut request = self.client.get(url);
        if url.starts_with("https://api.github.com/")
            && let Some(token) = &self.github_token
        {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .map_err(|err| SourceError::new(format!("GET {url} failed: {err}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(SourceError::new(format!(
                "GET {url} returned HTTP {}",
                status.as_u16()
            )));
        }
        response
            .bytes()
            .map(|bytes| bytes.to_vec())
            .map_err(|err| SourceError::new(format!("GET {url} body failed: {err}")))
    }
}

#[cfg(test)]
mod tests;
