//! Shared builders for catalog fixtures used across the toolchain tests.

use crate::platform::Arch;
use crate::toolchain::catalog::{
    Artifact, ArtifactOs, Format, HashOrigin, Profile, ToolVersion,
};

/// A valid artifact for `os`/`arch`/`profile` pointing at a harmless URL.
pub(crate) fn artifact(os: &str, arch: &str, profile: Option<&str>) -> Artifact {
    build_artifact(os, arch, profile, "a")
}

fn build_artifact(os: &str, arch: &str, profile: Option<&str>, sha_prefix: &str) -> Artifact {
    Artifact::new(
        parse_os(os),
        parse_arch(arch),
        profile.and_then(|p| p.parse::<Profile>().ok()),
        Format::TarGz,
        format!("https://example.test/{os}/{arch}/archive.tar.gz"),
        format!("{}", sha_prefix.repeat(64).chars().take(64).collect::<String>()),
        1024,
        "test",
        HashOrigin::Pinned,
        Vec::new(),
    )
    .expect("valid fixture artifact")
}

/// A version built from one or more artifacts.
pub(crate) fn version(
    version: &str,
    series: &str,
    lts: Option<bool>,
    artifacts: Vec<Artifact>,
) -> ToolVersion {
    ToolVersion::new(version, series, lts, artifacts)
}

fn parse_os(os: &str) -> ArtifactOs {
    match os {
        "linux" => ArtifactOs::Linux,
        "windows" => ArtifactOs::Windows,
        "any" => ArtifactOs::Any,
        other => panic!("unknown fixture os {other}"),
    }
}

fn parse_arch(arch: &str) -> Arch {
    match arch {
        "x86_64" => Arch::X86_64,
        "aarch64" => Arch::Aarch64,
        other => panic!("unknown fixture arch {other}"),
    }
}
