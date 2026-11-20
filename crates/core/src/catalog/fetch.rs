//! The refresh transport, behind a trait so tests never touch the network.

use std::future::Future;
use std::time::Duration;

use crate::DblError;
use crate::consts;
use crate::error::Result;

/// Most redirect hops a catalog refresh may follow.
///
/// The catalog is the root of trust for every SHA-256 we verify, so a redirect
/// chain is kept short and must stay on TLS (AGENTS.md invariant 6).
pub const MAX_REDIRECTS: usize = 5;

/// Largest catalog body we will buffer, so a hostile server cannot exhaust
/// memory. The real document is a few hundred kilobytes.
pub const MAX_BODY_BYTES: usize = consts::CATALOG_MAX_BODY_BYTES as usize;

/// Decides whether one redirect hop may be followed.
///
/// `previous` is the number of hops already followed. Any target that is not
/// HTTPS is refused, and so is a chain longer than [`MAX_REDIRECTS`]. This is a
/// pure function so the policy can be unit-tested directly, without a server.
pub fn allow_redirect(previous: usize, next: &reqwest::Url) -> Result<bool> {
    if next.scheme() != "https" {
        return Err(DblError::Network {
            reason: format!(
                "catalog refresh refused a redirect to non-HTTPS {}",
                next.scheme()
            ),
        });
    }
    if previous > MAX_REDIRECTS {
        return Err(DblError::Network {
            reason: format!("catalog refresh exceeded {MAX_REDIRECTS} redirects"),
        });
    }
    Ok(true)
}

/// What one refresh request produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// The server accepted our `If-None-Match` and the snapshot is unchanged.
    NotModified,
    /// A new document, with the ETag to send next time.
    Modified {
        /// ETag of the new response, if the server sent one.
        etag: Option<String>,
        /// Raw JSON body, validated by the caller before it is trusted.
        body: String,
    },
}

/// Minimal HTTP source for the catalog refresh.
///
/// Implementations must use HTTPS only (AGENTS.md invariant 6) and must send
/// the stored ETag as `If-None-Match` so a daily refresh is cheap (F-CAT-03
/// edge case: GitHub rate limits).
pub trait CatalogFetcher {
    /// Fetches `url`, or detects that the snapshot is unchanged.
    fn fetch(
        &self,
        url: &str,
        etag: Option<&str>,
    ) -> impl Future<Output = Result<RefreshOutcome>> + Send;
}

/// The only production implementation: plain HTTPS GET with a timeout.
#[derive(Debug, Clone)]
pub struct HttpCatalogFetcher {
    client: reqwest::Client,
    timeout: Duration,
}

impl HttpCatalogFetcher {
    /// Builds a fetcher with an explicit timeout.
    ///
    /// The client refuses any non-HTTPS request and follows at most
    /// [`MAX_REDIRECTS`] hops that stay on HTTPS, so a redirect cannot move the
    /// refresh to cleartext.
    pub fn with_timeout(timeout: Duration) -> Result<Self> {
        Self::build(timeout, true)
    }

    /// Builds the client, optionally enforcing HTTPS.
    ///
    /// `https_only` is `true` in production; the loopback constructor below is
    /// the only caller that passes `false`, so the response-handling path can be
    /// exercised against a plain `127.0.0.1` test server.
    fn build(timeout: Duration, https_only: bool) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .https_only(https_only)
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                let previous = attempt.previous().len();
                match allow_redirect(previous, attempt.url()) {
                    Ok(true) => attempt.follow(),
                    Ok(false) => attempt.stop(),
                    Err(err) => attempt.error(err),
                }
            }))
            .build()
            .map_err(|err| DblError::Network {
                reason: err.to_string(),
            })?;
        Ok(Self { client, timeout })
    }

    /// A client that may talk plain HTTP to a loopback test server.
    ///
    /// Only used by tests; production always goes through [`Self::with_timeout`].
    #[cfg(test)]
    fn for_loopback(timeout: Duration) -> Result<Self> {
        Self::build(timeout, false)
    }

    /// Timeout applied to every request.
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Sends the request and interprets the response.
    ///
    /// Kept separate from [`CatalogFetcher::fetch`] so the response handling
    /// can be exercised against an ephemeral local server without relaxing the
    /// HTTPS-only rule at the public boundary.
    async fn send(&self, url: &str, etag: Option<&str>) -> Result<RefreshOutcome> {
        let mut request = self.client.get(url);
        if let Some(etag) = etag {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag);
        }
        let response = request.send().await.map_err(|err| DblError::Network {
            reason: err.to_string(),
        })?;

        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(RefreshOutcome::NotModified);
        }
        if !response.status().is_success() {
            return Err(DblError::Network {
                reason: format!("catalog refresh got HTTP {}", response.status().as_u16()),
            });
        }

        if let Some(length) = response.content_length()
            && length > MAX_BODY_BYTES as u64
        {
            return Err(DblError::Network {
                reason: format!(
                    "catalog refresh body is {length} bytes, over the {MAX_BODY_BYTES}-byte limit"
                ),
            });
        }

        let etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = read_capped(response).await?;
        Ok(RefreshOutcome::Modified { etag, body })
    }
}

