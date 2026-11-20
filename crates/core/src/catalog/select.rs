//! Picking one artifact for a host.
//!
//! Upstream occasionally publishes several builds for the same `(os, arch)`
//! pair: Valkey ships `noble` and `jammy` tarballs whose only difference is the
//! glibc floor they were linked against (W0-B finding C-03). Choosing between
//! them needs the host glibc, so the choice is a catalog concern, not a
//! download concern.

use crate::catalog::validate::parse_glibc;
use crate::model::{Artifact, EngineKey};

/// Picks the best artifact for one engine version on this host.
///
/// `available` is the platform-filtered list, for example
/// [`crate::model::CatalogSnapshot::artifacts_for`]. Selection rules:
///
/// 1. Artifacts whose `min_glibc` floor is above the host glibc never win.
/// 2. Among the rest, the newest floor the host satisfies wins, so a modern
///    host gets the `noble` build and an older host gets `jammy`.
/// 3. Without a known host glibc, the lowest floor wins: most compatible.
///
/// Ties break on variant name (`jammy` before `noble`), then id, so the answer
/// is deterministic for a fixed catalog.
pub fn select_artifact<'a>(
    available: &[&'a Artifact],
    engine: EngineKey,
    version: &str,
    host_glibc: Option<(u32, u32)>,
) -> Option<&'a Artifact> {
    let mut candidates: Vec<&Artifact> = available
        .iter()
        .copied()
        .filter(|artifact| artifact.engine() == engine && artifact.version() == version)
        .collect();
    if candidates.is_empty() {
        return None;
    }

    candidates.sort_by(|left, right| {
        rank(left, host_glibc)
            .cmp(&rank(right, host_glibc))
            .then_with(|| variant_key(left).cmp(&variant_key(right)))
            .then_with(|| left.id().cmp(right.id()))
    });
    candidates.first().copied()
}

/// Sort key: lower is better. `None` floors sort above every explicit floor so
/// a build with an unknown requirement is only used when nothing else exists.
fn rank(artifact: &Artifact, host_glibc: Option<(u32, u32)>) -> (u8, u32, u32) {
    let Some(floor) = artifact.min_glibc().and_then(parse_glibc) else {
        return (2, u32::MAX, u32::MAX);
    };
    match host_glibc {
        Some(host) if floor <= host => (0, u32::MAX - floor.0, u32::MAX - floor.1),
        Some(_) => (3, floor.0, floor.1),
        None => (1, floor.0, floor.1),
    }
}

fn variant_key(artifact: &Artifact) -> (u8, &str) {
    match artifact.variant() {
        Some("jammy") => (0, "jammy"),
        Some("noble") => (1, "noble"),
        Some(other) => (2, other),
        None => (3, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ArchiveFormat;
    use crate::platform::{Arch, Os};

    fn artifact(id: &str, version: &str, variant: Option<&str>, floor: Option<&str>) -> Artifact {
        let mut artifact = Artifact::new(
            id,
            EngineKey::Valkey,
            version,
            Os::Linux,
            Arch::X86_64,
            format!("https://example.test/{id}.tar.gz"),
            "0123456789abcdef".repeat(4),
            10,
            ArchiveFormat::TarGz,
        )
        .expect("valid artifact");
        if let Some(variant) = variant {
            artifact = artifact.with_variant(variant);
        }
        if let Some(floor) = floor {
            artifact = artifact.with_min_glibc(floor);
        }
        artifact
    }

    fn valkey_builds() -> Vec<Artifact> {
        vec![
            artifact("valkey-noble", "9.2.0", Some("noble"), Some("2.38")),
            artifact("valkey-jammy", "9.2.0", Some("jammy"), Some("2.34")),
        ]
    }

    #[test]
    fn modern_host_gets_the_newest_floor() {
        let builds = valkey_builds();
        let available: Vec<&Artifact> = builds.iter().collect();
        let picked = select_artifact(&available, EngineKey::Valkey, "9.2.0", Some((2, 39)))
            .expect("jammy or noble");
        assert_eq!(picked.id(), "valkey-noble");
    }

    #[test]
    fn older_host_falls_back_to_jammy() {
        let builds = valkey_builds();
        let available: Vec<&Artifact> = builds.iter().collect();
        let picked =
            select_artifact(&available, EngineKey::Valkey, "9.2.0", Some((2, 35))).expect("jammy");
        assert_eq!(picked.id(), "valkey-jammy");
    }

    #[test]
    fn exactly_the_floor_is_enough() {
        let builds = valkey_builds();
        let available: Vec<&Artifact> = builds.iter().collect();
        assert_eq!(
            select_artifact(&available, EngineKey::Valkey, "9.2.0", Some((2, 38)))
                .expect("noble")
                .id(),
            "valkey-noble"
        );
        assert_eq!(
            select_artifact(&available, EngineKey::Valkey, "9.2.0", Some((2, 34)))
                .expect("jammy")
                .id(),
            "valkey-jammy"
        );
    }

    #[test]
    fn unknown_host_glibc_prefers_the_lowest_floor() {
        let builds = valkey_builds();
        let available: Vec<&Artifact> = builds.iter().collect();
        assert_eq!(
            select_artifact(&available, EngineKey::Valkey, "9.2.0", None)
                .expect("jammy")
                .id(),
            "valkey-jammy"
        );
    }

    #[test]
    fn a_host_below_every_floor_still_gets_a_build() {
        let builds = valkey_builds();
        let available: Vec<&Artifact> = builds.iter().collect();
        let picked = select_artifact(&available, EngineKey::Valkey, "9.2.0", Some((2, 31)))
            .expect("closest build");
        assert_eq!(picked.id(), "valkey-jammy");
    }

    #[test]
    fn missing_combinations_are_none() {
        let builds = valkey_builds();
        let available: Vec<&Artifact> = builds.iter().collect();
        assert!(select_artifact(&available, EngineKey::Valkey, "9.2.0", None).is_some());
        assert!(select_artifact(&available, EngineKey::Postgresql, "9.2.0", None).is_none());
        assert!(select_artifact(&available, EngineKey::Valkey, "9.1.0", None).is_none());
    }

    #[test]
    fn tie_break_is_stable() {
        let builds = [
            artifact("b", "9.2.0", None, None),
            artifact("a", "9.2.0", None, None),
        ];
        let available: Vec<&Artifact> = builds.iter().collect();
        assert_eq!(
            select_artifact(&available, EngineKey::Valkey, "9.2.0", None)
                .expect("a build")
                .id(),
            "a"
        );
    }
}
