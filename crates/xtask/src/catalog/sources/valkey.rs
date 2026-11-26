//! Valkey builds from `download.valkey.io`.
//!
//! `download.valkey.io` is behind CloudFront and has been observed serving two
//! different objects for the same URL depending on `Accept-Encoding` (W1-B
//! review round 1, finding B2): a stale cached object for `identity` and a
//! fresh one for `gzip`, each with its own matching `.sha256` sidecar. A pinned
//! hash that came from one object will fail verification when the other is
//! served, so the builder reads the sidecar *and* the `HEAD` size under both
//! encodings and refuses to record an artifact when they disagree.

use super::helpers::parse_sha256_sidecar;
use super::tracked::TrackedVersion;
use super::{
    Arch, CatalogArtifact, CatalogVersion, ENCODING_GZIP, ENCODING_IDENTITY, Os, Source,
    SourceError,
};

/// Base URL for Valkey release binaries.
pub const VALKEY_RELEASES: &str = "https://download.valkey.io/releases";

/// Which Valkey variants the Wave-1 catalog publishes.
///
/// Only `jammy` for now: its two encoding variants are identical and it has the
/// lowest glibc floor, so it runs wherever `noble` does (W0-B finding C-03).
/// Re-enabling `noble` is a one-line data change once its objects agree.
pub const WAVE_ONE_VARIANTS: [&str; 1] = ["jammy"];

struct ValkeyBuild {
    os: Os,
    arch: Arch,
    variant: &'static str,
    min_glibc: &'static str,
    arch_token: &'static str,
}

const VALKEY_BUILDS: [ValkeyBuild; 4] = [
    ValkeyBuild {
        os: Os::Linux,
        arch: Arch::X86_64,
        variant: "jammy",
        min_glibc: "2.34",
        arch_token: "x86_64",
    },
    ValkeyBuild {
        os: Os::Linux,
        arch: Arch::X86_64,
        variant: "noble",
        min_glibc: "2.38",
        arch_token: "x86_64",
    },
    ValkeyBuild {
        os: Os::Linux,
        arch: Arch::Aarch64,
        variant: "jammy",
        min_glibc: "2.34",
        arch_token: "arm64",
    },
    ValkeyBuild {
        os: Os::Linux,
        arch: Arch::Aarch64,
        variant: "noble",
        min_glibc: "2.38",
        arch_token: "arm64",
    },
];

/// The checksum and size of one object, read under one encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValkeyObject {
    /// SHA-256 from the `.sha256` sidecar.
    pub sha256: String,
    /// `Content-Length` from `HEAD`.
    pub size_bytes: u64,
}

/// Reads the sidecar and `HEAD` size of `url` under one encoding.
pub fn valkey_object(
    source: &dyn Source,
    url: &str,
    encoding: &str,
) -> Result<ValkeyObject, SourceError> {
    let asset = url
        .rsplit('/')
        .next()
        .ok_or_else(|| SourceError::new(format!("no file name in {url}")))?;
    let sidecar = source.get_text_with(&format!("{url}.sha256"), encoding)?;
    let sha256 = parse_sha256_sidecar(&sidecar, asset)?;
    let size_bytes = source.head_size(url, encoding)?;
    Ok(ValkeyObject { sha256, size_bytes })
}

/// Builds Valkey versions and artifacts for the enabled variants.
///
/// Every artifact is cross-checked under `identity` and `gzip`; a mismatch
/// fails the build naming the artifact, instead of pinning a hash that will not
/// verify later (finding B2).
pub fn valkey_from_sidecars(
    tracked: &TrackedVersion,
    variants: &[&str],
    source: &dyn Source,
) -> Result<(Vec<CatalogVersion>, Vec<CatalogArtifact>), SourceError> {
    let version = tracked.version;
    let versions = vec![CatalogVersion {
        engine: "valkey".to_owned(),
        version: version.to_owned(),
        major_line: version.split('.').next().unwrap_or(version).to_owned(),
        release_date: None,
        eol: tracked.eol,
        line: Some(tracked.line.to_owned()),
        status: tracked.status.map(|status| status.as_str().to_owned()),
        license: None,
    }];

    let mut artifacts = Vec::new();
    for build in VALKEY_BUILDS {
        if !variants.contains(&build.variant) {
            continue;
        }
        let asset = format!(
            "valkey-{version}-{}-{}.tar.gz",
            build.variant, build.arch_token
        );
        let url = format!("{VALKEY_RELEASES}/{asset}");
        let id = format!(
            "valkey-{version}-{}-{}-{}",
            build.os.as_str(),
            build.arch.as_str(),
            build.variant
        );

        let identity = valkey_object(source, &url, ENCODING_IDENTITY)?;
        let gzip = valkey_object(source, &url, ENCODING_GZIP)?;
        if identity != gzip {
            return Err(SourceError::new(format!(
                "Valkey artifact {id} is inconsistent across Accept-Encoding: \
                 identity sha {} / {} bytes, gzip sha {} / {} bytes; refusing to pin a hash \
                 that would fail verification",
                identity.sha256, identity.size_bytes, gzip.sha256, gzip.size_bytes
            )));
        }

        artifacts.push(CatalogArtifact {
            id,
            engine: "valkey".to_owned(),
            version: version.to_owned(),
            os: build.os.as_str().to_owned(),
            arch: build.arch.as_str().to_owned(),
            url,
            sha256: identity.sha256,
            size_bytes: identity.size_bytes,
            format: "tar.gz".to_owned(),
            strip_prefix: Some(format!(
                "valkey-{version}-{}-{}",
                build.variant, build.arch_token
            )),
            min_glibc: Some(build.min_glibc.to_owned()),
            variant: Some(build.variant.to_owned()),
        });
    }
    Ok((versions, artifacts))
}
