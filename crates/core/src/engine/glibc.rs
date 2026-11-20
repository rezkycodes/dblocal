//! Host glibc detection, used to pick a Valkey runtime variant (finding C-03).
//!
//! The Ubuntu `jammy` tarball requires `GLIBC_2.34`; `noble` requires
//! `GLIBC_2.38`. On a host with glibc in `[2.34, 2.38)` only `jammy` runs, so
//! the catalog asks this module before offering a variant. Parsing is a pure
//! function over `ldd --version` output; [`host_version`] is the thin runner.

use std::process::Command;

use crate::engine::uri::exe_name;

/// A parsed `major.minor` glibc version, with an optional patch level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    /// Major version, always `2` on glibc.
    pub major: u32,
    /// Minor version, the part that actually moves.
    pub minor: u32,
    /// Patch level when the output carries one.
    pub patch: u32,
}

impl Version {
    /// Builds a version.
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)?;
        if self.patch != 0 {
            write!(f, ".{}", self.patch)?;
        }
        Ok(())
    }
}

/// Lowest glibc the Valkey `jammy` tarball requires (`GLIBC_2.34`).
pub const JAMMY_FLOOR: Version = Version::new(2, 34, 0);
/// Lowest glibc the Valkey `noble` tarball requires (`GLIBC_2.38`).
pub const NOBLE_FLOOR: Version = Version::new(2, 38, 0);

/// Which Ubuntu build of Valkey can run on this host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValkeyVariant {
    /// `valkey-<ver>-jammy-<arch>`: needs `GLIBC_2.34`.
    Jammy,
    /// `valkey-<ver>-noble-<arch>`: needs `GLIBC_2.38`.
    Noble,
}

impl ValkeyVariant {
    /// Stable identifier used in catalog artifact names.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Jammy => "jammy",
            Self::Noble => "noble",
        }
    }

    /// Lowest glibc this variant requires.
    pub const fn floor(self) -> Version {
        match self {
            Self::Jammy => JAMMY_FLOOR,
            Self::Noble => NOBLE_FLOOR,
        }
    }
}

/// Chooses the Valkey variant for a host glibc version.
///
/// Returns `None` when the host is older than both floors, so the catalog can
/// mark Valkey unavailable instead of installing a binary that cannot start.
pub fn valkey_variant(version: Version) -> Option<ValkeyVariant> {
    if version >= NOBLE_FLOOR {
        Some(ValkeyVariant::Noble)
    } else if version >= JAMMY_FLOOR {
        Some(ValkeyVariant::Jammy)
    } else {
        None
    }
}

/// Extracts a glibc version from `ldd --version` output.
///
/// Accepts the Fedora/upstream shape `ldd (GNU libc) 2.43`, the Ubuntu shape
/// `ldd (Ubuntu GLIBC 2.35-0ubuntu3.8) 2.35` and any line whose last
/// `major.minor[.patch]` token is the version. `musl` output carries no glibc
/// version, so it returns `None`.
pub fn parse_ldd_version(output: &str) -> Option<Version> {
    for line in output.lines() {
        let trimmed = line.trim();
        let looks_like_ldd = trimmed.starts_with("ldd");
        let looks_like_glibc = trimmed.to_ascii_lowercase().contains("glibc");
        if !looks_like_ldd && !looks_like_glibc {
            continue;
        }
        if let Some(version) = last_version_token(trimmed) {
            return Some(version);
        }
    }
    None
}

fn last_version_token(line: &str) -> Option<Version> {
    line.split_whitespace()
        .filter_map(parse_version_token)
        .next_back()
}

fn parse_version_token(token: &str) -> Option<Version> {
    let token = token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.');
    let mut parts = token.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    let patch = parts
        .next()
        .and_then(|raw| {
            let digits: String = raw.chars().take_while(char::is_ascii_digit).collect();
            digits.parse::<u32>().ok()
        })
        .unwrap_or(0);
    Some(Version::new(major, minor, patch))
}

/// Detects the host glibc version by running `ldd --version`.
///
/// Returns `None` when `ldd` is missing or reports a non-glibc libc.
pub fn host_version() -> Option<Version> {
    let output = Command::new(exe_name("ldd")).arg("--version").output();
    let output = output.ok()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push('\n');
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    parse_ldd_version(&text)
}

/// Chooses the Valkey variant for the host, if a glibc can be detected.
pub fn host_valkey_variant() -> Option<ValkeyVariant> {
    host_version().and_then(valkey_variant)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fedora_shape() {
        let out = "ldd (GNU libc) 2.43\nCopyright (C) 2024 Free Software Foundation, Inc.\n";
        assert_eq!(parse_ldd_version(out), Some(Version::new(2, 43, 0)));
    }

    #[test]
    fn parses_ubuntu_packaged_shape() {
        let out = "ldd (Ubuntu GLIBC 2.35-0ubuntu3.8) 2.35\nCopyright (C) 2022 FSF.\n";
        assert_eq!(parse_ldd_version(out), Some(Version::new(2, 35, 0)));
    }

    #[test]
    fn parses_three_part_version() {
        let out = "ldd (GNU libc) 2.38.1\n";
        assert_eq!(parse_ldd_version(out), Some(Version::new(2, 38, 1)));
    }

    #[test]
    fn rejects_musl_and_garbage() {
        assert_eq!(
            parse_ldd_version("musl libc (x86_64)\nVersion 1.2.5\n"),
            None
        );
        assert_eq!(parse_ldd_version(""), None);
        assert_eq!(parse_ldd_version("command not found\n"), None);
    }

    #[test]
    fn chooses_variant_by_floor() {
        assert_eq!(
            valkey_variant(Version::new(2, 43, 0)),
            Some(ValkeyVariant::Noble)
        );
        assert_eq!(
            valkey_variant(Version::new(2, 38, 0)),
            Some(ValkeyVariant::Noble)
        );
        assert_eq!(
            valkey_variant(Version::new(2, 37, 9)),
            Some(ValkeyVariant::Jammy)
        );
        assert_eq!(
            valkey_variant(Version::new(2, 34, 0)),
            Some(ValkeyVariant::Jammy)
        );
        assert_eq!(valkey_variant(Version::new(2, 33, 0)), None);
        assert_eq!(valkey_variant(Version::new(2, 31, 0)), None);
    }

    #[test]
    fn variant_identifiers_match_artifact_tokens() {
        assert_eq!(ValkeyVariant::Jammy.as_str(), "jammy");
        assert_eq!(ValkeyVariant::Noble.as_str(), "noble");
        assert_eq!(ValkeyVariant::Jammy.floor(), JAMMY_FLOOR);
        assert_eq!(ValkeyVariant::Noble.floor(), NOBLE_FLOOR);
    }

    #[test]
    fn version_display_is_stable() {
        assert_eq!(Version::new(2, 43, 0).to_string(), "2.43");
        assert_eq!(Version::new(2, 38, 1).to_string(), "2.38.1");
    }

    #[test]
    fn host_detection_is_optional_not_fatal() {
        // The host may be a non-glibc one; the call must simply not panic.
        let _ = host_version();
        let _ = host_valkey_variant();
    }
}
