//! The toolchain layout under the DBLocal data directory.
//!
//! Everything lives below the same home the engines use, resolved through
//! [`crate::paths::DblPaths`] so `~/.dblocal` (or `DBLOCAL_HOME`) is honoured on
//! every platform without a second, hardcoded notion of the data dir:
//!
//! ```text
//! <root>/toolchains/active.json                    the active-version choice
//! <root>/toolchains/<tool>/<version>[-<profile>]/   one installed version
//! <root>/toolchains/php/conf.d/99-dblocal.ini       managed PHP settings
//! <root>/toolchains/.tmp/                           in-flight downloads/staging
//! <root>/bin/<shim>                                 generated launchers
//! ```
//!
//! [`ToolchainPaths`] is a thin, borrowed view; it never creates anything, so it is
//! cheap to build in a read path.

use std::path::{Component, Path, PathBuf};

use crate::DblError;
use crate::paths::DblPaths;
use crate::platform::Os;

use super::catalog::ToolId;
use super::consts;

/// The toolchain subtree of one DBLocal home.
#[derive(Debug, Clone)]
pub struct ToolchainPaths {
    root: PathBuf,
}

impl ToolchainPaths {
    /// Builds the view from the shared home resolver.
    pub fn from_paths(paths: &DblPaths) -> Self {
        Self {
            root: paths.root().to_path_buf(),
        }
    }

    /// Builds the view from an explicit home root (tests).
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The DBLocal home root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `<root>/toolchains`.
    pub fn toolchains_dir(&self) -> PathBuf {
        self.root.join("toolchains")
    }

    /// `<root>/toolchains/<tool>`.
    pub fn tool_dir(&self, tool: ToolId) -> PathBuf {
        self.toolchains_dir().join(tool.as_str())
    }

    /// `<root>/toolchains/<tool>/<version>[-<profile>]`.
    ///
    /// A profile is appended for a PHP Linux build only, exactly as the install
    /// writes it, so this is the single place that decides the suffix.
    pub fn install_dir(&self, tool: ToolId, version: &str, profile: Option<&str>) -> PathBuf {
        let leaf = match profile {
            Some(profile) if !profile.is_empty() => format!("{version}-{profile}"),
            _ => version.to_owned(),
        };
        self.tool_dir(tool).join(leaf)
    }

    /// A checked install directory, rejecting unsafe version/profile segments.
    pub fn checked_install_dir(
        &self,
        tool: ToolId,
        version: &str,
        profile: Option<&str>,
    ) -> crate::Result<PathBuf> {
        validate_segment("version", version)?;
        if let Some(profile) = profile {
            validate_segment("profile", profile)?;
        }
        Ok(self.install_dir(tool, version, profile))
    }

    /// `<root>/toolchains/.tmp`, where downloads and staging directories live.
    ///
    /// Kept inside `toolchains/` so a crash never leaves a half-extracted tree in
    /// a place a `list` would mistake for an installed version (the scan ignores
    /// dot-prefixed entries).
    pub fn tmp_dir(&self) -> PathBuf {
        self.toolchains_dir().join(consts::TMP_DIR_NAME)
    }

    /// `<root>/toolchains/active.json`.
    pub fn active_registry(&self) -> PathBuf {
        self.toolchains_dir().join(consts::ACTIVE_REGISTRY_FILE)
    }

    /// Per-install manifest written after a successful install.
    pub fn install_manifest(&self, install_dir: &Path) -> PathBuf {
        install_dir.join(consts::INSTALL_MANIFEST_FILE)
    }

    /// `<root>/bin`, where the generated shims live.
    pub fn shims_dir(&self) -> PathBuf {
        self.root.join(consts::SHIMS_DIR_NAME)
    }

    /// A shim path for `name` on `os` (`.cmd` on Windows, plain otherwise).
    pub fn shim_path(&self, name: &str, os: Os) -> PathBuf {
        let file = match os {
            Os::Windows => format!("{name}.cmd"),
            _ => name.to_owned(),
        };
        self.shims_dir().join(file)
    }

