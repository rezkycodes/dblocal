//! Resolving a user version spec (`8`, `8.4`, `8.4.15`, `latest`) to one version.

use crate::DblError;
use crate::platform::{Arch, Os};
use crate::toolchain::error::ToolchainError;

use super::{Profile, ToolEntry, ToolId, ToolVersion, compare_versions};

/// A parsed user version specification.
///
/// The spec is just a string with a special case for `latest`; it is validated
/// lazily against the catalog by [`resolve_version`] so an unknown spec produces
/// an error that names what *is* available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionSpec {
    /// The newest version of the tool.
    Latest,
    /// A concrete prefix or exact version, e.g. `8`, `8.4` or `8.4.15`.
    Tag(String),
}

impl VersionSpec {
    /// Parses a spec. Empty input and whitespace are rejected here, not later.
    pub fn parse(raw: &str) -> crate::Result<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(DblError::InvalidInput {
                field: "version",
                reason: "must not be empty (try 'latest' or a series such as '8.4')".to_owned(),
            });
        }
        if trimmed.eq_ignore_ascii_case("latest") {
            return Ok(Self::Latest);
        }
        let valid = trimmed.len() <= 32
            && trimmed.starts_with(|c: char| c.is_ascii_digit())
            && trimmed
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
        if !valid {
            return Err(DblError::InvalidInput {
                field: "version",
                reason: format!("'{trimmed}' is not a version, series or 'latest'"),
            });
        }
        Ok(Self::Tag(trimmed.to_owned()))
    }

    /// The tag text, or `latest` for [`VersionSpec::Latest`].
    pub fn as_str(&self) -> &str {
        match self {
            Self::Latest => "latest",
            Self::Tag(tag) => tag,
        }
    }
}

/// Picks the version of `tool` that `spec` names.
///
/// Order of preference, newest first within each rule:
/// 1. `latest` — the newest version;
/// 2. an exact match (`8.4.15`);
/// 3. a dotted prefix (`8` matches `8.4.15`, `8.4` matches `8.4.15`);
/// 4. a series match (`8.4` matches a version whose series is `8.4`).
///
/// A spec that matches nothing is an [`DblError::NotFound`] listing the available
/// versions, because "8.5 is not in this catalog" is a data fact, not bad input.
pub fn resolve_version<'a>(
    tool: ToolId,
    entry: &'a ToolEntry,
    spec: &VersionSpec,
) -> crate::Result<&'a ToolVersion> {
    let versions = entry.versions();
    if versions.is_empty() {
        return Err(DblError::NotFound {
            kind: tool.as_str(),
            name: "any version".to_owned(),
        });
    }
    if matches!(spec, VersionSpec::Latest) {
        return Ok(newest(versions));
    }
    let tag = spec.as_str();

    if let Some(exact) = versions.iter().find(|v| v.version() == tag) {
        return Ok(exact);
    }
    if let Some(prefixed) = versions
        .iter()
        .filter(|v| dotted_prefix(v.version(), tag))
        .max_by(|a, b| compare_versions(a.version(), b.version()))
    {
        return Ok(prefixed);
    }
    if let Some(series) = versions.iter().find(|v| v.series() == tag) {
        return Ok(series);
    }
    Err(DblError::NotFound {
        kind: tool.as_str(),
        name: format!(
            "{tag} (available: {})",
            available_summary(versions)
        ),
    })
}

