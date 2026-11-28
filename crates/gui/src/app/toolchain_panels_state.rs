//! Pure view-model state for the Toolchain PATH panel, PHP settings card, and
//! remove-version confirmation dialog (v0.3.0).

use std::collections::BTreeMap;
use std::path::PathBuf;

use dblocal_core::toolchain::catalog::{Profile, ToolId};
use dblocal_core::toolchain::path::{Finding, PathEditReport};
use dblocal_core::toolchain::php_ini::{PhpSettings, SettingKey, SettingValue};

use super::toolchain_state::tool_display_name;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathHealth {
    /// Shims directory is on PATH, comes first, and no shadowing/dangling issues exist.
    AppliedAndFirst,
    /// Shims directory is not on PATH yet.
    NotApplied,
    /// Shims directory is on PATH or applied, but shadowed, not first, stale, or has dangling shims.
    NeedsAttention,
}

/// One plain-English finding for the PATH panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathFindingView {
    /// Machine code from `Finding::code()`.
    pub code: &'static str,
    /// Plain-English summary.
    pub message: String,
    /// True for shadowing / dangling shims; false for informational/missing.
    pub severe: bool,
}

/// Snapshot of the PATH panel state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathPanelState {
    /// Overall health classification.
    pub health: PathHealth,
    /// Shims directory path (`~/.dblocal/bin`).
    pub shims_dir: PathBuf,
    /// Whether the managed block is currently applied in any rc file (or Windows registry).
    pub applied: bool,
    /// Doctor findings translated into plain English.
    pub findings: Vec<PathFindingView>,
    /// Exact shell block preview (for the consent dialog).
    pub block_preview: String,
    /// Exact files (or registry key on Windows) that "Add to PATH" will modify.
    pub target_files: Vec<String>,
    /// Last apply/remove report, so per-file successes and failures stay visible.
    pub last_report: Option<PathEditReport>,
    /// Last top-level error (e.g. Windows registry failure).
    pub last_error: Option<String>,
    /// Whether a PATH apply/remove operation is currently running.
    pub busy: bool,
}

impl Default for PathPanelState {
    fn default() -> Self {
        Self {
            health: PathHealth::NotApplied,
            shims_dir: PathBuf::from("~/.dblocal/bin"),
            applied: false,
            findings: Vec::new(),
            block_preview: String::new(),
            target_files: Vec::new(),
            last_report: None,
            last_error: None,
            busy: false,
        }
    }
}

impl PathPanelState {
    /// Builds the PATH panel state from doctor findings and target files.
    pub fn from_doctor(
        shims_dir: PathBuf,
        applied: bool,
        findings: &[Finding],
        block_preview: String,
        target_files: Vec<String>,
    ) -> Self {
        let mut has_missing = false;
        let mut has_warning = false;
        let views = findings
            .iter()
            .map(|f| {
                let severe = matches!(
                    f,
                    Finding::ShadowedBinary { .. }
                        | Finding::DanglingShim { .. }
                        | Finding::ShimsDirNotFirst { .. }
                        | Finding::StaleBlock { .. }
                );
                if matches!(f, Finding::ShimsDirMissing { .. }) {
                    has_missing = true;
                }
                if severe {
                    has_warning = true;
                }
                PathFindingView {
                    code: f.code(),
                    message: format_finding_plain(f),
                    severe,
                }
            })
            .collect::<Vec<_>>();

        let health = if has_warning {
            PathHealth::NeedsAttention
        } else if has_missing {
            PathHealth::NotApplied
        } else {
            PathHealth::AppliedAndFirst
        };

        Self {
            health,
            shims_dir,
            applied,
            findings: views,
            block_preview,
            target_files,
            last_report: None,
            last_error: None,
            busy: false,
        }
    }
}

