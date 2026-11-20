//! Downloadable engine artifacts and their validation.

use std::str::FromStr;

use crate::DblError;
use crate::model::EngineKey;
use crate::platform::{Arch, Os};

/// Packaging of a published engine artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ArchiveFormat {
    /// `.tar.gz`.
    TarGz,
    /// `.tar.xz`.
    TarXz,
    /// `.zip`.
    Zip,
}

impl ArchiveFormat {
    /// Identifier used in the catalog JSON.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TarGz => "tar.gz",
            Self::TarXz => "tar.xz",
            Self::Zip => "zip",
        }
    }
}

impl FromStr for ArchiveFormat {
    type Err = DblError;

    fn from_str(value: &str) -> Result<Self, DblError> {
        match value {
            "tar.gz" => Ok(Self::TarGz),
            "tar.xz" => Ok(Self::TarXz),
            "zip" => Ok(Self::Zip),
            other => Err(DblError::InvalidInput {
                field: "format",
                reason: format!("unknown archive format: {other}"),
            }),
        }
    }
}

/// One downloadable engine build for one `(os, arch)` pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    id: String,
    engine: EngineKey,
    version: String,
    os: Os,
    arch: Arch,
    url: String,
    sha256: String,
    size_bytes: u64,
    format: ArchiveFormat,
    strip_prefix: Option<String>,
    min_glibc: Option<String>,
    variant: Option<String>,
}