/// Picks the version of `tool` that `spec` names AND that has an installable
/// artifact for `(os, arch, profile)`.
///
/// Versions without an artifact for this platform are invisible to resolution:
/// e.g. when PHP `8.5.13` exists only for Windows while `8.5.8` exists for
/// Linux, `php 8.5` resolves to `8.5.8` on Linux and `8.5.13` on Windows.
///
/// If `spec` matches a version in the catalog on another platform (such as
/// `php 7.4` on Linux, which is Windows-only), this returns
/// [`ToolchainError::UnavailableOnPlatform`] with a clear explanation instead
/// of claiming the version does not exist at all.
pub fn resolve_version_for_platform<'a>(
    tool: ToolId,
    entry: &'a ToolEntry,
    spec: &VersionSpec,
    os: Os,
    arch: Arch,
    profile: Option<Profile>,
) -> crate::Result<&'a ToolVersion> {
    let available: Vec<&'a ToolVersion> = entry
        .versions()
        .iter()
        .filter(|v| v.select_artifact(os, arch, profile).is_ok())
        .collect();

    if let Some(found) = match_spec(&available, spec) {
        return Ok(found);
    }

    // Check whether the spec exists on another platform/profile so we can give
    // a clear "not available on this platform" error.
    let all_refs: Vec<&'a ToolVersion> = entry.versions().iter().collect();
    if match_spec(&all_refs, spec).is_some() {
        let profile_note = profile
            .map(|p| format!(" with the {} profile", p.as_str()))
            .unwrap_or_default();
        return Err(ToolchainError::UnavailableOnPlatform {
            tool: tool.as_str(),
            version: spec.as_str().to_owned(),
            reason: format!("{os}-{arch}{profile_note}"),
        }
        .into_dbl_error());
    }

    if available.is_empty() {
        return Err(DblError::NotFound {
            kind: tool.as_str(),
            name: "any version".to_owned(),
        });
    }

    Err(DblError::NotFound {
        kind: tool.as_str(),
        name: format!(
            "{} (available: {})",
            spec.as_str(),
            available_summary_refs(&available)
        ),
    })
}

fn match_spec<'a>(versions: &[&'a ToolVersion], spec: &VersionSpec) -> Option<&'a ToolVersion> {
    if versions.is_empty() {
        return None;
    }
    if matches!(spec, VersionSpec::Latest) {
        return versions
            .iter()
            .copied()
            .max_by(|a, b| compare_versions(a.version(), b.version()));
    }
    let tag = spec.as_str();
    if let Some(&exact) = versions.iter().find(|v| v.version() == tag) {
        return Some(exact);
    }
    if let Some(prefixed) = versions
        .iter()
        .copied()
        .filter(|v| dotted_prefix(v.version(), tag))
        .max_by(|a, b| compare_versions(a.version(), b.version()))
    {
        return Some(prefixed);
    }
    versions
        .iter()
        .copied()
        .filter(|v| v.series() == tag)
        .max_by(|a, b| compare_versions(a.version(), b.version()))
}

fn available_summary_refs(versions: &[&ToolVersion]) -> String {
    let mut series: Vec<&str> = versions.iter().map(|v| v.series()).collect();
    series.sort_unstable();
    series.dedup();
    series.reverse();
    series.join(", ")
}

fn newest(versions: &[ToolVersion]) -> &ToolVersion {
    versions
        .iter()
        .max_by(|a, b| compare_versions(a.version(), b.version()))
        .unwrap_or(&versions[0])
}

/// Whether `version` starts with `tag` followed by a dot (a real sub-version).
fn dotted_prefix(version: &str, tag: &str) -> bool {
    version
        .strip_prefix(tag)
        .is_some_and(|rest| rest.starts_with('.'))
}

