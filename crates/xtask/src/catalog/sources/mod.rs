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
mod redis;
mod tracked;
mod valkey;

pub use helpers::{assemble, require_https};
pub use mariadb::{mariadb_from_api, mariadb_release_from_api};
#[cfg(test)]
pub use mysql::md5_hex as mysql_md5_hex;
pub use mysql::{MysqlBuild, mysql_artifact, mysql_builds_for, mysql_target_optional};
pub use postgresql::{GithubRelease, postgresql_from_releases};
pub use redis::redis_from_apt;
#[cfg(test)]
pub use tracked::find as find_tracked;
pub use tracked::{TrackedVersion, for_engine as tracked_for_engine};
pub use valkey::{WAVE_ONE_VARIANTS, valkey_from_sidecars};

pub use dblocal_core::catalog::{
    CATALOG_SCHEMA_VERSION, CatalogArtifact, CatalogDocument, CatalogVersion,
};
pub use dblocal_core::model::ArchiveFormat;
pub use dblocal_core::platform::{Arch, Os};

/// Builds the MariaDB REST API URL for one version.
pub fn mariadb_api_url(version: &str) -> String {
    format!("https://downloads.mariadb.org/rest-api/mariadb/{version}/")
}

/// Where the runtime refreshes the catalog from.
///
/// Published at `catalog/engines-v3.json` (`schema_version: 3`) so released
/// v0.1.3/v0.1.4 clients keep reading `catalog/engines-v2.json` without
/// rejecting `redis` or `deb` entries, and older v0.1.x clients keep reading
/// `catalog/engines.json` (`schema_version: 1`).
pub const CATALOG_SOURCE_URL: &str =
    "https://raw.githubusercontent.com/rezkycodes/dblocal/main/catalog/engines-v3.json";

/// Refresh URL written into the compatible `catalog/engines-v2.json` output
/// for released `v0.1.3`/`v0.1.4` clients.
pub const CATALOG_V2_SOURCE_URL: &str =
    "https://raw.githubusercontent.com/rezkycodes/dblocal/main/catalog/engines-v2.json";

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

/// Most redirect hops a builder request may follow, and it must stay on TLS.
pub const MAX_REDIRECTS: usize = 5;

/// Decides whether one redirect hop may be followed.
///
/// Any target that is not HTTPS is refused, and so is a chain longer than
/// [`MAX_REDIRECTS`]. Pure, so it is unit-testable without a server.
pub fn allow_redirect(previous: usize, next: &url::Url) -> Result<bool, SourceError> {
    if next.scheme() != "https" {
        return Err(SourceError::new(format!(
            "refusing a redirect to non-HTTPS {} (AGENTS.md invariant 6)",
            next.scheme()
        )));
    }
    if previous > MAX_REDIRECTS {
        return Err(SourceError::new(format!(
            "exceeded {MAX_REDIRECTS} redirects"
        )));
    }
    Ok(true)
}

/// The only `Accept-Encoding` the builder ever asks for.
pub const ENCODING_IDENTITY: &str = "identity";
/// The alternative encoding we compare against (see [`Source::head_size`]).
pub const ENCODING_GZIP: &str = "gzip";

/// Minimal HTTP transport, injectable for tests.
pub trait Source {
    /// Fetches `url` as bytes.
    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, SourceError>;
    /// Fetches `url` as text with an explicit `Accept-Encoding`.
    ///
    /// CloudFront can serve different objects for the same URL depending on the
    /// encoding, so the caller decides which object to read.
    fn get_text_with(&self, url: &str, accept_encoding: &str) -> Result<String, SourceError>;
    /// Returns `Content-Length` for `url` via `HEAD`, with an explicit
    /// `Accept-Encoding`.
    fn head_size(&self, url: &str, accept_encoding: &str) -> Result<u64, SourceError>;

    /// Fetches `url` as text with the identity encoding.
    fn get_text(&self, url: &str) -> Result<String, SourceError> {
        self.get_text_with(url, ENCODING_IDENTITY)
    }
}

/// HTTPS transport used by the real build.
#[derive(Debug, Clone)]
pub struct HttpSource {
    client: reqwest::blocking::Client,
    github_token: Option<String>,
}

