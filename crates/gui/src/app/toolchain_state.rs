//! Pure view-model and state reduction for the Toolchains GUI (v0.3.0).
//!
//! Keeps all version-row derivation, update detection, button enablement,
//! PATH status/findings classification, and PHP settings validation out of
//! GPUI render code so every rule is unit-tested without a window.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use dblocal_core::platform::{Arch, Os};
use dblocal_core::toolchain::catalog::{
    CatalogStatus, Profile, ToolId, ToolchainCatalog, compare_versions,
};
use dblocal_core::toolchain::ops::{InstalledListing, ToolchainService};
use dblocal_core::toolchain::path::{existing_rc_files, fish_config_dir, fish_fragment};
use dblocal_core::toolchain::php_ini::PhpSettings;
use dblocal_core::toolchain::registry::ActiveVersion;

/// Human-readable display name for a toolchain.
pub const fn tool_display_name(tool: ToolId) -> &'static str {
    match tool {
        ToolId::Php => "PHP",
        ToolId::Node => "Node.js",
        ToolId::Composer => "Composer",
    }
}

/// Short subtitle for a toolchain page header.
pub const fn tool_subtitle(tool: ToolId) -> &'static str {
    match tool {
        ToolId::Php => "Per-user PHP CLI binaries exposed via DBLocal shims",
        ToolId::Node => "Official Node.js runtimes with bundled npm and npx shims",
        ToolId::Composer => "PHP dependency manager running on your active PHP version",
    }
}

/// Short description of a Linux PHP extension profile.
pub const fn profile_description(profile: Profile) -> &'static str {
    match profile {
        Profile::Common => {
            "Default for Composer and Laravel: pdo_mysql, pdo_pgsql, pdo_sqlite, mbstring, openssl, curl."
        }
        Profile::Bulk => "Adds intl, sodium, opcache and mysqli; omits pdo_pgsql and pdo_sqlite.",
    }
}

/// Summary of one tool for the sidebar TOOLCHAINS section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarToolSummary {
    /// Tool identifier.
    pub tool: ToolId,
    /// Display name (`PHP`, `Node.js`, `Composer`).
    pub label: &'static str,
    /// Compact state hint shown on the right of the sidebar row.
    pub hint: String,
    /// Whether an active version is currently set.
    pub has_active: bool,
    /// Whether any version of this tool is installed.
    pub any_installed: bool,
}

/// Lifecycle state of one version row on a toolchain page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionRowStatus {
    /// Installed and currently active.
    Active,
    /// Installed, not active.
    Installed,
    /// Not yet installed, available in the catalog.
    Available,
}

/// In-flight download/install progress for one `(tool, version, profile)` key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallProgress {
    /// Bytes downloaded so far.
    pub bytes_done: u64,
    /// Total expected bytes (0 if unknown).
    pub bytes_total: u64,
    /// Whether cancellation has been requested.
    pub cancelling: bool,
}

impl InstallProgress {
    /// Percentage in `0..=100`, or `None` when total is unknown.
    pub fn percent(&self) -> Option<u8> {
        if self.bytes_total == 0 {
            return None;
        }
        let pct = (self.bytes_done.saturating_mul(100) / self.bytes_total).min(100);
        Some(pct as u8)
    }

    /// Compact label for the progress bar.
    pub fn label(&self) -> String {
        if self.cancelling {
            return "Cancelling…".to_owned();
        }
        match self.percent() {
            Some(pct) => format!(
                "{pct}% ({} / {})",
                crate::format::format_bytes(self.bytes_done),
                crate::format::format_bytes(self.bytes_total),
            ),
            None if self.bytes_done > 0 => {
                format!(
                    "Downloading {}…",
                    crate::format::format_bytes(self.bytes_done)
                )
            }
            None => "Preparing…".to_owned(),
        }
    }
}

/// Key uniquely identifying an installable or installed version row.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VersionKey {
    pub tool: ToolId,
    pub version: String,
    pub profile: Option<Profile>,
}

impl VersionKey {
    pub fn new(tool: ToolId, version: impl Into<String>, profile: Option<Profile>) -> Self {
        Self {
            tool,
            version: version.into(),
            profile,
        }
    }
}

