//! MariaDB builds from the `downloads.mariadb.org` REST API.

use serde::{Deserialize, Serialize};

use super::helpers::{archive_format, is_sha256_hex};
use super::{Arch, CatalogArtifact, CatalogVersion, Os, SourceError};

/// One file entry of the MariaDB REST API.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MariadbFile {
    /// File name.
    pub file_name: String,
    /// Operating system label, for example `Linux`.
    #[serde(default)]
    pub os: String,
    /// CPU label, for example `x86_64`.
    #[serde(default)]
    pub cpu: String,
    /// Download URL served by the API redirector.
    #[serde(default)]
    pub file_download_url: String,
    /// Checksums published by the API.
    #[serde(default)]
    pub checksum: MariadbChecksum,
    /// Size in bytes if the API reports it.
    #[serde(default)]
    pub size: Option<u64>,
}

/// Checksums attached to a MariaDB file.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct MariadbChecksum {
    /// SHA-256 in the API's `sha256sum` form.
    #[serde(default)]
    pub sha256sum: Option<String>,
}

/// One release of the MariaDB REST API.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MariadbRelease {
    /// Version tag.
    #[serde(default)]
    pub release_id: String,
    /// Files in the release.
    #[serde(default)]
    pub files: Vec<MariadbFile>,
}

/// Builds MariaDB versions and artifacts from a REST API release payload.
pub fn mariadb_from_api(
    release: &MariadbRelease,
    version: &str,
) -> Result<(Vec<CatalogVersion>, Vec<CatalogArtifact>), SourceError> {
    let mut artifacts = Vec::new();
    for file in &release.files {
        let Some((os, arch)) = mariadb_target(&file.os, &file.cpu, &file.file_name) else {
            continue;
        };
        let sha256 = file
            .checksum
            .sha256sum
            .as_deref()
            .ok_or_else(|| {
                SourceError::new(format!(
                    "MariaDB file {} has no checksum.sha256sum",
                    file.file_name
                ))
            })
            .and_then(|value| {
                if is_sha256_hex(value) {
                    Ok(value.to_ascii_lowercase())
                } else {
                    Err(SourceError::new(format!(
                        "MariaDB file {} has a malformed sha256sum",
                        file.file_name
                    )))
                }
            })?;
        let size_bytes = file.size.filter(|size| *size > 0).ok_or_else(|| {
            SourceError::new(format!(
                "MariaDB file {} has no size; refusing to record a catalog entry",
                file.file_name
            ))
        })?;
        if file.file_download_url.is_empty() {
            return Err(SourceError::new(format!(
                "MariaDB file {} has no download URL",
                file.file_name
            )));
        }
        let format = archive_format(&file.file_name).ok_or_else(|| {
            SourceError::new(format!(
                "MariaDB file {} has an unknown format",
                file.file_name
            ))
        })?;
        artifacts.push(CatalogArtifact {
            id: format!("mariadb-{version}-{}-{}", os.as_str(), arch.as_str()),
            engine: "mariadb".to_owned(),
            version: version.to_owned(),
            os: os.as_str().to_owned(),
            arch: arch.as_str().to_owned(),
            url: file.file_download_url.clone(),
            sha256,
            size_bytes,
            format: format.as_str().to_owned(),
            strip_prefix: None,
            min_glibc: None,
            variant: None,
        });
    }

    if artifacts.is_empty() {
        return Err(SourceError::new(format!(
            "MariaDB {version} has no usable bintar for a supported target"
        )));
    }

    let versions = vec![CatalogVersion {
        engine: "mariadb".to_owned(),
        version: version.to_owned(),
        major_line: version.split('.').next().unwrap_or(version).to_owned(),
        release_date: None,
        eol: false,
    }];
    Ok((versions, artifacts))
}

/// Maps a MariaDB REST API file to a supported `(os, arch)` target.
pub fn mariadb_target(os: &str, cpu: &str, file_name: &str) -> Option<(Os, Arch)> {
    if !file_name.ends_with(".tar.gz") && !file_name.ends_with(".zip") {
        return None;
    }
    let os = match os.to_ascii_lowercase().as_str() {
        "linux" => Os::Linux,
        "windows" => Os::Windows,
        _ => return None,
    };
    let arch = match cpu.to_ascii_lowercase().as_str() {
        "x86_64" | "amd64" => Arch::X86_64,
        "aarch64" | "arm64" => Arch::Aarch64,
        _ => return None,
    };
    Some((os, arch))
}