impl Artifact {
    /// Builds an artifact, rejecting entries that cannot be installed.
    ///
    /// AGENTS.md invariants 3, 6 and 8: a catalog entry is only installable
    /// when it carries a SHA-256, a real size and an HTTPS URL. Anything else
    /// is rejected here rather than at download time.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        engine: EngineKey,
        version: impl Into<String>,
        os: Os,
        arch: Arch,
        url: impl Into<String>,
        sha256: impl Into<String>,
        size_bytes: u64,
        format: ArchiveFormat,
    ) -> crate::Result<Self> {
        let sha256 = sha256.into();
        if !is_sha256_hex(&sha256) {
            return Err(DblError::InvalidInput {
                field: "sha256",
                reason: "must be 64 lowercase hex characters".to_owned(),
            });
        }
        let url = url.into();
        if !is_https_url(&url) {
            return Err(DblError::InvalidInput {
                field: "url",
                reason: "must be an absolute https URL".to_owned(),
            });
        }
        if size_bytes == 0 {
            return Err(DblError::InvalidInput {
                field: "size_bytes",
                reason: "must be greater than zero".to_owned(),
            });
        }
        Ok(Self {
            id: id.into(),
            engine,
            version: version.into(),
            os,
            arch,
            url,
            sha256,
            size_bytes,
            format,
            strip_prefix: None,
            min_glibc: None,
            variant: None,
        })
    }

    /// Artifact identifier.
    pub fn id(&self) -> &str {
        &self.id
    }
    /// Engine the artifact unpacks.
    pub const fn engine(&self) -> EngineKey {
        self.engine
    }
    /// Version tag.
    pub fn version(&self) -> &str {
        &self.version
    }
    /// Target OS.
    pub const fn os(&self) -> Os {
        self.os
    }
    /// Target architecture.
    pub const fn arch(&self) -> Arch {
        self.arch
    }
    /// Download URL.
    pub fn url(&self) -> &str {
        &self.url
    }
    /// Expected SHA-256, lowercase hex.
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
    /// Expected size in bytes, used for progress and free-space checks.
    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }
    /// Archive format.
    pub const fn format(&self) -> ArchiveFormat {
        self.format
    }
    /// Directory to strip while extracting.
    pub fn strip_prefix(&self) -> Option<&str> {
        self.strip_prefix.as_deref()
    }
    /// Minimum glibc version required on Linux.
    pub fn min_glibc(&self) -> Option<&str> {
        self.min_glibc.as_deref()
    }
    /// Upstream build variant, for example Valkey's `jammy` or `noble`.
    pub fn variant(&self) -> Option<&str> {
        self.variant.as_deref()
    }

    /// Returns a copy with a strip prefix.
    pub fn with_strip_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.strip_prefix = Some(prefix.into());
        self
    }

    /// Returns a copy with a minimum glibc requirement.
    pub fn with_min_glibc(mut self, version: impl Into<String>) -> Self {
        self.min_glibc = Some(version.into());
        self
    }

    /// Returns a copy with an upstream build variant.
    pub fn with_variant(mut self, variant: impl Into<String>) -> Self {
        self.variant = Some(variant.into());
        self
    }
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Whether `value` is an absolute URL whose scheme is exactly `https`.
///
/// Compared case-sensitively and without a scheme allowlist elsewhere: the only
/// acceptable transport is TLS (AGENTS.md invariant 6).
fn is_https_url(value: &str) -> bool {
    let Some((scheme, rest)) = value.split_once("://") else {
        return false;
    };
    scheme == "https" && !rest.is_empty() && !rest.contains(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha() -> String {
        "a".repeat(64)
    }

    #[test]
    fn artifact_rejects_bad_checksum() {
        let bad = Artifact::new(
            "a1",
            EngineKey::Postgresql,
            "18.7",
            Os::Linux,
            Arch::X86_64,
            "https://example.test/pg.tar.gz",
            "not-a-hash",
            10,
            ArchiveFormat::TarGz,
        );
        assert!(bad.is_err());

        let ok = Artifact::new(
            "a1",
            EngineKey::Postgresql,
            "18.7",
            Os::Linux,
            Arch::X86_64,
            "https://example.test/pg.tar.gz",
            sha(),
            10,
            ArchiveFormat::TarGz,
        );
        assert!(ok.is_ok());
    }

    #[test]
    fn artifact_rejects_non_https_schemes() {
        for url in [
            "http://example.test/pg.tar.gz",
            "file:///etc/passwd",
            "ftp://example.test/pg.tar.gz",
            "example.test/pg.tar.gz",
            "https://",
            "",
            "HTTPS://example.test/pg.tar.gz",
            "https://example.test/pg file.tar.gz",
        ] {
            let result = Artifact::new(
                "a1",
                EngineKey::Postgresql,
                "18.7",
                Os::Linux,
                Arch::X86_64,
                url,
                sha(),
                10,
                ArchiveFormat::TarGz,
            );
            match result {
                Err(DblError::InvalidInput { field, .. }) => {
                    assert_eq!(field, "url", "wrong field rejected for {url}");
                }
                other => panic!("expected url rejection for {url:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn artifact_rejects_zero_size() {
        let result = Artifact::new(
            "a1",
            EngineKey::Postgresql,
            "18.7",
            Os::Linux,
            Arch::X86_64,
            "https://example.test/pg.tar.gz",
            sha(),
            0,
            ArchiveFormat::TarGz,
        );
        match result {
            Err(DblError::InvalidInput { field, .. }) => assert_eq!(field, "size_bytes"),
            other => panic!("expected size_bytes rejection, got {other:?}"),
        }
    }

    #[test]
    fn artifact_accepts_a_well_formed_entry() {
        let artifact = Artifact::new(
            "a1",
            EngineKey::Postgresql,
            "18.7",
            Os::Linux,
            Arch::X86_64,
            "https://example.test/pg-18.7.tar.gz",
            sha(),
            1,
            ArchiveFormat::TarGz,
        );
        assert!(artifact.is_ok());
    }

    #[test]
    fn archive_formats_round_trip() {
        for format in [
            ArchiveFormat::TarGz,
            ArchiveFormat::TarXz,
            ArchiveFormat::Zip,
        ] {
            assert_eq!(format.as_str().parse::<ArchiveFormat>().ok(), Some(format));
        }
        assert!("rar".parse::<ArchiveFormat>().is_err());
    }

    #[test]
    fn artifact_accessors_and_builders() {
        let artifact = Artifact::new(
            "a1",
            EngineKey::Postgresql,
            "18.7",
            Os::Linux,
            Arch::X86_64,
            "https://example.test/pg.tar.gz",
            sha(),
            10,
            ArchiveFormat::TarGz,
        )
        .expect("valid artifact")
        .with_strip_prefix("pgsql/")
        .with_min_glibc("2.28");
        assert_eq!(artifact.id(), "a1");
        assert_eq!(artifact.engine(), EngineKey::Postgresql);
        assert_eq!(artifact.version(), "18.7");
        assert_eq!(artifact.os(), Os::Linux);
        assert_eq!(artifact.arch(), Arch::X86_64);
        assert_eq!(artifact.url(), "https://example.test/pg.tar.gz");
        assert_eq!(artifact.sha256(), sha());
        assert_eq!(artifact.size_bytes(), 10);
        assert_eq!(artifact.format(), ArchiveFormat::TarGz);
        assert_eq!(artifact.strip_prefix(), Some("pgsql/"));
        assert_eq!(artifact.min_glibc(), Some("2.28"));
        assert_eq!(artifact.variant(), None);
        assert_eq!(ArchiveFormat::TarGz.as_str(), "tar.gz");
    }

    #[test]
    fn artifact_variant_builder() {
        let artifact = Artifact::new(
            "v1",
            EngineKey::Valkey,
            "9.2.0",
            Os::Linux,
            Arch::X86_64,
            "https://example.test/valkey.tar.gz",
            sha(),
            10,
            ArchiveFormat::TarGz,
        )
        .expect("valid artifact")
        .with_variant("jammy");
        assert_eq!(artifact.variant(), Some("jammy"));
    }
}
