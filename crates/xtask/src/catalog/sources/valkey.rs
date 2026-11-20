//! Valkey builds from `download.valkey.io`.

use super::helpers::parse_sha256_sidecar;
use super::{Arch, CatalogArtifact, CatalogVersion, Os, Source, SourceError};

/// Base URL for Valkey release binaries.
pub const VALKEY_RELEASES: &str = "https://download.valkey.io/releases";

struct ValkeyBuild {
    os: Os,
    arch: Arch,
    variant: &'static str,
    min_glibc: &'static str,
    arch_token: &'static str,
    size_bytes: u64,
}

const VALKEY_BUILDS: [ValkeyBuild; 4] = [
    ValkeyBuild {
        os: Os::Linux,
        arch: Arch::X86_64,
        variant: "jammy",
        min_glibc: "2.34",
        arch_token: "x86_64",
        size_bytes: 46_759_925,
    },
    ValkeyBuild {
        os: Os::Linux,
        arch: Arch::X86_64,
        variant: "noble",
        min_glibc: "2.38",
        arch_token: "x86_64",
        size_bytes: 45_651_721,
    },
    ValkeyBuild {
        os: Os::Linux,
        arch: Arch::Aarch64,
        variant: "jammy",
        min_glibc: "2.34",
        arch_token: "arm64",
        size_bytes: 46_686_566,
    },
    ValkeyBuild {
        os: Os::Linux,
        arch: Arch::Aarch64,
        variant: "noble",
        min_glibc: "2.38",
        arch_token: "arm64",
        size_bytes: 46_430_275,
    },
];

/// Builds Valkey versions and artifacts, reading each `.sha256` sidecar.
///
/// Upstream names the ARM build `arm64`; the catalog uses `aarch64`.
pub fn valkey_from_sidecars(
    version: &str,
    source: &dyn Source,
) -> Result<(Vec<CatalogVersion>, Vec<CatalogArtifact>), SourceError> {
    let versions = vec![CatalogVersion {
        engine: "valkey".to_owned(),
        version: version.to_owned(),
        major_line: version.split('.').next().unwrap_or(version).to_owned(),
        release_date: None,
        eol: false,
    }];

    let mut artifacts = Vec::new();
    for build in VALKEY_BUILDS {
        let asset = format!(
            "valkey-{version}-{}-{}.tar.gz",
            build.variant, build.arch_token
        );
        let url = format!("{VALKEY_RELEASES}/{asset}");
        let sidecar = source.get_text(&format!("{url}.sha256"))?;
        let sha256 = parse_sha256_sidecar(&sidecar, &asset)?;
        artifacts.push(CatalogArtifact {
            id: format!(
                "valkey-{version}-{}-{}-{}",
                build.os.as_str(),
                build.arch.as_str(),
                build.variant
            ),
            engine: "valkey".to_owned(),
            version: version.to_owned(),
            os: build.os.as_str().to_owned(),
            arch: build.arch.as_str().to_owned(),
            url,
            sha256,
            size_bytes: build.size_bytes,
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
