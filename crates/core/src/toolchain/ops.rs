//! High-level toolchain operations, shared by the CLI and the GUI.
//!
//! [`ToolchainService`] ties the pieces together with the semantics the release
//! specifies:
//!
//! * installing does not activate a version unless nothing of that tool is active
//!   yet;
//! * `use` switches the active version of an installed tool and regenerates its
//!   shims immediately;
//! * removing the active version clears the registry entry and the shim, but if
//!   another version of the tool remains installed it does **not** auto-switch —
//!   the caller is told so it can explain that;
//! * the Composer shim refuses to exist without an active PHP.
//!
//! Every call reads the install scan (`registry::scan_installed`) as the truth for
//! "installed"; `active.json` only records the choice.

use std::path::{Path, PathBuf};

use crate::DblError;
use crate::platform::{Arch, Os};
use crate::toolchain::catalog::{
    CatalogState, CatalogStatus, Profile, ToolId, VersionSpec, resolve_version_for_platform,
};
use crate::toolchain::error::ToolchainError;
use crate::toolchain::fsops::ToolchainPaths;
use crate::toolchain::install::{self, InstallKind, InstallRequest, RemoveOptions};
use crate::toolchain::php_ini::{PhpSettings, SettingKey, SettingValue};
use crate::toolchain::registry::{
    ActiveVersion, InstalledVersion, Registry, read_registry, scan_installed, write_registry,
};
use crate::toolchain::shim;

/// One catalog version, as a listing shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableVersion {
    /// Tool.
    pub tool: ToolId,
    /// Full version tag.
    pub version: String,
    /// Series (`8.4`, `24`).
    pub series: String,
    /// Node LTS marker, when known.
    pub lts: Option<bool>,
    /// Whether this version is installed on the host.
    pub installed: bool,
    /// Whether this version is the active one.
    pub active: bool,
    /// Whether an artifact is installable on the running platform.
    pub available_here: bool,
}

/// One installed version, as a listing shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledListing {
    /// Tool.
    pub tool: ToolId,
    /// Full version tag.
    pub version: String,
    /// Profile, when the install has one.
    pub profile: Option<String>,
    /// Size on disk.
    pub size_bytes: u64,
    /// Whether this is the active version.
    pub active: bool,
}

/// Result of a `use` or an implicit activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activation {
    /// The version that became active.
    pub active: ActiveVersion,
    /// Whether shims were (re)generated.
    pub shims_refreshed: bool,
    /// Optional non-fatal warning (e.g. Composer activated without an active PHP).
    pub warning: Option<String>,
}

/// Result of an install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallReport {
    /// Tool.
    pub tool: ToolId,
    /// Version.
    pub version: String,
    /// Profile.
    pub profile: Option<String>,
    /// Whether it was already installed.
    pub already_installed: bool,
    /// Whether it was activated as part of the install.
    pub activated: bool,
    /// The install directory.
    pub dir: PathBuf,
}

/// Result of a remove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveReport {
    /// Tool.
    pub tool: ToolId,
    /// Version.
    pub version: String,
    /// The version that was cleared from the registry, if any.
    pub cleared: Option<ActiveVersion>,
    /// Whether other versions of the tool remain installed.
    pub others_remain: bool,
    /// Whether shims were regenerated (a cleared active entry regenerates them).
    pub shims_refreshed: bool,
}

/// The toolchain service over one DBLocal home.
pub struct ToolchainService {
    paths: ToolchainPaths,
    home: PathBuf,
    os: Os,
    arch: Arch,
    catalog: CatalogState,
    /// Test seam: URL policy for a loopback fixture server.
    policy: Option<crate::download::UrlPolicy>,
    /// Test/debug seam: permit loopback `http://` downloads.
    allow_loopback_http: bool,
    /// Test seam: free-space probe.
    free_space: Option<crate::download::FreeSpaceFn>,
    /// Optional download progress sink (used by the GUI).
    progress: Option<crate::download::ProgressSink>,
    /// Optional cooperative cancellation token (used by the GUI).
    cancel: Option<crate::download::CancellationToken>,
}

