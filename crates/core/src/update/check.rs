//! GitHub Releases update check.

use std::future::Future;
use std::pin::Pin;

use semver::Version;
use serde::Deserialize;

use crate::platform::{Arch, Os};

use super::error::{UpdateError, UpdateResult};
use super::model::{
    AvailableUpdate, ReleaseAsset, ReleaseChannel, UpdateAssetKind, UpdateCheck,
    platform_asset_tokens,
};

/// Maximum release notes kept in memory/returned to callers (bytes, UTF-8 boundary safe).
pub const MAX_RELEASE_NOTES_BYTES: usize = 8 * 1024;
/// Maximum GitHub releases JSON body accepted.
pub const MAX_RELEASES_BODY_BYTES: usize = 2 * 1024 * 1024;
/// Maximum checksums.txt body accepted.
pub const MAX_CHECKSUMS_BODY_BYTES: usize = 64 * 1024;

/// Options controlling an update check.
#[derive(Debug, Clone)]
pub struct UpdateCheckOptions {
    /// GitHub repository owner/name, e.g. `dblocal-org/dblocal`.
    pub repo: String,
    /// Release channel.
    pub channel: ReleaseChannel,
    /// Target OS; defaults to current in helpers.
    pub os: Os,
    /// Target arch; defaults to current in helpers.
    pub arch: Arch,
    /// Allow selecting a version <= current (default false).
    pub allow_downgrade: bool,
}

impl UpdateCheckOptions {
    /// Creates options for the given repo on the current platform.
    pub fn for_repo(repo: impl Into<String>) -> Self {
        Self {
            repo: repo.into(),
            channel: ReleaseChannel::Stable,
            os: Os::current(),
            arch: Arch::current(),
            allow_downgrade: false,
        }
    }

    /// Sets release channel.
    pub fn with_channel(mut self, channel: ReleaseChannel) -> Self {
        self.channel = channel;
        self
    }

    /// Sets target platform.
    pub fn with_platform(mut self, os: Os, arch: Arch) -> Self {
        self.os = os;
        self.arch = arch;
        self
    }

    /// Allows downgrade selection.
    pub fn with_allow_downgrade(mut self, allow: bool) -> Self {
        self.allow_downgrade = allow;
        self
    }
}

/// HTTP response for update checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response body bytes.
    pub body: Vec<u8>,
}

impl FetchResponse {
    /// Creates a response.
    pub fn new(status: u16, body: Vec<u8>) -> Self {
        Self { status, body }
    }
}

/// Abstraction over HTTP GET for tests and production.
pub trait UpdateFetcher: Send + Sync {
    /// Performs a GET request and returns status + body.
    fn get(
        &self,
        url: String,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<FetchResponse, String>> + Send + '_>>;
}

/// Production fetcher using reqwest with HTTPS-only and timeout.
#[derive(Debug, Clone)]
pub struct ReqwestUpdateFetcher {
    client: reqwest::Client,
}

impl ReqwestUpdateFetcher {
    /// Builds the production fetcher.
    pub fn new() -> UpdateResult<Self> {
        let client = reqwest::Client::builder()
            .https_only(true)
            .timeout(std::time::Duration::from_secs(15))
            .user_agent(format!("dblocal/{}", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 5 {
                    return attempt.error("too many redirects");
                }
                let url = attempt.url();
                let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
                let ok = url.scheme() == "https" && host == "api.github.com";
                if ok {
                    attempt.follow()
                } else {
                    attempt.error("redirect host not allowed")
                }
            }))
            .build()
            .map_err(|err| UpdateError::Network {
                reason: format!("gagal membuat HTTP client: {err}"),
            })?;
        Ok(Self { client })
    }
}

impl Default for ReqwestUpdateFetcher {
    fn default() -> Self {
        // Default is best-effort for callers that want a simple constructor;
        // real callers should prefer `new()` and handle errors.
        Self::new().unwrap_or_else(|_| Self {
            client: reqwest::Client::new(),
        })
    }
}

impl UpdateFetcher for ReqwestUpdateFetcher {
    fn get(
        &self,
        url: String,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<FetchResponse, String>> + Send + '_>> {
        Box::pin(async move {
            let response = self
                .client
                .get(&url)
                .header(reqwest::header::ACCEPT, "application/vnd.github+json")
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let status = response.status().as_u16();
            let bytes = response.bytes().await.map_err(|e| e.to_string())?;
            Ok(FetchResponse::new(status, bytes.to_vec()))
        })
    }
}