/// A short "8.5, 8.4, 8.3" style list of the series present, for error text.
fn available_summary(versions: &[ToolVersion]) -> String {
    let mut series: Vec<&str> = versions.iter().map(ToolVersion::series).collect();
    series.sort_unstable();
    series.dedup();
    series.reverse();
    series.join(", ")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::toolchain::catalog::{ToolEntry, ToolId};
    use crate::toolchain::catalog_test_support::{artifact, version};

    fn php_entry() -> ToolEntry {
        ToolEntry::new(
            ToolId::Php,
            vec![
                version("8.5.1", "8.5", None, vec![artifact("linux", "x86_64", Some("common"))]),
                version("8.4.15", "8.4", None, vec![artifact("linux", "x86_64", Some("common"))]),
                version("8.4.9", "8.4", None, vec![artifact("linux", "x86_64", Some("common"))]),
                version("8.3.2", "8.3", None, vec![artifact("linux", "x86_64", Some("common"))]),
            ],
        )
    }

    #[test]
    fn latest_is_newest() {
        let entry = php_entry();
        let resolved = resolve_version(ToolId::Php, &entry, &VersionSpec::Latest).expect("latest");
        assert_eq!(resolved.version(), "8.5.1");
    }

    #[test]
    fn exact_match_wins() {
        let entry = php_entry();
        let spec = VersionSpec::parse("8.4.9").expect("spec");
        let resolved = resolve_version(ToolId::Php, &entry, &spec).expect("exact");
        assert_eq!(resolved.version(), "8.4.9");
    }

    #[test]
    fn series_resolves_to_newest_patch() {
        let entry = php_entry();
        for text in ["8.4", "8"] {
            let spec = VersionSpec::parse(text).expect("spec");
            let resolved = resolve_version(ToolId::Php, &entry, &spec).expect("series");
            let expected = if text == "8.4" { "8.4.15" } else { "8.5.1" };
            assert_eq!(resolved.version(), expected, "spec {text}");
        }
    }

    #[test]
    fn missing_series_names_what_is_available() {
        let entry = php_entry();
        let spec = VersionSpec::parse("7.4").expect("spec");
        let err = resolve_version(ToolId::Php, &entry, &spec).expect_err("missing");
        let message = err.to_string();
        assert!(message.contains("7.4"), "{message}");
        assert!(message.contains("8.5"), "{message}");
    }

    #[test]
    fn empty_and_garbage_specs_are_rejected() {
        assert!(VersionSpec::parse("").is_err());
        assert!(VersionSpec::parse("   ").is_err());
        assert!(VersionSpec::parse("php").is_err());
        assert!(VersionSpec::parse("8 4").is_err());
        assert_eq!(
            VersionSpec::parse("LATEST").expect("latest"),
            VersionSpec::Latest
        );
    }

    #[test]
    fn diverging_patch_versions_resolve_per_os_and_windows_only_series_reports_unavailable() {
        // Mirrors the real catalog: 8.5.13 is Windows-only while 8.5.8 is Linux-only;
        // 7.4.33 is Windows-only.
        let entry = ToolEntry::new(
            ToolId::Php,
            vec![
                version("8.5.13", "8.5", None, vec![artifact("windows", "x86_64", None)]),
                version(
                    "8.5.8",
                    "8.5",
                    None,
                    vec![
                        artifact("linux", "x86_64", Some("common")),
                        artifact("linux", "x86_64", Some("bulk")),
                    ],
                ),
                version("7.4.33", "7.4", None, vec![artifact("windows", "x86_64", None)]),
            ],
        );

        let spec_85 = VersionSpec::parse("8.5").expect("spec");
        let on_linux = resolve_version_for_platform(
            ToolId::Php,
            &entry,
            &spec_85,
            Os::Linux,
            Arch::X86_64,
            Some(Profile::Common),
        )
        .expect("linux 8.5");
        assert_eq!(on_linux.version(), "8.5.8");

        let on_windows = resolve_version_for_platform(
            ToolId::Php,
            &entry,
            &spec_85,
            Os::Windows,
            Arch::X86_64,
            None,
        )
        .expect("windows 8.5");
        assert_eq!(on_windows.version(), "8.5.13");

        let spec_74 = VersionSpec::parse("7.4").expect("spec");
        let err = resolve_version_for_platform(
            ToolId::Php,
            &entry,
            &spec_74,
            Os::Linux,
            Arch::X86_64,
            Some(Profile::Common),
        )
        .expect_err("7.4 unavailable on linux");
        assert!(
            err.to_string().contains("not available on this platform"),
            "{err}"
        );
    }
}