impl ToolchainService {
    /// Builds the service, loading the catalog from the cache or the bundle.
    pub fn new(paths: ToolchainPaths, home: impl Into<PathBuf>) -> Self {
        let catalog = crate::toolchain::catalog::load_or_bundled(&paths);
        Self {
            paths,
            home: home.into(),
            os: Os::current(),
            arch: Arch::current(),
            catalog,
            policy: None,
            allow_loopback_http: crate::toolchain::catalog::env_allows_loopback_http(),
            free_space: None,
            progress: None,
            cancel: None,
        }
    }

    /// Builds the service from the shared home resolver.
    ///
    /// User shell startup files live under `$HOME` (`%USERPROFILE%` on Windows),
    /// NOT inside `~/.dblocal` (`paths.root()`), so we resolve the user's home
    /// directory separately from the DBLocal data root.
    pub fn from_paths(paths: &crate::paths::DblPaths) -> crate::Result<Self> {
        let home = user_home_dir().unwrap_or_else(|| {
            paths
                .root()
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| paths.root().to_path_buf())
        });
        Ok(Self::new(ToolchainPaths::from_paths(paths), home))
    }

    /// The platform OS in use.
    pub const fn os(&self) -> Os {
        self.os
    }

    /// Overrides the platform (tests).
    pub fn with_platform(mut self, os: Os, arch: Arch) -> Self {
        self.os = os;
        self.arch = arch;
        self
    }

    /// Overrides the catalog (tests and a pre-seeded state).
    pub fn with_catalog(mut self, catalog: CatalogState) -> Self {
        self.catalog = catalog;
        self
    }

    /// Overrides the download URL policy (tests only).
    pub fn with_policy(mut self, policy: crate::download::UrlPolicy) -> Self {
        self.policy = Some(policy);
        self
    }

    /// Enables loopback `http://` downloads in debug/test builds.
    #[cfg(any(test, debug_assertions))]
    pub fn with_loopback_http(mut self) -> Self {
        self.allow_loopback_http = true;
        self
    }

    /// Overrides the free-space probe (tests).
    pub fn with_free_space(mut self, probe: crate::download::FreeSpaceFn) -> Self {
        self.free_space = Some(probe);
        self
    }

    /// Attaches a download progress callback (reuses [`crate::download::ProgressSink`]).
    pub fn with_progress(mut self, progress: crate::download::ProgressSink) -> Self {
        self.progress = Some(progress);
        self
    }

    /// Attaches a cooperative cancellation token (reuses [`crate::download::CancellationToken`]).
    pub fn with_cancel(mut self, cancel: crate::download::CancellationToken) -> Self {
        self.cancel = Some(cancel);
        self
    }

    /// Returns `true` if another process or task currently holds the install/remove
    /// lock for `(tool, version, profile)`. Cheap non-blocking probe for the GUI.
    pub fn is_busy(&self, tool: ToolId, version: &str, profile: Option<&str>) -> bool {
        let lock_path = self.paths.lock_file(tool, version, profile);
        if !lock_path.exists() {
            return false;
        }
        matches!(
            crate::toolchain::lock::ToolchainLock::try_exclusive(&lock_path),
            Ok(None)
        )
    }

    /// The paths in use.
    pub fn paths(&self) -> &ToolchainPaths {
        &self.paths
    }

    /// The catalog in use and its freshness.
    pub fn catalog(&self) -> &CatalogState {
        &self.catalog
    }

    /// The active registry.
    pub fn registry(&self) -> Registry {
        read_registry(&self.paths)
    }

    /// Every installed version.
    pub fn installed(&self) -> Vec<InstalledVersion> {
        scan_installed(&self.paths)
    }

    /// Catalog versions of `tool` (or all tools), annotated with installed/active
    /// state. When `installed_only` is set, versions that are not installed are
    /// dropped.
    pub fn list(&self, tool: Option<ToolId>, installed_only: bool) -> Vec<AvailableVersion> {
        let registry = self.registry();
        let installed = self.installed();
        let mut out = Vec::new();
        let tools: Vec<ToolId> = match tool {
            Some(id) => vec![id],
            None => ToolId::all().to_vec(),
        };
        for id in tools {
            let is_active = registry.get(id).cloned();
            for version in self.catalog.catalog().versions(id) {
                let is_installed = installed
                    .iter()
                    .any(|i| i.tool() == id && i.version() == version.version());
                if installed_only && !is_installed {
                    continue;
                }
                let available_here = self.is_available_here(id, version);
                if !available_here && !is_installed {
                    // Versions without an artifact for this platform are invisible.
                    continue;
                }
                let active = is_active
                    .as_ref()
                    .is_some_and(|a| a.version() == version.version());
                out.push(AvailableVersion {
                    tool: id,
                    version: version.version().to_owned(),
                    series: version.series().to_owned(),
                    lts: version.lts(),
                    installed: is_installed,
                    active,
                    available_here,
                });
            }
        }
        out
    }

    /// Every installed version, annotated with whether it is active.
    pub fn installed_listing(&self) -> Vec<InstalledListing> {
        let registry = self.registry();
        self.installed()
            .into_iter()
            .map(|installed| {
                let active = registry.get(installed.tool()).is_some_and(|a| {
                    a.version() == installed.version() && a.profile() == installed.profile()
                });
                InstalledListing {
                    tool: installed.tool(),
                    version: installed.version().to_owned(),
                    profile: installed.profile().map(str::to_owned),
                    size_bytes: installed.size_bytes(),
                    active,
                }
            })
            .collect()
    }

    /// Whether `version` has any installable artifact on the running platform.
    fn is_available_here(
        &self,
        tool: ToolId,
        version: &crate::toolchain::catalog::ToolVersion,
    ) -> bool {
        match (tool, self.os) {
            (ToolId::Php, Os::Linux) => {
                version
                    .select_artifact(self.os, self.arch, Some(Profile::Common))
                    .is_ok()
                    || version
                        .select_artifact(self.os, self.arch, Some(Profile::Bulk))
                        .is_ok()
            }
            _ => version.select_artifact(self.os, self.arch, None).is_ok(),
        }
    }

    /// Installs a catalog version by spec, activating it only when the tool has no
    /// active version yet.
    pub async fn install(
        &self,
        tool: ToolId,
        spec: &VersionSpec,
        profile: Option<Profile>,
        activate_unless_set: bool,
    ) -> crate::Result<InstallReport> {
        let entry = self.catalog.catalog().tool(tool).ok_or_else(|| {
            ToolchainError::UnavailableOnPlatform {
                tool: tool.as_str(),
                version: spec.as_str().to_owned(),
                reason: "the catalog lists no versions for this tool".to_owned(),
            }
            .into_dbl_error()
        })?;
        let effective_profile = match tool {
            ToolId::Php if self.os == Os::Linux => profile.or(Some(Profile::Common)),
            _ => None,
        };
        let version =
            resolve_version_for_platform(tool, entry, spec, self.os, self.arch, effective_profile)?;
        let artifact = version
            .select_artifact(self.os, self.arch, effective_profile)
            .map_err(|_| {
                ToolchainError::UnavailableOnPlatform {
                    tool: tool.as_str(),
                    version: version.version().to_owned(),
                    reason: format!(
                        "{}{}",
                        self.os,
                        effective_profile
                            .map(|p| format!(" with the {} profile", p.as_str()))
                            .unwrap_or_default()
                    ),
                }
                .into_dbl_error()
            })?;

        let mut request = InstallRequest::new(
            tool,
            artifact,
            version.version(),
            effective_profile.map(Profile::as_str),
        );
        request.policy = self.policy.clone();
        request.allow_loopback_http = self.allow_loopback_http;
        request.free_space = self.free_space.clone();
        request.progress = self.progress.clone();
        request.cancel = self.cancel.clone();

        let outcome = install::install(&self.paths, self.os, self.arch, request).await?;
        let already_installed = outcome.kind == InstallKind::AlreadyInstalled;

        // Activation: only when the tool has no active version yet.
        let registry = self.registry();
        let mut activated = false;
        if activate_unless_set && registry.get(tool).is_none() {
            let active = ActiveVersion::new(
                tool,
                version.version(),
                effective_profile.map(|p| p.as_str().to_owned()),
            );
            self.set_active(active)?;
            activated = true;
        }

        Ok(InstallReport {
            tool,
            version: version.version().to_owned(),
            profile: effective_profile.map(|p| p.as_str().to_owned()),
            already_installed,
            activated,
            dir: outcome.dir,
        })
    }

    /// Switches the active version of `tool` to the installed version `spec`
    /// resolves to.
    pub fn activate(&self, tool: ToolId, spec: &VersionSpec) -> crate::Result<Activation> {
        let installed: Vec<InstalledVersion> = self
            .installed()
            .into_iter()
            .filter(|v| v.tool() == tool)
            .collect();
        if installed.is_empty() {
            return Err(ToolchainError::NotInstalled {
                tool: tool.as_str(),
                version: spec.as_str().to_owned(),
            }
            .into_dbl_error());
        }
        let chosen = resolve_installed(tool, &installed, spec)?;
        let active = ActiveVersion::new(
            tool,
            chosen.version().to_owned(),
            chosen.profile().map(str::to_owned),
        );
        self.set_active(active.clone())?;
        let warning = if tool == ToolId::Composer && self.active(ToolId::Php).is_none() {
            Some(
                "composer needs an active PHP; run `dblocal toolchain use php <version>`"
                    .to_owned(),
            )
        } else {
            None
        };
        Ok(Activation {
            active,
            shims_refreshed: true,
            warning,
        })
    }

    /// Removes an installed version, applying the active-version rules.
    pub async fn remove(&self, tool: ToolId, spec: &VersionSpec) -> crate::Result<RemoveReport> {
        let installed: Vec<InstalledVersion> = self
            .installed()
            .into_iter()
            .filter(|v| v.tool() == tool)
            .collect();
        if installed.is_empty() {
            return Err(ToolchainError::NotInstalled {
                tool: tool.as_str(),
                version: spec.as_str().to_owned(),
            }
            .into_dbl_error());
        }
        let chosen = resolve_installed(tool, &installed, spec)?;
        let outcome = install::remove(
            &self.paths,
            tool,
            chosen.version(),
            chosen.profile(),
            RemoveOptions::default(),
        )
        .await?;

        let mut shims_refreshed = false;
        if outcome.cleared_active.is_some() {
            // The registry entry is gone; drop the shim(s) for this tool so a
            // stale launcher never points at a removed install.
            shim::remove_for(&self.paths, self.os, tool);
            shims_refreshed = true;
        }

        Ok(RemoveReport {
            tool,
            version: chosen.version().to_owned(),
            cleared: outcome.cleared_active,
            others_remain: outcome.others_remain,
            shims_refreshed,
        })
    }

    /// The active version of `tool`, if any.
    pub fn active(&self, tool: ToolId) -> Option<ActiveVersion> {
        self.registry().get(tool).cloned()
    }

    /// Regenerates every shim from the current registry.
    pub fn refresh_shims(&self) -> crate::Result<()> {
        shim::refresh(&self.paths, self.os, &self.registry())
    }

    /// Records `active` and regenerates the shims crash-safely.
    ///
    /// Shims are written first; if updating `active.json` then fails, the shims
    /// are restored to the previous registry so disk state never disagrees.
    fn set_active(&self, active: ActiveVersion) -> crate::Result<()> {
        let previous = self.registry();
        let next = previous.with_active(active);
        shim::refresh(&self.paths, self.os, &next)?;
        if let Err(err) = write_registry(&self.paths, &next) {
            let _ = shim::refresh(&self.paths, self.os, &previous);
            return Err(err);
        }
        Ok(())
    }

    /// Reads the managed PHP settings from disk (defaults when absent).
    pub fn php_settings(&self) -> PhpSettings {
        let path = self.paths.php_managed_ini();
        match std::fs::read_to_string(path) {
            Ok(text) => PhpSettings::parse_ini(&text),
            Err(_) => PhpSettings::default(),
        }
    }

    /// Sets PHP settings and writes the managed ini. Requires an active PHP.
    pub fn set_php_settings(
        &self,
        updates: &[(SettingKey, SettingValue)],
    ) -> crate::Result<PathBuf> {
        self.require_active_php()?;
        let mut settings = self.php_settings();
        for (key, value) in updates {
            settings = settings.with_value(*key, value.clone());
        }
        let path = crate::toolchain::php_ini::write_managed_ini(&self.paths, &settings)?;
        // Make sure the php shim exports the conf.d dir this file lives in.
        self.refresh_shims()?;
        Ok(path)
    }

    /// Resets PHP settings to defaults.
    pub fn reset_php_settings(&self) -> crate::Result<PathBuf> {
        self.require_active_php()?;
        crate::toolchain::php_ini::write_managed_ini(&self.paths, &PhpSettings::default())
    }

    /// The directory holding the managed PHP ini, for "open location".
    pub fn php_settings_dir(&self) -> PathBuf {
        self.paths.php_conf_dir(ToolId::Php)
    }

    fn require_active_php(&self) -> crate::Result<()> {
        if self.active(ToolId::Php).is_none() {
            return Err(ToolchainError::NoActiveVersion {
                tool: ToolId::Php.as_str(),
            }
            .into_dbl_error());
        }
        Ok(())
    }

    /// The doctor findings for the current home and PATH.
    pub fn doctor(&self, path_var: &str) -> Vec<crate::toolchain::path::Finding> {
        crate::toolchain::path::doctor(&self.paths, &self.home, path_var, self.os)
    }

    /// Applies the PATH block (Linux/macOS).
    pub fn apply_path(&self) -> crate::Result<crate::toolchain::path::PathEditReport> {
        crate::toolchain::path::apply(&self.home, &self.paths.shims_dir(), self.os)
    }

    /// Removes the PATH block.
    pub fn remove_path(&self) -> crate::Result<crate::toolchain::path::PathEditReport> {
        crate::toolchain::path::remove(&self.home, &self.paths.shims_dir(), self.os)
    }

    /// The exact lines the PATH block would add, for the consent prompt.
    pub fn path_block_preview(&self) -> String {
        crate::toolchain::path::render_block(&self.paths.shims_dir())
    }

    /// Whether the PATH block is currently applied in any rc file.
    pub fn path_applied(&self) -> bool {
        crate::toolchain::path::block_shims_dir(&self.home).is_some()
    }

    /// The catalog freshness.
    pub fn catalog_status(&self) -> CatalogStatus {
        self.catalog.status()
    }

    /// The home root (for the GUI).
    pub fn home(&self) -> &Path {
        &self.home
    }
}