    /// Lock file for one toolchain install/remove operation.
    pub fn lock_file(&self, tool: ToolId, version: &str, profile: Option<&str>) -> PathBuf {
        let leaf = match profile {
            Some(p) if !p.is_empty() => format!("{version}-{p}"),
            _ => version.to_owned(),
        };
        self.toolchains_dir()
            .join(".locks")
            .join(format!("{}-{leaf}.lock", tool.as_str()))
    }

    /// `<root>/toolchains/php/conf.d`.
    pub fn php_conf_dir(&self, tool: ToolId) -> PathBuf {
        self.tool_dir(tool).join(consts::PHP_CONF_DIR_NAME)
    }

    /// `<root>/toolchains/php/conf.d/99-dblocal.ini`.
    pub fn php_managed_ini(&self) -> PathBuf {
        self.php_conf_dir(ToolId::Php)
            .join(consts::PHP_MANAGED_INI_FILE)
    }

    /// `<root>/toolchains/php/php.ini` (the managed base ini, Windows only).
    pub fn php_base_ini(&self) -> PathBuf {
        self.tool_dir(ToolId::Php).join(consts::PHP_BASE_INI_FILE)
    }

    /// The refreshed catalog cache: `<root>/cache/toolchains.json`.
    pub fn catalog_cache(&self) -> PathBuf {
        self.root
            .join("cache")
            .join(format!("toolchains-{}", consts::CATALOG_CACHE_FILE))
    }

    /// Confines `candidate` to the toolchains subtree lexically, rejecting `..`
    /// and escaping paths. Symlink checks are the caller's job (they need the
    /// filesystem), but nothing may reach outside `toolchains/`.
    pub fn ensure_within_toolchains(&self, candidate: &Path) -> crate::Result<()> {
        let base = normalize(&self.toolchains_dir());
        let target = normalize(candidate);
        if !target.starts_with(&base) {
            return Err(DblError::UnsafeArchive {
                entry: candidate.display().to_string(),
            });
        }
        Ok(())
    }
}

fn validate_segment(field: &'static str, value: &str) -> crate::Result<()> {
    if value.is_empty() || value == "." || value == ".." {
        return Err(DblError::InvalidInput {
            field,
            reason: "empty or relative path segment".to_owned(),
        });
    }
    let safe = value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if safe {
        Ok(())
    } else {
        Err(DblError::InvalidInput {
            field,
            reason: format!("unsafe path segment: {value}"),
        })
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn layout_matches_the_contract() {
        let paths = ToolchainPaths::with_root("/h");
        assert_eq!(paths.toolchains_dir(), Path::new("/h/toolchains"));
        assert_eq!(
            paths.install_dir(ToolId::Php, "8.4.15", Some("common")),
            Path::new("/h/toolchains/php/8.4.15-common")
        );
        assert_eq!(
            paths.install_dir(ToolId::Node, "24.11.0", None),
            Path::new("/h/toolchains/node/24.11.0")
        );
        assert_eq!(
            paths.active_registry(),
            Path::new("/h/toolchains/active.json")
        );
        assert_eq!(paths.shims_dir(), Path::new("/h/bin"));
        assert_eq!(
            paths.php_managed_ini(),
            Path::new("/h/toolchains/php/conf.d/99-dblocal.ini")
        );
    }

    #[test]
    fn shim_names_follow_the_os() {
        let paths = ToolchainPaths::with_root("/h");
        assert_eq!(paths.shim_path("php", Os::Linux), Path::new("/h/bin/php"));
        assert_eq!(
            paths.shim_path("php", Os::Windows),
            Path::new("/h/bin/php.cmd")
        );
    }

    #[test]
    fn rejects_unsafe_segments_and_escapes() {
        let paths = ToolchainPaths::with_root("/h");
        assert!(
            paths
                .checked_install_dir(ToolId::Php, "../evil", None)
                .is_err()
        );
        assert!(
            paths
                .checked_install_dir(ToolId::Php, "8.4", Some("a/b"))
                .is_err()
        );
        assert!(
            paths
                .ensure_within_toolchains(Path::new("/h/elsewhere"))
                .is_err()
        );
        assert!(
            paths
                .ensure_within_toolchains(Path::new("/h/toolchains/php/8.4"))
                .is_ok()
        );
    }
}