/// One row on a toolchain version page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainVersionRow {
    /// Unique row key.
    pub key: VersionKey,
    /// Release series (`8.4`, `24`, `2`).
    pub series: String,
    /// Node.js LTS marker.
    pub lts: bool,
    /// Current status.
    pub status: VersionRowStatus,
    /// Newer patch version in the same series available in the catalog, if any.
    pub update_available: Option<String>,
    /// Size on disk (if installed) or download size (if available).
    pub size_bytes: u64,
    /// Sorted PHP extensions from the catalog artifact (empty for non-PHP).
    pub extensions: Vec<String>,
    /// True for Linux PHP Common builds whose catalog metadata omits `pdo_pgsql`
    /// (e.g. PHP 8.0 common).
    pub missing_pdo_pgsql_note: bool,
    /// True if an install/remove lock or in-flight task is active for this row.
    pub busy: bool,
    /// Live download progress if this row is currently installing.
    pub progress: Option<InstallProgress>,
    /// Last inline error for this row, if any.
    pub error: Option<String>,
    /// Whether the extension list is expanded in the UI.
    pub extensions_expanded: bool,
}

impl ToolchainVersionRow {
    /// Whether the Install button should be shown and enabled.
    pub fn can_install(&self) -> bool {
        self.status == VersionRowStatus::Available && !self.busy
    }

    /// Whether the Make Active button should be shown and enabled.
    pub fn can_activate(&self) -> bool {
        self.status == VersionRowStatus::Installed && !self.busy
    }

    /// Whether the Remove button should be shown and enabled.
    pub fn can_remove(&self) -> bool {
        matches!(
            self.status,
            VersionRowStatus::Installed | VersionRowStatus::Active
        ) && !self.busy
    }

    /// Whether the Update button should be shown and enabled.
    pub fn can_update(&self) -> bool {
        self.update_available.is_some() && !self.busy
    }

    /// Whether the Cancel button should be shown for an active download.
    pub fn can_cancel(&self) -> bool {
        self.progress.as_ref().is_some_and(|p| !p.cancelling)
    }
}

/// High-level PATH health state for the shared PATH panel.
pub use super::toolchain_panels_state::*;

#[derive(Debug, Clone)]
pub struct ToolchainState {
    pub os: Os,
    pub arch: Arch,
    pub catalog: ToolchainCatalog,
    pub catalog_status: CatalogStatus,
    pub installed: Vec<InstalledListing>,
    pub active: HashMap<ToolId, ActiveVersion>,
    /// Selected PHP extension profile on Linux (`Common` by default).
    pub php_profile: Profile,
    /// In-flight installs keyed by `VersionKey`.
    pub installs: HashMap<VersionKey, InstallProgress>,
    /// Versions currently locked by an external process (`service.is_busy`).
    pub externally_busy: HashSet<VersionKey>,
    /// Per-version inline errors (cleared on retry or success).
    pub row_errors: HashMap<VersionKey, String>,
    /// Rows whose PHP extension list is expanded.
    pub expanded_extensions: HashSet<VersionKey>,
    /// Shared PATH panel state.
    pub path: PathPanelState,
    /// PHP settings form state.
    pub php_settings: PhpSettingsForm,
    /// Whether the Add-to-PATH consent dialog is open.
    pub path_consent_open: bool,
    /// Whether the Remove-version confirmation dialog is open.
    pub remove_dialog: Option<ToolchainRemoveDialog>,
    /// Currently selected toolchain page in the centre pane, if any.
    pub selected_tool: Option<ToolId>,
}

impl Default for ToolchainState {
    fn default() -> Self {
        Self {
            os: Os::current(),
            arch: Arch::current(),
            catalog: ToolchainCatalog::empty(),
            catalog_status: CatalogStatus::Offline,
            installed: Vec::new(),
            active: HashMap::new(),
            php_profile: Profile::Common,
            installs: HashMap::new(),
            externally_busy: HashSet::new(),
            row_errors: HashMap::new(),
            expanded_extensions: HashSet::new(),
            path: PathPanelState::default(),
            php_settings: PhpSettingsForm::default(),
            path_consent_open: false,
            remove_dialog: None,
            selected_tool: None,
        }
    }
}

/// Read-only snapshot produced off the GPUI thread by reading `ToolchainService`.
#[derive(Debug, Clone)]
pub struct ToolchainSnapshot {
    pub os: Os,
    pub arch: Arch,
    pub catalog: ToolchainCatalog,
    pub catalog_status: CatalogStatus,
    pub installed: Vec<InstalledListing>,
    pub active: HashMap<ToolId, ActiveVersion>,
    pub externally_busy: HashSet<VersionKey>,
    pub path: PathPanelState,
    pub php_settings: PhpSettings,
    pub php_settings_dir: PathBuf,
}

