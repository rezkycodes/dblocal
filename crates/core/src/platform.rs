//! OS and architecture detection used to pick catalog artifacts.

use std::fmt;
use std::str::FromStr;

use crate::error::{DblError, Result};

/// Operating system family a catalog artifact can target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Os {
    /// Linux distributions.
    Linux,
    /// macOS 13 and newer, universal binary.
    Macos,
    /// Windows 10 22H2 and newer.
    Windows,
}

impl Os {
    /// Lowercase identifier used in the catalog JSON.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Macos => "macos",
            Self::Windows => "windows",
        }
    }

    /// Detects the OS this build is running on.
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Macos
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

impl fmt::Display for Os {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Os {
    type Err = DblError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "linux" => Ok(Self::Linux),
            "macos" => Ok(Self::Macos),
            "windows" => Ok(Self::Windows),
            other => Err(DblError::InvalidInput {
                field: "os",
                reason: format!("unknown os: {other}"),
            }),
        }
    }
}

/// CPU architecture a catalog artifact can target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Arch {
    /// 64-bit x86.
    X86_64,
    /// 64-bit ARM.
    Aarch64,
}

impl Arch {
    /// Identifier used in the catalog JSON.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
        }
    }

    /// Detects the architecture this build is running on.
    pub const fn current() -> Self {
        if cfg!(target_arch = "aarch64") {
            Self::Aarch64
        } else {
            Self::X86_64
        }
    }
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Arch {
    type Err = DblError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "x86_64" => Ok(Self::X86_64),
            "aarch64" => Ok(Self::Aarch64),
            other => Err(DblError::InvalidInput {
                field: "arch",
                reason: format!("unknown arch: {other}"),
            }),
        }
    }
}

/// The OS and architecture pair of the machine running DBLocal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Platform {
    /// Operating system.
    pub os: Os,
    /// CPU architecture.
    pub arch: Arch,
}

impl Platform {
    /// Creates a platform from its parts.
    pub const fn new(os: Os, arch: Arch) -> Self {
        Self { os, arch }
    }

    /// Detects the platform of this build.
    pub const fn current() -> Self {
        Self::new(Os::current(), Arch::current())
    }

    /// Whether a catalog artifact for `(os, arch)` can run here.
    pub fn accepts(&self, os: Os, arch: Arch) -> bool {
        self.os == os && self.arch == arch
    }

    /// Whether this platform matches macOS universal builds.
    pub fn is_universal_macos(&self) -> bool {
        self.os == Os::Macos
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.os, self.arch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_str() {
        for os in [Os::Linux, Os::Macos, Os::Windows] {
            assert_eq!(os.as_str().parse::<Os>().ok(), Some(os));
        }
        for arch in [Arch::X86_64, Arch::Aarch64] {
            assert_eq!(arch.as_str().parse::<Arch>().ok(), Some(arch));
        }
        assert!(matches!(
            "beos".parse::<Os>(),
            Err(DblError::InvalidInput { .. })
        ));
        assert!(matches!(
            "mips".parse::<Arch>(),
            Err(DblError::InvalidInput { .. })
        ));
    }

    #[test]
    fn current_platform_is_known() {
        let platform = Platform::current();
        assert!(platform.accepts(Os::current(), Arch::current()));
        assert!(!platform.accepts(Os::Windows, Arch::Aarch64) || cfg!(windows));
    }

    #[test]
    fn display_matches_catalog_identifiers() {
        assert_eq!(
            Platform::new(Os::Linux, Arch::X86_64).to_string(),
            "linux-x86_64"
        );
        assert_eq!(Os::Macos.to_string(), "macos");
    }
}