/// Checks GitHub releases for a newer applicable update.
pub async fn check(
    current: &Version,
    fetcher: &dyn UpdateFetcher,
    options: &UpdateCheckOptions,
) -> UpdateResult<UpdateCheck> {
    let url = releases_url(&options.repo)?;
    let response = fetcher.get(url).await.map_err(map_network)?;
    let releases = parse_releases_response(&response)?;
    let selected = select_release(current, &releases, options)?;
    match selected {
        Some(update) => Ok(UpdateCheck::Available(update)),
        None => Ok(UpdateCheck::UpToDate {
            current: current.clone(),
        }),
    }
}

fn releases_url(repo: &str) -> UpdateResult<String> {
    let repo = repo.trim().trim_matches('/');
    if repo.is_empty()
        || repo.contains("..")
        || repo.contains('\\')
        || repo.chars().any(|c| c.is_whitespace())
    {
        return Err(UpdateError::Malformed {
            reason: "nama repo tidak valid".to_owned(),
        });
    }
    let mut parts = repo.split('/');
    let owner = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    if owner.is_empty() || name.is_empty() || parts.next().is_some() {
        return Err(UpdateError::Malformed {
            reason: "format repo harus owner/name".to_owned(),
        });
    }
    if !is_repo_segment(owner) || !is_repo_segment(name) {
        return Err(UpdateError::Malformed {
            reason: "nama repo mengandung karakter tidak valid".to_owned(),
        });
    }
    Ok(format!(
        "https://api.github.com/repos/{owner}/{name}/releases?per_page=20"
    ))
}

fn is_repo_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn map_network(reason: String) -> UpdateError {
    let lower = reason.to_ascii_lowercase();
    if lower.contains("429") || lower.contains("rate limit") {
        return UpdateError::RateLimited { reason };
    }
    UpdateError::Network { reason }
}

fn parse_releases_response(response: &FetchResponse) -> UpdateResult<Vec<GhRelease>> {
    if response.status == 403 || response.status == 429 {
        let reason = body_text_lossy(&response.body);
        if looks_rate_limited(response.status, &reason) {
            return Err(UpdateError::RateLimited { reason });
        }
        return Err(UpdateError::Network {
            reason: format!("GitHub API mengembalikan status {}", response.status),
        });
    }
    if response.status != 200 {
        return Err(UpdateError::Network {
            reason: format!("GitHub API mengembalikan status {}", response.status),
        });
    }
    if response.body.len() > MAX_RELEASES_BODY_BYTES {
        return Err(UpdateError::Malformed {
            reason: "respons rilis terlalu besar".to_owned(),
        });
    }
    serde_json::from_slice::<Vec<GhRelease>>(&response.body).map_err(|err| UpdateError::Malformed {
        reason: format!("JSON rilis tidak valid: {err}"),
    })
}

fn body_text_lossy(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    text.chars().take(300).collect()
}

fn looks_rate_limited(status: u16, body: &str) -> bool {
    if status == 429 {
        return true;
    }
    let lower = body.to_ascii_lowercase();
    lower.contains("rate limit") || lower.contains("api rate limit exceeded")
}

fn select_release(
    current: &Version,
    releases: &[GhRelease],
    options: &UpdateCheckOptions,
) -> UpdateResult<Option<AvailableUpdate>> {
    let mut best: Option<(Version, AvailableUpdate)> = None;
    for release in releases {
        if release.draft {
            continue;
        }
        if release.prerelease && options.channel == ReleaseChannel::Stable {
            continue;
        }
        let version = match parse_release_version(&release.tag_name) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if version == *current {
            continue;
        }
        if version < *current && !options.allow_downgrade {
            continue;
        }
        if best
            .as_ref()
            .is_some_and(|(best_version, _)| version <= *best_version)
        {
            continue;
        }
        let Some(update) = release_to_update(release, version.clone(), options)? else {
            continue;
        };
        best = Some((version, update));
    }
    Ok(best.map(|(_, update)| update))
}

fn parse_release_version(tag: &str) -> UpdateResult<Version> {
    let trimmed = tag.trim();
    let no_v = trimmed.strip_prefix('v').unwrap_or(trimmed);
    Version::parse(no_v).map_err(|err| UpdateError::Malformed {
        reason: format!("versi rilis tidak valid '{tag}': {err}"),
    })
}

fn release_to_update(
    release: &GhRelease,
    version: Version,
    options: &UpdateCheckOptions,
) -> UpdateResult<Option<AvailableUpdate>> {
    let checksums = release
        .assets
        .iter()
        .find(|a| is_checksums_asset(&a.name))
        .map(asset_from_gh)
        .transpose()?;
    let Some(checksums_asset) = checksums else {
        return Ok(None);
    };
    let Some(asset) = select_platform_asset(&release.assets, options.os, options.arch)? else {
        return Ok(None);
    };
    let notes = sanitize_release_notes(release.body.as_deref().unwrap_or_default());
    Ok(Some(AvailableUpdate::new(
        version,
        release.tag_name.clone(),
        notes,
        asset,
        checksums_asset,
    )))
}

