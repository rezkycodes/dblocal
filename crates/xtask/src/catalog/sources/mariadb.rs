//! MariaDB builds from the `downloads.mariadb.org` REST API.

use serde::{Deserialize, Serialize};

use super::helpers::{archive_format, archive_root, is_sha256_hex};
use super::{Arch, CatalogArtifact, CatalogVersion, Os, Source, SourceError};

/// One file entry of the MariaDB REST API.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MariadbFile {
    /// File name.
    pub file_name: String,
    /// Operating system label, for example `Linux`.
    ///
    /// The API sends `null` for source-only files, so a null is an empty label.
    #[serde(default, deserialize_with = "null_as_empty")]
    pub os: String,
    /// CPU label, for example `x86_64`. Null for source-only files.
    #[serde(default, deserialize_with = "null_as_empty")]
    pub cpu: String,
    /// Download URL served by the API redirector.
    ///
    /// The REST API returns `http://` here even when the object is only served
    /// over TLS; [`mariadb_https_url`] upgrades it before it is recorded.
    #[serde(default, deserialize_with = "null_as_empty")]
    pub file_download_url: String,
    /// Checksums published by the API.
    #[serde(default)]
    pub checksum: MariadbChecksum,
    /// Size in bytes if the API reports it.
    #[serde(default)]
    pub size: Option<u64>,
}

/// Deserialises an explicit JSON `null` (or a missing field) as an empty string.
fn null_as_empty<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
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
    /// Release date the API reports (`YYYY-MM-DD`), if any.
    #[serde(default, deserialize_with = "null_as_empty")]
    pub date_of_release: String,
    /// Files in the release.
    #[serde(default)]
    pub files: Vec<MariadbFile>,
}

/// The envelope `/rest-api/mariadb/<version>/` returns for one version.
///
/// The newer endpoint wraps a single release in `release_data`; the older one
/// uses `releases`. Both shapes are accepted so the builder does not break when
/// the API changes layout again. A body that is itself one release (the fixture
/// and a per-version lookup) is also accepted.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MariadbVersionPayload {
    /// Newer single-release envelope.
    #[serde(default)]
    pub release_data: Option<std::collections::BTreeMap<String, MariadbRelease>>,
    /// Older multi-release envelope.
    #[serde(default)]
    pub releases: Option<std::collections::BTreeMap<String, MariadbRelease>>,
    /// A bare release object, used by fixtures and the per-version endpoint.
    #[serde(default)]
    pub release_id: Option<String>,
    /// Release date of the bare release object.
    #[serde(default)]
    pub date_of_release: Option<String>,
    /// Files of the bare release object.
    #[serde(default)]
    pub files: Option<Vec<MariadbFile>>,
}

impl MariadbVersionPayload {
    /// The release whose `release_id` matches `version`.
    pub fn release(&self, version: &str) -> Option<MariadbRelease> {
        self.release_data
            .as_ref()
            .and_then(|map| map.get(version))
            .or_else(|| self.releases.as_ref().and_then(|map| map.get(version)))
            .cloned()
            .or_else(|| {
                let id = self.release_id.as_deref()?;
                let files = self.files.clone()?;
                (id == version).then(|| MariadbRelease {
                    release_id: id.to_owned(),
                    date_of_release: self.date_of_release.clone().unwrap_or_default(),
                    files,
                })
            })
    }
}

/// Fetches one MariaDB release payload and extracts `version`, resolving the
/// size of the chosen build with a `HEAD` request.
///
/// The REST API reports `size: null` for the server tarballs, so the declared
/// size is read from the mirror that serves the object. `expected_size` is the
/// value the builder then records and the downloader enforces.
pub fn mariadb_release_from_api(
    source: &dyn Source,
    api_url: &str,
    version: &str,
) -> Result<MariadbRelease, SourceError> {
    let text = source.get_text(api_url)?;
    let payload: MariadbVersionPayload = serde_json::from_str(&text)?;
    let mut release = payload
        .release(version)
        .ok_or_else(|| SourceError::new(format!("MariaDB payload has no release {version}")))?;
    for file in &mut release.files {
        if file.size.is_some_and(|size| size > 0) {
            continue;
        }
        // Only the installable server build needs its size resolved with a HEAD;
        // plugins and debug archives are dropped by `mariadb_from_api` anyway.
        if !is_server_build(&file.file_name)
            || mariadb_target(&file.os, &file.cpu, &file.file_name).is_none()
        {
            continue;
        }
        let url = mariadb_https_url(&file.file_download_url)?;
        file.size = Some(source.head_size(&url, super::ENCODING_IDENTITY)?);
    }
    Ok(release)
}

/// Upgrades the REST API's `http://` download URL to `https://`.
///
/// The redirector serves the same object over TLS; recording the cleartext URL
/// would fail catalog validation (an artifact URL must be HTTPS) and would let a
/// download happen off TLS.
pub fn mariadb_https_url(url: &str) -> Result<String, SourceError> {
    if let Some(rest) = url.strip_prefix("https://") {
        return Ok(format!("https://{rest}"));
    }
    if let Some(rest) = url.strip_prefix("http://") {
        return Ok(format!("https://{rest}"));
    }
    Err(SourceError::new(format!(
        "MariaDB download URL {url:?} is not absolute"
    )))
}

/// Builds MariaDB versions and artifacts from a REST API release payload.
pub fn mariadb_from_api(
    release: &MariadbRelease,
    version: &str,
) -> Result<(Vec<CatalogVersion>, Vec<CatalogArtifact>), SourceError> {
    let mut artifacts = Vec::new();
    for file in &release.files {
        // The API also lists plugin and debug-symbol tarballs for the same
        // `(os, arch)`; only the server build (whose file name carries the
        // version and no `plugin`/`debugsymbols` marker) is installable.
        if !is_server_build(&file.file_name) {
            continue;
        }
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
        let url = mariadb_https_url(&file.file_download_url)?;
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
            url,
            sha256,
            size_bytes,
            format: format.as_str().to_owned(),
            // The archive has exactly one top-level directory named after the
            // file stem; it must be stripped or `bin/` lands one level too deep.
            // Deriving it from every format (not just `.tar.gz`) keeps the
            // Windows zip correct too.
            strip_prefix: archive_root(&file.file_name),
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
        release_date: trimmed_non_empty(&release.date_of_release),
        eol: false,
    }];
    Ok((versions, artifacts))
}

fn trimmed_non_empty(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.trim().to_owned())
}

/// Whether a MariaDB file name is the server build for a target.
fn is_server_build(file_name: &str) -> bool {
    !file_name.contains("plugin")
        && !file_name.contains("debugsymbols")
        && !file_name.contains("debug-test")
}

/// Maps a MariaDB REST API file to a supported `(os, arch)` target.
pub fn mariadb_target(os: &str, cpu: &str, file_name: &str) -> Option<(Os, Arch)> {
    // MariaDB publishes no arm64 Linux tarball and no macOS build; only shows of
    // the Windows x64 and Linux x64 systemd builds are installable (W0-B).
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
        _ => return None,
    };
    Some((os, arch))
}
