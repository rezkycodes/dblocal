//! GitHub Releases update check.

use std::future::Future;
use std::pin::Pin;

use semver::Version;
use serde::Deserialize;

use crate::platform::{Arch, Os};

use super::error::{UpdateError, UpdateResult};
use super::kind::InstallKind;
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
    /// GitHub repository owner/name, e.g. `rezkycodes/dblocal`.
    pub repo: String,
    /// Release channel.
    pub channel: ReleaseChannel,
    /// Target OS; defaults to current in helpers.
    pub os: Os,
    /// Target arch; defaults to current in helpers.
    pub arch: Arch,
    /// Allow selecting a version <= current (default false).
    pub allow_downgrade: bool,
    /// Detected install kind, used to select the matching release asset
    /// (e.g. `.tar.gz` for `PortableTarball`, `.AppImage` for `AppImage`,
    /// `.msi` for `WindowsMsi`, `.deb`/`.rpm` for distro packages).
    pub install_kind: Option<InstallKind>,
    /// Test/debug-only base URL override (compiled out in release builds).
    #[cfg(any(test, debug_assertions))]
    pub api_base_url: Option<String>,
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
            install_kind: None,
            #[cfg(any(test, debug_assertions))]
            api_base_url: None,
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

    /// Sets the install kind so asset selection matches the running installation.
    pub fn with_install_kind(mut self, kind: InstallKind) -> Self {
        self.install_kind = Some(kind);
        self
    }

    /// Test/debug-only override of the GitHub API base URL (unavailable in release builds).
    #[cfg(any(test, debug_assertions))]
    pub fn with_api_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.api_base_url = Some(base_url.into());
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
            .timeout(std::time::Duration::from_secs(
                crate::consts::UPDATE_CHECK_TIMEOUT_SECS,
            ))
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
                reason: format!("could not build the HTTP client: {err}"),
            })?;
        Ok(Self { client })
    }

    /// Test/debug-only fetcher permitting loopback HTTP (unavailable in release builds).
    #[cfg(any(test, debug_assertions))]
    pub fn loopback_for_tests() -> UpdateResult<Self> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent(format!("dblocal/{}", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 5 {
                    return attempt.error("too many redirects");
                }
                let url = attempt.url();
                let is_loopback = match url.host() {
                    Some(url::Host::Domain(d)) => d == "localhost",
                    Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                    Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                    None => false,
                };
                if url.scheme() == "http" && is_loopback {
                    attempt.follow()
                } else {
                    attempt.error("redirect host not allowed")
                }
            }))
            .build()
            .map_err(|err| UpdateError::Network {
                reason: format!("could not build the test HTTP client: {err}"),
            })?;
        Ok(Self { client })
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
            if let Some(len) = response.content_length()
                && len > MAX_RELEASES_BODY_BYTES as u64
            {
                return Err(format!("release response too large: {len} bytes"));
            }
            let mut body: Vec<u8> = Vec::new();
            let mut stream = response;
            while let Some(chunk) = stream.chunk().await.map_err(|e| e.to_string())? {
                let next = body.len().saturating_add(chunk.len());
                if next > MAX_RELEASES_BODY_BYTES {
                    return Err(format!(
                        "release response too large: over {} bytes",
                        MAX_RELEASES_BODY_BYTES
                    ));
                }
                body.extend_from_slice(&chunk);
            }
            Ok(FetchResponse::new(status, body))
        })
    }
}