/// Formats a `Finding` in concise, GUI-friendly English (no CLI-only commands as
/// the primary fix).
pub fn format_finding_plain(finding: &Finding) -> String {
    match finding {
        Finding::ShimsDirMissing { dir } => {
            format!("DBLocal shims directory ({dir}) is not on PATH.")
        }
        Finding::ShimsDirNotFirst { dir, before } => format!(
            "DBLocal shims directory ({dir}) is not first on PATH — {before} appears before it."
        ),
        Finding::ShadowedBinary { name, earlier } => format!(
            "Another {name} at {earlier} comes earlier on PATH and shadows the DBLocal shim."
        ),
        Finding::DanglingShim { shim, target } => format!(
            "Shim {shim} points to {target}, which no longer exists. Remove or reinstall that version."
        ),
        Finding::StaleBlock { dir } => format!(
            "The managed PATH block points to {dir} instead of the current shims directory. Re-apply PATH to refresh it."
        ),
    }
}

/// Metadata for one PHP settings row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhpSettingMeta {
    pub key: SettingKey,
    pub label: &'static str,
    pub description: &'static str,
    pub placeholder: &'static str,
}

/// The six managed PHP settings in display order.
pub const PHP_SETTINGS_META: [PhpSettingMeta; 6] = [
    PhpSettingMeta {
        key: SettingKey::MemoryLimit,
        label: "memory_limit",
        description: "Maximum memory a PHP script may allocate (e.g. 512M, 1G, -1).",
        placeholder: "512M",
    },
    PhpSettingMeta {
        key: SettingKey::UploadMaxFilesize,
        label: "upload_max_filesize",
        description: "Maximum size of an uploaded file (e.g. 64M, 128M).",
        placeholder: "64M",
    },
    PhpSettingMeta {
        key: SettingKey::PostMaxSize,
        label: "post_max_size",
        description: "Maximum size of POST request data (should be >= upload_max_filesize).",
        placeholder: "64M",
    },
    PhpSettingMeta {
        key: SettingKey::MaxExecutionTime,
        label: "max_execution_time",
        description: "Maximum script execution time in seconds (0 for unlimited).",
        placeholder: "120",
    },
    PhpSettingMeta {
        key: SettingKey::DateTimezone,
        label: "date.timezone",
        description: "Default IANA timezone for date/time functions (e.g. UTC, Asia/Jakarta).",
        placeholder: "UTC",
    },
    PhpSettingMeta {
        key: SettingKey::DisplayErrors,
        label: "display_errors",
        description: "Print PHP errors to output during local development.",
        placeholder: "On",
    },
];

/// Pure state for the PHP settings card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhpSettingsForm {
    /// Persisted settings read from `99-dblocal.ini`.
    pub saved: PhpSettings,
    /// Current text values for the editable keys.
    pub draft: BTreeMap<SettingKey, String>,
    /// Per-field validation errors from `PhpSettings::parse_value`.
    pub errors: BTreeMap<SettingKey, String>,
    /// Directory containing `99-dblocal.ini`.
    pub config_dir: PathBuf,
    /// Whether a Save/Reset/Open call is in flight.
    pub busy: bool,
    /// Status message after save/reset.
    pub notice: Option<String>,
    /// Error message from save/reset/open.
    pub error: Option<String>,
}

impl Default for PhpSettingsForm {
    fn default() -> Self {
        Self::from_settings(PhpSettings::default(), PathBuf::new())
    }
}

impl PhpSettingsForm {
    /// Builds a clean form initialized from `saved`.
    pub fn from_settings(saved: PhpSettings, config_dir: PathBuf) -> Self {
        let mut draft = BTreeMap::new();
        for key in SettingKey::all() {
            draft.insert(key, saved.get(key).render());
        }
        Self {
            saved,
            draft,
            errors: BTreeMap::new(),
            config_dir,
            busy: false,
            notice: None,
            error: None,
        }
    }

    /// Updates one field's draft text and validates it through the core parser.
    pub fn set_field(&mut self, key: SettingKey, value: String) {
        self.notice = None;
        self.error = None;
        self.draft.insert(key, value.clone());
        match PhpSettings::parse_value(key, &value) {
            Ok(_) => {
                self.errors.remove(&key);
            }
            Err(err) => {
                self.errors
                    .insert(key, format_setting_error(key, &value, &err));
            }
        }
    }

    /// Toggles `display_errors`.
    pub fn set_display_errors(&mut self, enabled: bool) {
        self.set_field(
            SettingKey::DisplayErrors,
            if enabled { "On" } else { "Off" }.to_owned(),
        );
    }