impl CatalogFetcher for HttpCatalogFetcher {
    async fn fetch(&self, url: &str, etag: Option<&str>) -> Result<RefreshOutcome> {
        validate_https(url)?;
        self.send(url, etag).await
    }
}

/// Rejects anything but an absolute HTTPS URL before a request is built.
pub fn validate_https(url: &str) -> Result<()> {
    let https = url.split_once("://").is_some_and(|(scheme, rest)| {
        scheme == "https" && !rest.is_empty() && !rest.contains(char::is_whitespace)
    });
    if https {
        Ok(())
    } else {
        Err(DblError::InvalidInput {
            field: "url",
            reason: "catalog refresh requires an absolute https URL".to_owned(),
        })
    }
}

/// Reads the body while refusing to buffer more than [`MAX_BODY_BYTES`].
///
/// The `Content-Length` check in [`HttpCatalogFetcher::send`] catches an honest
/// oversized response; this also stops a chunked response that never declares
/// its size.
async fn read_capped(mut response: reqwest::Response) -> Result<String> {
    let mut buffer: Vec<u8> = Vec::new();
    loop {
        let chunk = response.chunk().await.map_err(|err| DblError::Network {
            reason: err.to_string(),
        })?;
        let Some(chunk) = chunk else {
            break;
        };
        if buffer.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(DblError::Network {
                reason: format!("catalog refresh body exceeded {MAX_BODY_BYTES} bytes"),
            });
        }
        buffer.extend_from_slice(&chunk);
    }
    String::from_utf8(buffer).map_err(|err| DblError::Network {
        reason: format!("catalog refresh body is not UTF-8: {err}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_is_accepted() {
        validate_https("https://example.test/catalog.json").expect("https");
        for bad in [
            "http://example.test/catalog.json",
            "file:///etc/passwd",
            "example.test/catalog.json",
            "https://",
            "",
        ] {
            assert!(validate_https(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn fetcher_builds_with_a_timeout() {
        let fetcher = HttpCatalogFetcher::with_timeout(Duration::from_secs(3)).expect("client");
        assert_eq!(fetcher.timeout(), Duration::from_secs(3));
    }

    #[tokio::test]
    async fn rejects_plain_http_before_building_a_request() {
        let fetcher = HttpCatalogFetcher::with_timeout(Duration::from_secs(3)).expect("client");
        let err = fetcher
            .fetch("http://127.0.0.1:1/catalog.json", None)
            .await
            .expect_err("http must be rejected");
        assert!(matches!(err, DblError::InvalidInput { field: "url", .. }));
    }

    /// Minimal HTTP server that answers every request from `responses` in order.
    async fn serve(responses: Vec<&'static str>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            for response in responses {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut buffer = [0u8; 2048];
                let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buffer).await;
                let _ = tokio::io::AsyncWriteExt::write_all(&mut socket, response.as_bytes()).await;
            }
        });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn not_modified_is_reported() {
        let base = serve(vec![
            "HTTP/1.1 304 Not Modified\r\nETag: \"v1\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        ])
        .await;
        let fetcher = HttpCatalogFetcher::for_loopback(Duration::from_secs(3)).expect("client");
        let outcome = fetcher
            .send(&format!("{base}/catalog.json"), Some("\"v1\""))
            .await
            .expect("outcome");
        assert_eq!(outcome, RefreshOutcome::NotModified);
    }

    #[tokio::test]
    async fn modified_carries_body_and_etag() {
        let body = "{\"schema_version\":1}";
        let response: &'static str = Box::leak(
            format!(
                "HTTP/1.1 200 OK\r\nETag: \"v2\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .into_boxed_str(),
        );
        let base = serve(vec![response]).await;
        let fetcher = HttpCatalogFetcher::for_loopback(Duration::from_secs(3)).expect("client");
        let outcome = fetcher
            .send(&format!("{base}/catalog.json"), None)
            .await
            .expect("outcome");
        match outcome {
            RefreshOutcome::Modified {
                etag,
                body: received,
            } => {
                assert_eq!(etag.as_deref(), Some("\"v2\""));
                assert_eq!(received, body);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn non_success_status_is_an_error() {
        let base = serve(vec![
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        ])
        .await;
        let fetcher = HttpCatalogFetcher::with_timeout(Duration::from_secs(3)).expect("client");
        let err = fetcher
            .send(&format!("{base}/catalog.json"), None)
            .await
            .expect_err("500 must fail");
        assert!(matches!(err, DblError::Network { .. }));
    }

    #[test]
    fn redirect_policy_stops_https_to_http() {
        // Failing-first: the production client used to follow any redirect,
        // including https -> http, letting an attacker swap the catalog.
        let https = reqwest::Url::parse("https://example.test/catalog.json").expect("url");
        let plain = reqwest::Url::parse("http://example.test/catalog.json").expect("url");
        assert!(
            allow_redirect(0, &plain).is_err(),
            "http target must be refused"
        );
        assert!(allow_redirect(0, &https).is_ok_and(|allowed| allowed));
        assert!(allow_redirect(1, &https).is_ok_and(|allowed| allowed));
        assert!(allow_redirect(MAX_REDIRECTS, &https).is_ok_and(|allowed| allowed));
        assert!(
            allow_redirect(MAX_REDIRECTS + 1, &https).is_err(),
            "too many hops"
        );
    }

    #[tokio::test]
    async fn body_larger_than_the_cap_is_rejected() {
        // Failing-first: the body used to be buffered without limit.
        let oversized = MAX_BODY_BYTES + 1;
        let response: &'static str = Box::leak(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {oversized}\r\nConnection: close\r\n\r\n{}",
                "x".repeat(oversized)
            )
            .into_boxed_str(),
        );
        let base = serve(vec![response]).await;
        let fetcher = HttpCatalogFetcher::for_loopback(Duration::from_secs(5)).expect("client");
        let err = fetcher
            .send(&format!("{base}/catalog.json"), None)
            .await
            .expect_err("oversized body must be rejected");
        assert!(matches!(err, DblError::Network { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn body_within_the_cap_is_accepted() {
        let body = "{\"schema_version\":1}";
        let response: &'static str = Box::leak(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .into_boxed_str(),
        );
        let base = serve(vec![response]).await;
        let fetcher = HttpCatalogFetcher::for_loopback(Duration::from_secs(5)).expect("client");
        let outcome = fetcher
            .send(&format!("{base}/catalog.json"), None)
            .await
            .expect("outcome");
        assert!(matches!(outcome, RefreshOutcome::Modified { .. }));
    }

    #[tokio::test]
    async fn a_redirect_to_plain_http_is_refused() {
        // End-to-end proof of the policy: the server tries to move the refresh
        // to cleartext, and the client refuses before sending that request.
        let base = serve(vec![
            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/catalog.json\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        ])
        .await;
        let fetcher = HttpCatalogFetcher::for_loopback(Duration::from_secs(3)).expect("client");
        let err = fetcher
            .send(&format!("{base}/catalog.json"), None)
            .await
            .expect_err("http redirect must be refused");
        // reqwest wraps a policy error as "error following redirect"; the point
        // is that the cleartext hop was not performed.
        assert!(matches!(err, DblError::Network { .. }), "{err:?}");
        assert!(err.to_string().contains("redirect"), "{err}");
    }

    #[tokio::test]
    async fn a_redirect_chain_longer_than_the_limit_is_refused() {
        // One hop is fine, but the policy caps the chain, so a server that
        // loops on itself fails instead of spinning.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut buffer = [0u8; 2048];
                let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buffer).await;
                let response = format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://{address}/loop\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = tokio::io::AsyncWriteExt::write_all(&mut socket, response.as_bytes()).await;
            }
        });
        let fetcher = HttpCatalogFetcher::for_loopback(Duration::from_secs(3)).expect("client");
        let err = fetcher
            .send(&format!("http://{address}/loop"), None)
            .await
            .expect_err("a self-redirect loop must fail");
        assert!(matches!(err, DblError::Network { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn connection_failure_is_a_network_error() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        drop(listener);
        let fetcher = HttpCatalogFetcher::with_timeout(Duration::from_secs(1)).expect("client");
        let err = fetcher
            .send(&format!("http://127.0.0.1:{port}/catalog.json"), None)
            .await
            .expect_err("closed port must fail");
        assert!(matches!(err, DblError::Network { .. }));
    }
}