fn is_checksums_asset(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "checksums.txt" || lower == "sha256sums.txt" || lower.ends_with("-checksums.txt")
}

fn select_platform_asset(
    assets: &[GhAsset],
    os: Os,
    arch: Arch,
) -> UpdateResult<Option<ReleaseAsset>> {
    let tokens = platform_asset_tokens(os, arch).ok_or_else(|| UpdateError::AssetMissing {
        reason: format!("platform tidak didukung: {os}-{arch}"),
    })?;
    let mut selected: Option<ReleaseAsset> = None;
    for asset in assets {
        let Some(kind) = classify_install_asset(&asset.name, os) else {
            continue;
        };
        let lower = asset.name.to_ascii_lowercase();
        if !tokens.iter().any(|t| lower.contains(t)) {
            continue;
        }
        let candidate = asset_from_gh(asset)?.with_kind(kind);
        if should_replace_asset(selected.as_ref(), &candidate) {
            selected = Some(candidate);
        }
    }
    Ok(selected)
}

fn should_replace_asset(current: Option<&ReleaseAsset>, candidate: &ReleaseAsset) -> bool {
    let Some(current) = current else {
        return true;
    };
    asset_priority(candidate.kind()) > asset_priority(current.kind())
}

fn asset_priority(kind: UpdateAssetKind) -> u8 {
    match kind {
        UpdateAssetKind::AppImage => 50,
        UpdateAssetKind::TarGz => 40,
        UpdateAssetKind::Dmg => 40,
        UpdateAssetKind::Zip => 30,
        UpdateAssetKind::Msi => 20,
        UpdateAssetKind::Checksums => 0,
    }
}

fn classify_install_asset(name: &str, os: Os) -> Option<UpdateAssetKind> {
    let lower = name.to_ascii_lowercase();
    match os {
        Os::Linux => {
            if lower.ends_with(".appimage") {
                Some(UpdateAssetKind::AppImage)
            } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
                Some(UpdateAssetKind::TarGz)
            } else {
                None
            }
        }
        Os::Windows => {
            if lower.ends_with(".zip") {
                Some(UpdateAssetKind::Zip)
            } else if lower.ends_with(".msi") {
                Some(UpdateAssetKind::Msi)
            } else {
                None
            }
        }
        Os::Macos => {
            if lower.ends_with(".dmg") {
                Some(UpdateAssetKind::Dmg)
            } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
                Some(UpdateAssetKind::TarGz)
            } else {
                None
            }
        }
    }
}

fn asset_from_gh(asset: &GhAsset) -> UpdateResult<ReleaseAsset> {
    let kind = if is_checksums_asset(&asset.name) {
        UpdateAssetKind::Checksums
    } else {
        UpdateAssetKind::TarGz
    };
    let url = asset.browser_download_url.clone();
    validate_asset_name(&asset.name)?;
    Ok(ReleaseAsset::new(
        asset.name.clone(),
        url,
        asset.size.max(0) as u64,
        kind,
    ))
}

trait ReleaseAssetKindExt {
    fn with_kind(self, kind: UpdateAssetKind) -> Self;
}

impl ReleaseAssetKindExt for ReleaseAsset {
    fn with_kind(mut self, kind: UpdateAssetKind) -> Self {
        self = ReleaseAsset::new(
            self.name().to_owned(),
            self.url().to_owned(),
            self.size_bytes(),
            kind,
        );
        self
    }
}

fn validate_asset_name(name: &str) -> UpdateResult<()> {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.chars().any(|c| c.is_control())
    {
        return Err(UpdateError::Malformed {
            reason: format!("nama asset tidak aman: {name}"),
        });
    }
    Ok(())
}

/// Sanitizes release notes as plain text and bounds length.
pub fn sanitize_release_notes(body: &str) -> String {
    let mut out = String::new();
    let mut last_was_space = false;
    for ch in body.chars() {
        let mapped = if ch == '\r' || ch == '\n' || ch == '\t' || ch.is_control() {
            ' '
        } else {
            ch
        };
        if mapped.is_whitespace() {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(mapped);
            last_was_space = false;
        }
        if out.len() >= MAX_RELEASE_NOTES_BYTES {
            break;
        }
    }
    let trimmed = out.trim();
    if trimmed.len() <= MAX_RELEASE_NOTES_BYTES {
        return trimmed.to_owned();
    }
    let mut end = MAX_RELEASE_NOTES_BYTES;
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    trimmed[..end].trim_end().to_owned()
}

#[derive(Debug, Clone, Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Debug, Clone, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: i64,
}

#[cfg(test)]
#[path = "check/tests.rs"]
mod tests;