impl HttpSource {
    /// Builds a client with a bounded timeout and an optional GitHub token.
    ///
    /// The client refuses any non-HTTPS request and follows at most
    /// [`MAX_REDIRECTS`] hops that stay on HTTPS, so a redirect cannot move a
    /// request to cleartext.
    pub fn new(github_token: Option<String>) -> Result<Self, SourceError> {
        Self::build(github_token, true)
    }

    fn build(github_token: Option<String>, https_only: bool) -> Result<Self, SourceError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .user_agent("dblocal-catalog-builder")
            .https_only(https_only)
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                let previous = attempt.previous().len();
                match allow_redirect(previous, attempt.url()) {
                    Ok(true) => attempt.follow(),
                    Ok(false) => attempt.stop(),
                    Err(err) => attempt.error(err.to_string()),
                }
            }))
            .build()
            .map_err(|err| SourceError::new(format!("could not build HTTP client: {err}")))?;
        Ok(Self {
            client,
            github_token,
        })
    }

    /// A client that may talk plain HTTP to a loopback test server.
    #[cfg(test)]
    pub(crate) fn for_loopback() -> Result<Self, SourceError> {
        Self::build(None, false)
    }
}

impl Source for HttpSource {
    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, SourceError> {
        require_https(url)?;
        self.request(reqwest::Method::GET, url, None)
    }

    fn get_text_with(&self, url: &str, accept_encoding: &str) -> Result<String, SourceError> {
        require_https(url)?;
        let bytes = self.request(reqwest::Method::GET, url, Some(accept_encoding))?;
        String::from_utf8(bytes)
            .map_err(|err| SourceError::new(format!("{url} is not UTF-8: {err}")))
    }

    fn head_size(&self, url: &str, accept_encoding: &str) -> Result<u64, SourceError> {
        require_https(url)?;
        self.head(url, accept_encoding)
    }
}

impl HttpSource {
    /// HEAD without the HTTPS check, so loopback tests can exercise it.
    pub(crate) fn head(&self, url: &str, accept_encoding: &str) -> Result<u64, SourceError> {
        let response = self
            .build_request(reqwest::Method::HEAD, url, Some(accept_encoding))?
            .send()
            .map_err(|err| SourceError::new(format!("HEAD {url} failed: {err}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(SourceError::new(format!(
                "HEAD {url} returned HTTP {}",
                status.as_u16()
            )));
        }
        // A HEAD reply carries no body, so `content_length()` (a body size hint)
        // is zero or absent; the declared length lives in the header.
        response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|length| *length > 0)
            .ok_or_else(|| SourceError::new(format!("HEAD {url} returned no Content-Length")))
    }
}

impl HttpSource {
    /// Performs the GET itself.
    ///
    /// Split from [`Source::get_bytes`] so the request/response handling can be
    /// exercised against an ephemeral local server without relaxing the
    /// HTTPS-only boundary.
    pub(crate) fn request(
        &self,
        method: reqwest::Method,
        url: &str,
        accept_encoding: Option<&str>,
    ) -> Result<Vec<u8>, SourceError> {
        let verb = method.clone();
        let response = self
            .build_request(method, url, accept_encoding)?
            .send()
            .map_err(|err| SourceError::new(format!("{verb} {url} failed: {err}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(SourceError::new(format!(
                "{verb} {url} returned HTTP {}",
                status.as_u16()
            )));
        }
        response
            .bytes()
            .map(|bytes| bytes.to_vec())
            .map_err(|err| SourceError::new(format!("{verb} {url} body failed: {err}")))
    }

    pub(crate) fn build_request(
        &self,
        method: reqwest::Method,
        url: &str,
        accept_encoding: Option<&str>,
    ) -> Result<reqwest::blocking::RequestBuilder, SourceError> {
        let mut request = self.client.request(method, url);
        // CloudFront serves different objects per encoding; pin identity unless
        // the caller is deliberately probing another object.
        let encoding = accept_encoding.unwrap_or(ENCODING_IDENTITY);
        request = request.header(reqwest::header::ACCEPT_ENCODING, encoding);
        if url.starts_with("https://api.github.com/")
            && let Some(token) = &self.github_token
        {
            request = request.bearer_auth(token);
        }
        Ok(request)
    }
}

#[cfg(test)]
mod tests;