/// Resolves the user's home directory (`$HOME` on Unix, `%USERPROFILE%` on Windows).
fn user_home_dir() -> Option<PathBuf> {
    if let Some(raw) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(raw));
    }
    #[cfg(windows)]
    if let Some(raw) = std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(raw));
    }
    None
}

/// Resolves a spec against installed versions of `tool`.
///
/// Same preference order as the catalog resolver (exact, dotted prefix, series),
/// but over what is actually on disk. `latest` means the newest installed.
fn resolve_installed<'a>(
    tool: ToolId,
    installed: &'a [InstalledVersion],
    spec: &VersionSpec,
) -> crate::Result<&'a InstalledVersion> {
    let newest = || {
        installed
            .iter()
            .max_by(|a, b| crate::toolchain::catalog::compare_versions(a.version(), b.version()))
            .unwrap_or(&installed[0])
    };
    if matches!(spec, VersionSpec::Latest) {
        return Ok(newest());
    }
    let tag = spec.as_str();
    if let Some(exact) = installed.iter().find(|v| v.version() == tag) {
        return Ok(exact);
    }
    if let Some(prefixed) = installed
        .iter()
        .filter(|v| {
            v.version()
                .strip_prefix(tag)
                .is_some_and(|r| r.starts_with('.'))
        })
        .max_by(|a, b| crate::toolchain::catalog::compare_versions(a.version(), b.version()))
    {
        return Ok(prefixed);
    }
    Err(ToolchainError::NotInstalled {
        tool: tool.as_str(),
        version: tag.to_owned(),
    }
    .into_dbl_error())
}