    /// Current boolean value of `display_errors`.
    pub fn display_errors_on(&self) -> bool {
        self.draft
            .get(&SettingKey::DisplayErrors)
            .and_then(|raw| PhpSettings::parse_value(SettingKey::DisplayErrors, raw).ok())
            .is_some_and(|v| v == SettingValue::Toggle(true))
    }

    /// Current draft string for `key`.
    pub fn field_text(&self, key: SettingKey) -> String {
        self.draft
            .get(&key)
            .cloned()
            .unwrap_or_else(|| self.saved.get(key).render())
    }

    /// Whether any field differs from the saved settings.
    pub fn is_dirty(&self) -> bool {
        SettingKey::all().into_iter().any(|key| {
            let draft_val = self.field_text(key);
            let saved_val = self.saved.get(key).render();
            match PhpSettings::parse_value(key, &draft_val) {
                Ok(parsed) => parsed != *self.saved.get(key),
                Err(_) => draft_val != saved_val,
            }
        })
    }

    /// Whether Save is enabled (requires an active PHP, dirty edits, no errors, not busy).
    pub fn can_save(&self, php_active: bool) -> bool {
        php_active && !self.busy && self.is_dirty() && self.errors.is_empty()
    }

    /// Whether Reset to defaults is enabled.
    pub fn can_reset(&self, php_active: bool) -> bool {
        php_active && !self.busy
    }

    /// Validates all fields and returns the typed updates for `ToolchainService::set_php_settings`.
    pub fn validated_updates(&mut self) -> Result<Vec<(SettingKey, SettingValue)>, ()> {
        self.errors.clear();
        let mut updates = Vec::new();
        for key in SettingKey::all() {
            let raw = self.field_text(key);
            match PhpSettings::parse_value(key, &raw) {
                Ok(val) => updates.push((key, val)),
                Err(err) => {
                    self.errors
                        .insert(key, format_setting_error(key, &raw, &err));
                }
            }
        }
        if self.errors.is_empty() {
            Ok(updates)
        } else {
            Err(())
        }
    }
}

/// Formats a core PHP-setting parse error as a complete sentence that names the
/// offending value (never a dangling lowercase fragment like "is not a known
/// timezone").
fn format_setting_error(key: SettingKey, raw: &str, err: &dblocal_core::DblError) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return format!("{} cannot be empty.", key.as_str());
    }
    let reason = match err {
        dblocal_core::DblError::InvalidInput { reason, .. } => reason.as_str(),
        other => {
            return format!(
                "'{trimmed}' is not valid for {}: {}",
                key.as_str(),
                other.user_message()
            );
        }
    };
    if reason.contains(trimmed) && !starts_with_lowercase_fragment(reason) {
        return reason.to_owned();
    }
    if reason.starts_with("is ") || reason.starts_with("must ") {
        format!("'{trimmed}' {reason}")
    } else {
        format!("'{trimmed}' is not valid: {reason}")
    }
}

fn starts_with_lowercase_fragment(text: &str) -> bool {
    text.chars().next().is_some_and(|c| c.is_ascii_lowercase())
}

/// Confirmation dialog payload for removing an installed toolchain version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainRemoveDialog {
    pub tool: ToolId,
    pub version: String,
    pub profile: Option<Profile>,
    pub size_bytes: u64,
    pub is_active: bool,
    pub others_remain: bool,
}

impl ToolchainRemoveDialog {
    /// Plain-English explanation of what removing this version will do.
    pub fn consequence_text(&self) -> String {
        let name = tool_display_name(self.tool);
        if self.is_active {
            if self.others_remain {
                format!(
                    "This is your active {name} version. Removing it clears the active entry and \
                     removes its shim. DBLocal will not auto-switch to another installed version."
                )
            } else {
                format!(
                    "This is your active {name} version. Removing it clears the active entry and \
                     removes its shim."
                )
            }
        } else {
            format!(
                "This removes the installed {name} {} files from disk. You can install it again later.",
                self.version
            )
        }
    }
}