impl ToolchainSnapshot {
    /// Captures a full snapshot from `service` and the given `PATH` string.
    pub fn capture(service: &ToolchainService, arch: Arch, path_var: &str) -> Self {
        let os = service.os();
        let catalog = service.catalog().catalog().clone();
        let catalog_status = service.catalog_status();
        let installed = service.installed_listing();
        let registry = service.registry();
        let mut active = HashMap::new();
        for tool in ToolId::all() {
            if let Some(a) = registry.get(tool).cloned() {
                active.insert(tool, a);
            }
        }

        let mut externally_busy = HashSet::new();
        for tool in ToolId::all() {
            for v in catalog.versions(tool) {
                let profiles: &[Option<Profile>] = match (tool, os) {
                    (ToolId::Php, Os::Linux) => &[Some(Profile::Common), Some(Profile::Bulk)],
                    _ => &[None],
                };
                for &prof in profiles {
                    if service.is_busy(tool, v.version(), prof.map(Profile::as_str)) {
                        externally_busy.insert(VersionKey::new(tool, v.version(), prof));
                    }
                }
            }
        }

        let findings = service.doctor(path_var);
        let target_files = path_target_files(service.home(), os);
        let path = PathPanelState::from_doctor(
            service.paths().shims_dir(),
            service.path_applied(),
            &findings,
            service.path_block_preview(),
            target_files,
        );

        Self {
            os,
            arch,
            catalog,
            catalog_status,
            installed,
            active,
            externally_busy,
            path,
            php_settings: service.php_settings(),
            php_settings_dir: service.php_settings_dir(),
        }
    }
}

/// Lists the exact files (or registry entry on Windows) that `apply_path` will edit.
pub fn path_target_files(home: &Path, os: Os) -> Vec<String> {
    if os == Os::Windows {
        return vec![r"HKCU\Environment#Path".to_owned()];
    }
    let mut files: Vec<String> = existing_rc_files(home)
        .into_iter()
        .map(|rc| rc.path.display().to_string())
        .collect();
    if fish_config_dir(home).is_dir() {
        files.push(fish_fragment(home).display().to_string());
    }
    files
}

impl ToolchainState {
    /// Applies a fresh background snapshot, preserving in-flight UI state (active
    /// downloads, inline errors, expanded rows, dirty PHP settings edits).
    pub fn apply_snapshot(&mut self, snap: ToolchainSnapshot) {
        self.os = snap.os;
        self.arch = snap.arch;
        self.catalog = snap.catalog;
        self.catalog_status = snap.catalog_status;
        self.installed = snap.installed;
        self.active = snap.active;
        self.externally_busy = snap.externally_busy;

        // Preserve last_report / last_error / busy on the PATH panel across refreshes.
        let prev_report = self.path.last_report.clone();
        let prev_error = self.path.last_error.clone();
        let prev_busy = self.path.busy;
        self.path = snap.path;
        self.path.last_report = prev_report;
        self.path.last_error = prev_error;
        self.path.busy = prev_busy;

        // Only overwrite the PHP settings draft if the user has no unsaved edits.
        if !self.php_settings.is_dirty() {
            let prev_notice = self.php_settings.notice.clone();
            let prev_error = self.php_settings.error.clone();
            let prev_busy = self.php_settings.busy;
            self.php_settings =
                PhpSettingsForm::from_settings(snap.php_settings, snap.php_settings_dir);
            self.php_settings.notice = prev_notice;
            self.php_settings.error = prev_error;
            self.php_settings.busy = prev_busy;
        } else {
            self.php_settings.saved = snap.php_settings;
            self.php_settings.config_dir = snap.php_settings_dir;
        }
    }

    /// Sidebar summaries for PHP, Node.js, Composer in catalog order.
    pub fn sidebar_summaries(&self) -> Vec<SidebarToolSummary> {
        ToolId::all()
            .into_iter()
            .map(|tool| {
                let active = self.active.get(&tool);
                let any_installed = self.installed.iter().any(|i| i.tool == tool);
                let hint = match active {
                    Some(a) => match a.profile() {
                        Some(p) if tool == ToolId::Php && p != Profile::Common.as_str() => {
                            format!("{} · {p}", a.version())
                        }
                        _ => a.version().to_owned(),
                    },
                    None if any_installed => "Inactive".to_owned(),
                    None => "Not installed".to_owned(),
                };
                SidebarToolSummary {
                    tool,
                    label: tool_display_name(tool),
                    hint,
                    has_active: active.is_some(),
                    any_installed,
                }
            })
            .collect()
    }

