//! The refresh transport, behind a trait so tests never touch the network.

use std::future::Future;
use std::time::Duration;

use crate::DblError;
use crate::error::Result;

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
    pub fn with_timeout(timeout: Duration) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|err| DblError::Network {
                reason: err.to_string(),
            })?;
        Ok(Self { client, timeout })
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

        let etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = response.text().await.map_err(|err| DblError::Network {
            reason: err.to_string(),
        })?;
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
        let fetcher = HttpCatalogFetcher::with_timeout(Duration::from_secs(3)).expect("client");
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
        let fetcher = HttpCatalogFetcher::with_timeout(Duration::from_secs(3)).expect("client");
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