/// Whether an error is the "no such installed version" case (used by the CLI to
/// choose exit code 3 rather than 2).
pub fn is_not_found(err: &DblError) -> bool {
    matches!(err, DblError::NotFound { .. })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::toolchain::catalog::{CatalogState, CatalogStatus, parse_catalog};

    #[test]
    fn list_hides_versions_unavailable_on_this_platform() {
        let doc = r#"{
          "schema": 1,
          "generated_at": "2026-11-27T00:00:00Z",
          "tools": [{
            "id": "php",
            "versions": [
              {
                "version": "8.5.13", "series": "8.5",
                "artifacts": [{
                  "os": "windows", "arch": "x86_64", "profile": null, "format": "zip",
                  "url": "https://example.test/php-8.5.13.zip",
                  "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                  "size_bytes": 100, "source": "windows.php.net", "hash_origin": "upstream"
                }]
              },
              {
                "version": "8.5.8", "series": "8.5",
                "artifacts": [{
                  "os": "linux", "arch": "x86_64", "profile": "common", "format": "tar.gz",
                  "url": "https://example.test/php-8.5.8.tar.gz",
                  "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                  "size_bytes": 100, "source": "static-php-cli", "hash_origin": "pinned"
                }]
              },
              {
                "version": "7.4.33", "series": "7.4",
                "artifacts": [{
                  "os": "windows", "arch": "x86_64", "profile": null, "format": "zip",
                  "url": "https://example.test/php-7.4.33.zip",
                  "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                  "size_bytes": 100, "source": "windows.php.net", "hash_origin": "upstream"
                }]
              }
            ]
          }]
        }"#;
        let (catalog, skips) = parse_catalog(doc).expect("parse");
        let state = CatalogState::new(
            catalog,
            doc,
            CatalogStatus::Fresh,
            None,
            std::time::SystemTime::now(),
            skips,
        );
        let dir = tempfile::tempdir().expect("tmp");
        let paths = ToolchainPaths::with_root(dir.path());

        let linux_svc = ToolchainService::new(paths.clone(), dir.path())
            .with_platform(Os::Linux, Arch::X86_64)
            .with_catalog(state.clone());
        let linux_versions: Vec<String> = linux_svc
            .list(Some(ToolId::Php), false)
            .into_iter()
            .map(|v| v.version)
            .collect();
        assert_eq!(linux_versions, vec!["8.5.8"]);

        let win_svc = ToolchainService::new(paths, dir.path())
            .with_platform(Os::Windows, Arch::X86_64)
            .with_catalog(state);
        let win_versions: Vec<String> = win_svc
            .list(Some(ToolId::Php), false)
            .into_iter()
            .map(|v| v.version)
            .collect();
        assert_eq!(win_versions, vec!["8.5.13", "7.4.33"]);
    }

    #[test]
    fn composer_without_php_succeeds_and_never_blocks_other_tools() {
        let dir = tempfile::tempdir().expect("tmp");
        let paths = ToolchainPaths::with_root(dir.path());
        // Pre-seed installed composer and node directories with valid manifests.
        let comp_dir = paths.install_dir(ToolId::Composer, "2.11.3", None);
        std::fs::create_dir_all(&comp_dir).expect("mkdir");
        std::fs::write(comp_dir.join("composer.phar"), b"phar").expect("phar");
        crate::toolchain::registry::write_manifest(
            &comp_dir,
            ToolId::Composer,
            "2.11.3",
            None,
            "https://example.test/composer.phar",
            &"a".repeat(64),
        )
        .expect("manifest");

        let node_dir = paths.install_dir(ToolId::Node, "24.11.0", None);
        std::fs::create_dir_all(node_dir.join("bin")).expect("mkdir");
        std::fs::write(node_dir.join("bin/node"), b"node").expect("node");
        crate::toolchain::registry::write_manifest(
            &node_dir,
            ToolId::Node,
            "24.11.0",
            None,
            "https://example.test/node.tar.gz",
            &"b".repeat(64),
        )
        .expect("manifest");

        let svc =
            ToolchainService::new(paths.clone(), dir.path()).with_platform(Os::Linux, Arch::X86_64);

        // Activating Composer first (with no PHP active) must succeed and write its shim.
        let act = svc
            .activate(ToolId::Composer, &VersionSpec::Latest)
            .expect("composer activate without php");
        assert!(paths.shim_path("composer", Os::Linux).is_file());
        assert!(
            act.warning.is_some(),
            "expected warning when php is inactive"
        );

        // Activating Node afterwards must also succeed.
        let node_act = svc
            .activate(ToolId::Node, &VersionSpec::Latest)
            .expect("node activate after composer");
        assert!(paths.shim_path("node", Os::Linux).is_file());
        assert!(node_act.warning.is_none());

        // Refreshing when PHP was removed while Composer stays active must succeed.
        let reg = svc.registry().without(ToolId::Php);
        crate::toolchain::shim::refresh(&paths, Os::Linux, &reg)
            .expect("refresh with composer active and php absent");
        assert!(paths.shim_path("composer", Os::Linux).is_file());
    }

    #[tokio::test]
    async fn service_wires_progress_cancel_and_is_busy() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let phar = b"<?php // fake phar for progress/cancel test\n".to_vec();
        let sha: String = {
            use sha2::Digest as _;
            sha2::Sha256::digest(&phar)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let body = phar.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let b = body.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = socket.read(&mut buf).await;
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        b.len()
                    );
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(&b).await;
                    let _ = socket.flush().await;
                });
            }
        });

        let doc = format!(
            r#"{{"schema":1,"generated_at":"2026-11-28T00:00:00Z","tools":[{{"id":"composer","versions":[{{"version":"2.11.3","series":"2","artifacts":[{{"os":"any","arch":"any","profile":null,"format":"phar","url":"http://{addr}/composer.phar","sha256":"{sha}","size_bytes":{},"source":"test","hash_origin":"upstream"}}]}}]}}]}}"#,
            phar.len()
        );
        let (catalog, skips) =
            crate::toolchain::catalog::parse_catalog_allow_loopback(&doc).expect("parse");
        let state = CatalogState::new(
            catalog,
            doc,
            CatalogStatus::Fresh,
            None,
            std::time::SystemTime::now(),
            skips,
        );

        let dir = tempfile::tempdir().expect("tmp");
        let paths = ToolchainPaths::with_root(dir.path());

        // 1. Pre-cancelled token aborts cleanly with DblError::Cancelled.
        let cancel = crate::download::CancellationToken::new();
        cancel.cancel();
        let cancelled_svc = ToolchainService::new(paths.clone(), dir.path())
            .with_platform(Os::Linux, Arch::X86_64)
            .with_catalog(state.clone())
            .with_loopback_http()
            .with_cancel(cancel);
        let err = cancelled_svc
            .install(ToolId::Composer, &VersionSpec::Latest, None, true)
            .await
            .expect_err("cancelled install must fail");
        assert!(matches!(err, DblError::Cancelled { .. }), "{err:?}");
        assert!(!paths.install_dir(ToolId::Composer, "2.11.3", None).exists());

        // 2. Progress sink receives events during a normal install.
        let events = Arc::new(AtomicUsize::new(0));
        let counter = events.clone();
        let sink: crate::download::ProgressSink = Arc::new(move |_ev| {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        let svc = ToolchainService::new(paths.clone(), dir.path())
            .with_platform(Os::Linux, Arch::X86_64)
            .with_catalog(state)
            .with_loopback_http()
            .with_progress(sink);
        svc.install(ToolId::Composer, &VersionSpec::Latest, None, true)
            .await
            .expect("install with progress");
        assert!(
            events.load(Ordering::SeqCst) >= 1,
            "progress sink must fire"
        );

        // 3. is_busy reflects lock state.
        assert!(!svc.is_busy(ToolId::Composer, "2.11.3", None));
        let held = crate::toolchain::lock::ToolchainLock::acquire_default(&paths.lock_file(
            ToolId::Composer,
            "2.11.3",
            None,
        ))
        .expect("lock");
        assert!(svc.is_busy(ToolId::Composer, "2.11.3", None));
        drop(held);
        assert!(!svc.is_busy(ToolId::Composer, "2.11.3", None));
    }
}