    /// Whether the catalog has no usable entries for `tool` on this platform.
    pub fn catalog_unavailable(&self, tool: ToolId) -> bool {
        let profile = self.effective_profile(tool);
        let has_catalog_entry = self
            .catalog
            .versions(tool)
            .iter()
            .any(|v| v.select_artifact(self.os, self.arch, profile).is_ok());
        !has_catalog_entry
    }

    /// Effective profile for `tool` on the current OS.
    pub const fn effective_profile(&self, tool: ToolId) -> Option<Profile> {
        match (tool, self.os) {
            (ToolId::Php, Os::Linux) => Some(self.php_profile),
            _ => None,
        }
    }

    /// Whether PHP profile selection applies (Linux PHP only).
    pub const fn show_php_profile_selector(&self, tool: ToolId) -> bool {
        matches!((tool, self.os), (ToolId::Php, Os::Linux))
    }

    /// Whether the Composer page should warn that no PHP is active.
    pub fn composer_needs_php(&self) -> bool {
        !self.active.contains_key(&ToolId::Php)
    }

    /// Whether PHP settings are enabled (an active PHP version exists).
    pub fn php_settings_enabled(&self) -> bool {
        self.active.contains_key(&ToolId::Php)
    }

    /// Active version of `tool`, if any.
    pub fn active_version(&self, tool: ToolId) -> Option<&ActiveVersion> {
        self.active.get(&tool)
    }

    /// Recommended first-install version for the empty state (newest catalog
    /// version for this OS/profile, or `None` when catalog is empty).
    pub fn recommended_first_install(&self, tool: ToolId) -> Option<ToolchainVersionRow> {
        if self.installed.iter().any(|i| i.tool == tool) {
            return None;
        }
        self.version_rows(tool).into_iter().next()
    }

    /// Derived version rows for `tool`, sorted newest first.
    ///
    /// Includes all catalog versions available for `(self.os, self.arch, profile)`
    /// plus any installed version of `tool` (even if its profile differs or it is
    /// no longer in the catalog). Computes `update_available` when a newer patch
    /// in the same series exists in the catalog.
    pub fn version_rows(&self, tool: ToolId) -> Vec<ToolchainVersionRow> {
        let profile = self.effective_profile(tool);
        let active = self.active.get(&tool);

        // Map series -> newest catalog version available for this platform/profile.
        let mut newest_by_series: HashMap<String, String> = HashMap::new();
        for v in self.catalog.versions(tool) {
            if v.select_artifact(self.os, self.arch, profile).is_err() {
                continue;
            }
            newest_by_series
                .entry(v.series().to_owned())
                .and_modify(|cur| {
                    if compare_versions(v.version(), cur).is_gt() {
                        *cur = v.version().to_owned();
                    }
                })
                .or_insert_with(|| v.version().to_owned());
        }

        let mut rows: Vec<ToolchainVersionRow> = Vec::new();
        let mut seen: HashSet<(String, Option<Profile>)> = HashSet::new();

        // 1. Catalog versions matching this OS/arch/profile.
        for v in self.catalog.versions(tool) {
            let Ok(artifact) = v.select_artifact(self.os, self.arch, profile) else {
                continue;
            };
            let inst = self.find_installed(tool, v.version(), profile);
            let is_active = active.is_some_and(|a| {
                a.version() == v.version() && a.profile() == profile.map(Profile::as_str)
            });
            let status = if is_active {
                VersionRowStatus::Active
            } else if inst.is_some() {
                VersionRowStatus::Installed
            } else {
                VersionRowStatus::Available
            };
            let update_available = if inst.is_some() {
                newest_by_series
                    .get(v.series())
                    .filter(|latest| compare_versions(latest, v.version()).is_gt())
                    .cloned()
            } else {
                None
            };
            let key = VersionKey::new(tool, v.version(), profile);
            let extensions = artifact.extensions().to_vec();
            let missing_pdo_pgsql_note = tool == ToolId::Php
                && self.os == Os::Linux
                && profile == Some(Profile::Common)
                && !extensions.is_empty()
                && !extensions.iter().any(|e| e == "pdo_pgsql");
            let progress = self.installs.get(&key).cloned();
            let busy = progress.is_some() || self.externally_busy.contains(&key);
            let error = self.row_errors.get(&key).cloned();
            let extensions_expanded = self.expanded_extensions.contains(&key);
            let size_bytes = inst.map_or(artifact.size_bytes(), |i| i.size_bytes);

            seen.insert((v.version().to_owned(), profile));
            rows.push(ToolchainVersionRow {
                key,
                series: v.series().to_owned(),
                lts: v.lts().unwrap_or(false),
                status,
                update_available,
                size_bytes,
                extensions,
                missing_pdo_pgsql_note,
                busy,
                progress,
                error,
                extensions_expanded,
            });
        }

        // 2. Installed versions not already represented (e.g. older patch or other profile).
        for inst in self.installed.iter().filter(|i| i.tool == tool) {
            let inst_profile = inst
                .profile
                .as_deref()
                .and_then(|p| p.parse::<Profile>().ok());
            if seen.contains(&(inst.version.clone(), inst_profile)) {
                continue;
            }
            // For Linux PHP, show only installed versions matching the selected profile
            // OR the currently active version so the user never loses sight of what's active.
            let is_active = active.is_some_and(|a| {
                a.version() == inst.version && a.profile() == inst.profile.as_deref()
            });
            if tool == ToolId::Php && self.os == Os::Linux && inst_profile != profile && !is_active
            {
                continue;
            }
            let series = derive_series(tool, &inst.version);
            let update_available = newest_by_series
                .get(&series)
                .filter(|latest| compare_versions(latest, &inst.version).is_gt())
                .cloned();
            let key = VersionKey::new(tool, inst.version.clone(), inst_profile);
            let progress = self.installs.get(&key).cloned();
            let busy = progress.is_some() || self.externally_busy.contains(&key);
            let error = self.row_errors.get(&key).cloned();
            let extensions_expanded = self.expanded_extensions.contains(&key);

            rows.push(ToolchainVersionRow {
                key,
                series,
                lts: false,
                status: if is_active {
                    VersionRowStatus::Active
                } else {
                    VersionRowStatus::Installed
                },
                update_available,
                size_bytes: inst.size_bytes,
                extensions: Vec::new(),
                missing_pdo_pgsql_note: false,
                busy,
                progress,
                error,
                extensions_expanded,
            });
        }

        // Sort newest version first; tie-break Active before Installed before Available.
        rows.sort_by(|a, b| {
            compare_versions(&b.key.version, &a.key.version)
                .then_with(|| status_rank(a.status).cmp(&status_rank(b.status)))
        });
        rows
    }