/// Checks GitHub releases for a newer applicable update.
pub async fn check(
    current: &Version,
    fetcher: &dyn UpdateFetcher,
    options: &UpdateCheckOptions,
) -> UpdateResult<UpdateCheck> {
    let url = releases_url_for_options(options)?;
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

fn releases_url_for_options(options: &UpdateCheckOptions) -> UpdateResult<String> {
    let (owner, name) = parse_repo(&options.repo)?;
    #[cfg(any(test, debug_assertions))]
    if let Some(base) = options.api_base_url.as_deref() {
        let base = base.trim_end_matches('/');
        return Ok(format!("{base}/repos/{owner}/{name}/releases?per_page=20"));
    }
    Ok(format!(
        "https://api.github.com/repos/{owner}/{name}/releases?per_page=20"
    ))
}

fn parse_repo(repo: &str) -> UpdateResult<(&str, &str)> {
    let repo = repo.trim().trim_matches('/');
    if repo.is_empty()
        || repo.contains("..")
        || repo.contains('\\')
        || repo.chars().any(|c| c.is_whitespace())
    {
        return Err(UpdateError::Malformed {
            reason: "invalid repository name".to_owned(),
        });
    }
    let mut parts = repo.split('/');
    let owner = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    if owner.is_empty() || name.is_empty() || parts.next().is_some() {
        return Err(UpdateError::Malformed {
            reason: "repository must be in owner/name form".to_owned(),
        });
    }
    if !is_repo_segment(owner) || !is_repo_segment(name) {
        return Err(UpdateError::Malformed {
            reason: "repository name contains invalid characters".to_owned(),
        });
    }
    Ok((owner, name))
}

#[cfg(test)]
fn releases_url(repo: &str) -> UpdateResult<String> {
    let (owner, name) = parse_repo(repo)?;
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
        return Err(UpdateError::SourceNotFound {
            reason: format!("the GitHub API returned status {}", response.status),
        });
    }
    if response.status == 404 {
        return Err(UpdateError::SourceNotFound {
            reason: format!("the GitHub API returned status {}", response.status),
        });
    }
    if response.status != 200 {
        return Err(UpdateError::Network {
            reason: format!("the GitHub API returned status {}", response.status),
        });
    }
    if response.body.len() > MAX_RELEASES_BODY_BYTES {
        return Err(UpdateError::Malformed {
            reason: "release response too large".to_owned(),
        });
    }
    serde_json::from_slice::<Vec<GhRelease>>(&response.body).map_err(|err| UpdateError::Malformed {
        reason: format!("invalid release JSON: {err}"),
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
        let version = match parse_release_version(&release.tag_name) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if options.channel == ReleaseChannel::Stable && !version.pre.is_empty() {
            continue;
        }
        if release.prerelease && options.channel == ReleaseChannel::Stable {
            continue;
        }
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
        reason: format!("invalid release version '{tag}': {err}"),
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
    let signature_asset = release
        .assets
        .iter()
        .find(|a| is_signature_asset(&a.name))
        .map(asset_from_gh)
        .transpose()?;
    let preferred_kind = options
        .install_kind
        .as_ref()
        .and_then(InstallKind::expected_asset_kind);
    let Some(asset) = select_platform_asset(
        &release.assets,
        &version,
        options.os,
        options.arch,
        preferred_kind,
    )?
    else {
        return Ok(None);
    };
    let notes = sanitize_release_notes(release.body.as_deref().unwrap_or_default());
    let release_url = resolve_release_url(
        release.html_url.as_deref(),
        &options.repo,
        &release.tag_name,
    );
    let mut update = AvailableUpdate::new(
        version,
        release.tag_name.clone(),
        notes,
        asset,
        checksums_asset,
    )
    .with_release_url(release_url);
    if let Some(sig) = signature_asset {
        update = update.with_signature_asset(sig);
    }
    Ok(Some(update))
}

fn is_signature_asset(name: &str) -> bool {
    name.eq_ignore_ascii_case("checksums.txt.sig")
}

fn resolve_release_url(html_url: Option<&str>, repo: &str, tag: &str) -> String {
    if let Some(raw) = html_url
        && let Ok(parsed) = reqwest::Url::parse(raw)
        && parsed.scheme() == "https"
        && parsed
            .host_str()
            .is_some_and(|h| h.eq_ignore_ascii_case("github.com"))
    {
        return parsed.to_string();
    }
    format!("https://github.com/{repo}/releases/tag/{tag}")
}

fn is_checksums_asset(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "checksums.txt" || lower == "sha256sums.txt" || lower.ends_with("-checksums.txt")
}

fn select_platform_asset(
    assets: &[GhAsset],
    version: &Version,
    os: Os,
    arch: Arch,
    preferred: Option<UpdateAssetKind>,
) -> UpdateResult<Option<ReleaseAsset>> {
    let tokens = platform_asset_tokens(os, arch).ok_or_else(|| UpdateError::AssetMissing {
        reason: format!("unsupported platform: {os}-{arch}"),
    })?;
    let mut exact_match: Option<ReleaseAsset> = None;
    let mut fallback: Option<ReleaseAsset> = None;

    for asset in assets {
        let Some(kind) = classify_install_asset(&asset.name, os) else {
            continue;
        };
        let lower = asset.name.to_ascii_lowercase();
        if !tokens.iter().any(|t| lower.contains(t)) {
            continue;
        }
        if !asset_matches_version(&asset.name, version, tokens) {
            continue;
        }
        let candidate = asset_from_gh(asset)?.with_kind(kind);
        if preferred == Some(kind) {
            exact_match = Some(candidate);
            break;
        }
        // Without a preferred kind, only pick self-updatable/portable assets as fallback
        // (never pick .deb/.rpm when a tarball/AppImage exists).
        if should_replace_asset(fallback.as_ref(), &candidate) {
            fallback = Some(candidate);
        }
    }

    if exact_match.is_some() {
        return Ok(exact_match);
    }
    // If the caller specifically requires an exact self-updatable format (e.g. AppImage or TarGz),
    // do not silently fall back to a mismatched format like AppImage for PortableTarball.
    if matches!(
        preferred,
        Some(
            UpdateAssetKind::AppImage
                | UpdateAssetKind::TarGz
                | UpdateAssetKind::Zip
                | UpdateAssetKind::Msi
        )
    ) {
        return Ok(None);
    }
    Ok(fallback)
}

/// Verifies that `name` embeds the release's exact `version` on token boundaries
/// so an older signed `checksums.txt` + older asset (including `.msi`, which has
/// no pre-install `--version` smoke check) re-uploaded under a newer tag is refused.
fn asset_matches_version(name: &str, version: &Version, platform_tokens: &[&str]) -> bool {
    let lower = name.to_ascii_lowercase();
    let canonical = version.to_string().to_ascii_lowercase();
    let mut candidates = vec![canonical.clone()];
    if !version.pre.is_empty() {
        // GitHub Releases / build-deb/rpm normalize prerelease `-` to `.` (from `~`).
        let normalized = canonical.replace('-', ".");
        if normalized != canonical {
            candidates.push(normalized);
        }
    }

    for ver in &candidates {
        let mut search_from = 0;
        while let Some(rel) = lower[search_from..].find(ver.as_str()) {
            let start = search_from + rel;
            let end = start + ver.len();
            search_from = start + 1;

            let prefix_ok = is_valid_version_prefix(&lower[..start]);
            let suffix_ok = is_valid_version_suffix(&lower[end..], platform_tokens);
            if prefix_ok && suffix_ok {
                return true;
            }
        }
    }
    false
}

fn is_valid_version_prefix(prefix: &str) -> bool {
    if prefix.ends_with('-') || prefix.ends_with('_') {
        return true;
    }
    if let Some(before_v) = prefix.strip_suffix('v') {
        return before_v.ends_with('-') || before_v.ends_with('_');
    }
    false
}

fn is_valid_version_suffix(suffix: &str, platform_tokens: &[&str]) -> bool {
    if let Some(rest) = suffix
        .strip_prefix('-')
        .or_else(|| suffix.strip_prefix('_'))
    {
        // Require the next segment to be a platform token (e.g. `linux-x86_64`,
        // `windows-x86_64`, `amd64`) or an RPM numeric release (`1.x86_64.rpm`),
        // so stable `0.2.0` never matches prerelease `0.2.0-rc.1-...`.
        if platform_tokens
            .iter()
            .any(|t| suffix.starts_with(t) || rest.starts_with(t))
        {
            return true;
        }
        let mut chars = rest.chars().peekable();
        let mut digits = 0usize;
        while chars.peek().is_some_and(char::is_ascii_digit) {
            chars.next();
            digits += 1;
        }
        return digits > 0 && chars.peek() == Some(&'.');
    }
    if let Some(rest) = suffix.strip_prefix('.') {
        return rest.chars().next().is_some_and(|c| !c.is_ascii_digit());
    }
    false
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
        UpdateAssetKind::Deb => 10,
        UpdateAssetKind::Rpm => 10,
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
            } else if lower.ends_with(".deb") {
                Some(UpdateAssetKind::Deb)
            } else if lower.ends_with(".rpm") {
                Some(UpdateAssetKind::Rpm)
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

fn validate_asset_name(name: &str) -> UpdateResult<()> {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.chars().any(|c| c.is_control())
    {
        return Err(UpdateError::Malformed {
            reason: format!("unsafe asset name: {name}"),
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
    html_url: Option<String>,
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
