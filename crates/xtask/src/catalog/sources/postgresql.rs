//! PostgreSQL builds from `theseus-rs/postgresql-binaries`.

use serde::Deserialize;

use super::helpers::parse_sha256_sidecar;
use super::{Arch, ArchiveFormat, CatalogArtifact, CatalogVersion, Os, Source, SourceError};

/// One release as reported by the GitHub Releases API.
#[derive(Debug, Clone, Deserialize)]
pub struct GithubRelease {
    /// Release tag, which is also the version tag.
    pub tag_name: String,
    /// Assets attached to the release.
    #[serde(default)]
    pub assets: Vec<GithubAsset>,
    /// Publish timestamp, RFC 3339.
    #[serde(default)]
    pub published_at: Option<String>,
}

/// One asset of a GitHub release.
#[derive(Debug, Clone, Deserialize)]
pub struct GithubAsset {
    /// File name.
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// Download URL.
    pub browser_download_url: String,
}

/// `(os, arch)` to the upstream target triple PostgreSQL publishes.
const POSTGRES_TARGETS: [(Os, Arch, &str); 6] = [
    (Os::Linux, Arch::X86_64, "x86_64-unknown-linux-gnu"),
    (Os::Linux, Arch::Aarch64, "aarch64-unknown-linux-gnu"),
    (Os::Macos, Arch::X86_64, "x86_64-apple-darwin"),
    (Os::Macos, Arch::Aarch64, "aarch64-apple-darwin"),
    (Os::Windows, Arch::X86_64, "x86_64-pc-windows-msvc"),
    (Os::Windows, Arch::Aarch64, "aarch64-pc-windows-msvc"),
];

/// Builds PostgreSQL versions and artifacts from a Releases API response.
///
/// Windows ships both `.tar.gz` and `.zip`; the `.zip` is preferred.
pub fn postgresql_from_releases(
    releases: &[GithubRelease],
    wanted: &[&str],
    source: &dyn Source,
) -> Result<(Vec<CatalogVersion>, Vec<CatalogArtifact>), SourceError> {
    let mut versions = Vec::new();
    let mut artifacts = Vec::new();

    for version in wanted {
        let release = releases
            .iter()
            .find(|release| release.tag_name == *version)
            .ok_or_else(|| {
                SourceError::new(format!("upstream has no PostgreSQL release {version}"))
            })?;

        let release_date = release
            .published_at
            .as_deref()
            .map(|stamp| stamp.split('T').next().unwrap_or(stamp).to_owned());
        versions.push(CatalogVersion {
            engine: "postgresql".to_owned(),
            version: (*version).to_owned(),
            major_line: version.split('.').next().unwrap_or(version).to_owned(),
            release_date,
            eol: false,
        });

        for (os, arch, triple) in POSTGRES_TARGETS {
            let artifact = if os == Os::Windows {
                pg_artifact(release, version, os, arch, triple, "zip", source)?.or(pg_artifact(
                    release, version, os, arch, triple, "tar.gz", source,
                )?)
            } else {
                pg_artifact(release, version, os, arch, triple, "tar.gz", source)?
            };
            let artifact = artifact.ok_or_else(|| {
                SourceError::new(format!(
                    "PostgreSQL {version} has no asset for {os}-{arch} ({triple})"
                ))
            })?;
            artifacts.push(artifact);
        }
    }

    Ok((versions, artifacts))
}

fn pg_artifact(
    release: &GithubRelease,
    version: &str,
    os: Os,
    arch: Arch,
    triple: &str,
    extension: &str,
    source: &dyn Source,
) -> Result<Option<CatalogArtifact>, SourceError> {
    let asset_name = format!("postgresql-{version}-{triple}.{extension}");
    let Some(asset) = release.assets.iter().find(|asset| asset.name == asset_name) else {
        return Ok(None);
    };
    let sha_name = format!("{asset_name}.sha256");
    let sha_asset = release
        .assets
        .iter()
        .find(|asset| asset.name == sha_name)
        .ok_or_else(|| {
            SourceError::new(format!(
                "PostgreSQL {version} asset {asset_name} has no .sha256 sidecar"
            ))
        })?;
    let sidecar = source.get_text(&sha_asset.browser_download_url)?;
    let sha256 = parse_sha256_sidecar(&sidecar, &asset_name)?;
    let format = extension.parse::<ArchiveFormat>().map_err(|_| {
        SourceError::new(format!(
            "unsupported PostgreSQL archive extension {extension}"
        ))
    })?;

    Ok(Some(CatalogArtifact {
        id: format!("postgresql-{version}-{}-{}", os.as_str(), arch.as_str()),
        engine: "postgresql".to_owned(),
        version: version.to_owned(),
        os: os.as_str().to_owned(),
        arch: arch.as_str().to_owned(),
        url: asset.browser_download_url.clone(),
        sha256,
        size_bytes: asset.size,
        format: format.as_str().to_owned(),
        strip_prefix: Some(
            asset_name
                .trim_end_matches(&format!(".{extension}"))
                .to_owned(),
        ),
        min_glibc: None,
        variant: None,
    }))
}