    /// Toggles the expanded extension list for one PHP version row.
    pub fn toggle_extensions(&mut self, key: &VersionKey) {
        if !self.expanded_extensions.remove(key) {
            self.expanded_extensions.insert(key.clone());
        }
    }

    /// Builds a remove-confirmation dialog payload for an installed version.
    pub fn build_remove_dialog(
        &self,
        tool: ToolId,
        version: &str,
        profile: Option<Profile>,
    ) -> Option<ToolchainRemoveDialog> {
        let inst = self.find_installed(tool, version, profile)?;
        let is_active = self
            .active
            .get(&tool)
            .is_some_and(|a| a.version() == inst.version && a.profile() == inst.profile.as_deref());
        let others_remain = self
            .installed
            .iter()
            .filter(|i| i.tool == tool)
            .any(|i| !(i.version == inst.version && i.profile == inst.profile));
        Some(ToolchainRemoveDialog {
            tool,
            version: inst.version.clone(),
            profile: inst
                .profile
                .as_deref()
                .and_then(|p| p.parse::<Profile>().ok()),
            size_bytes: inst.size_bytes,
            is_active,
            others_remain,
        })
    }

    fn find_installed(
        &self,
        tool: ToolId,
        version: &str,
        profile: Option<Profile>,
    ) -> Option<&InstalledListing> {
        let prof_str = profile.map(Profile::as_str);
        self.installed
            .iter()
            .find(|i| i.tool == tool && i.version == version && i.profile.as_deref() == prof_str)
    }
}

fn status_rank(status: VersionRowStatus) -> u8 {
    match status {
        VersionRowStatus::Active => 0,
        VersionRowStatus::Installed => 1,
        VersionRowStatus::Available => 2,
    }
}

/// Derives a series string (`8.4`, `24`, `2`) from a version string when an
/// installed version is not in the current catalog.
fn derive_series(tool: ToolId, version: &str) -> String {
    let mut parts = version.split('.');
    match tool {
        ToolId::Php => match (parts.next(), parts.next()) {
            (Some(maj), Some(min)) => format!("{maj}.{min}"),
            _ => version.to_owned(),
        },
        ToolId::Node | ToolId::Composer => parts.next().unwrap_or(version).to_owned(),
    }
}
