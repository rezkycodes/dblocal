//! Pure view-model helpers. No GPUI types, no I/O — every function here is
//! unit-tested and drives the render code.
//!
//! The prototype hard-codes its rows in `model.rs`; this module is the ported
//! equivalent that derives the same decisions from the frozen service's read
//! models (`InstanceView`, `CatalogView`, `Runtime`).

use dblocal_core::model::{
    AuthMode, CatalogStatus, CredentialBackend, EngineKey, InstanceStatus, Runtime,
};
use dblocal_core::service::{CatalogView, EngineEntry, InstanceView};
use dblocal_core::strings;

/// The four run-states a status glyph encodes (`F-PLH-02` AC2).
///
/// The prototype draws a different shape per state, so status never depends on
/// colour alone: filled circle, hollow circle, filled triangle, half circle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusGlyph {
    /// A process is running.
    Running,
    /// The instance is idle and ready to start.
    Stopped,
    /// The last operation failed.
    Failed,
    /// A transition is in flight (provisioning, starting, stopping, deleting).
    Busy,
}

/// Maps a lifecycle status to the glyph and colour class it draws.
pub fn status_glyph(status: InstanceStatus) -> StatusGlyph {
    match status {
        InstanceStatus::Running => StatusGlyph::Running,
        InstanceStatus::Failed => StatusGlyph::Failed,
        InstanceStatus::Provisioning
        | InstanceStatus::Initializing
        | InstanceStatus::Starting
        | InstanceStatus::Stopping
        | InstanceStatus::Deleting => StatusGlyph::Busy,
        _ => StatusGlyph::Stopped,
    }
}

/// Human-readable status label shown next to the status dot (`F-PLH-02` AC2:
/// status is never colour alone).
pub fn status_label(status: InstanceStatus) -> &'static str {
    match status {
        InstanceStatus::Running => strings::STATUS_RUNNING,
        InstanceStatus::Stopped => strings::STATUS_STOPPED,
        InstanceStatus::Failed => strings::STATUS_FAILED,
        InstanceStatus::Provisioning => strings::STATUS_PROVISIONING,
        InstanceStatus::Initializing => strings::STATUS_INITIALIZING,
        InstanceStatus::Starting => strings::STATUS_STARTING,
        InstanceStatus::Stopping => strings::STATUS_STOPPING,
        InstanceStatus::Deleting => strings::STATUS_DELETING,
        InstanceStatus::Draft => strings::STATUS_DRAFT,
        _ => strings::STATUS_STOPPED,
    }
}

/// Whether the engine is running (or transitioning into it).
pub fn is_running(status: InstanceStatus) -> bool {
    matches!(
        status,
        InstanceStatus::Running | InstanceStatus::Starting | InstanceStatus::Stopping
    )
}

/// Whether a transition is in flight, so actions must be disabled.
pub fn is_busy(status: InstanceStatus) -> bool {
    status_glyph(status) == StatusGlyph::Busy
}

/// Whether Start is offered right now.
pub fn can_start(status: InstanceStatus) -> bool {
    matches!(status, InstanceStatus::Stopped | InstanceStatus::Failed)
}

/// Whether Stop is offered right now.
pub fn can_stop(status: InstanceStatus) -> bool {
    status == InstanceStatus::Running
}

/// Whether Restart is offered right now (it stops then starts).
pub fn can_restart(status: InstanceStatus) -> bool {
    status == InstanceStatus::Running
}

/// Whether Delete is offered right now (`F-INS-05` needs a settled status).
pub fn can_delete(status: InstanceStatus) -> bool {
    matches!(status, InstanceStatus::Stopped | InstanceStatus::Failed)
}

/// Two-letter monogram the engine badge draws (`PG`, `MY`, `MA`, `VK`).
pub fn engine_monogram(engine: EngineKey) -> &'static str {
    match engine {
        EngineKey::Postgresql => "PG",
        EngineKey::Mysql => "MY",
        EngineKey::Mariadb => "MA",
        EngineKey::Valkey => "VK",
        _ => "??",
    }
}

/// One engine group in the instances dock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineGroup<'a> {
    /// The engine this group lists.
    pub engine: EngineKey,
    /// Display label, from the domain's canonical names.
    pub label: &'static str,
    /// Instances of this engine, sorted by name.
    pub instances: Vec<&'a InstanceView>,
}

/// Groups instances by engine in the canonical engine order, skipping empty
/// groups. Names are sorted so the tree is stable across refreshes.
pub fn group_by_engine(instances: &[InstanceView]) -> Vec<EngineGroup<'_>> {
    let mut groups = Vec::new();
    for engine in EngineKey::ALL {
        let mut members: Vec<&InstanceView> = instances
            .iter()
            .filter(|view| view.engine == engine)
            .collect();
        members.sort_by(|a, b| a.name.cmp(&b.name));
        if members.is_empty() {
            continue;
        }
        groups.push(EngineGroup {
            engine,
            label: engine.display_name(),
            instances: members,
        });
    }
    groups
}

/// Groups an already-filtered list of borrowed views.
///
/// The dock filters first and groups second, so it does not clone rows just to
/// satisfy a signature.
pub fn group_by_engine_refs<'a>(visible: &[&'a InstanceView]) -> Vec<EngineGroup<'a>> {
    let mut groups = Vec::new();
    for engine in EngineKey::ALL {
        let mut members: Vec<&'a InstanceView> = visible
            .iter()
            .copied()
            .filter(|view| view.engine == engine)
            .collect();
        members.sort_by(|a, b| a.name.cmp(&b.name));
        if members.is_empty() {
            continue;
        }
        groups.push(EngineGroup {
            engine,
            label: engine.display_name(),
            instances: members,
        });
    }
    groups
}

/// Filters instances by a case-insensitive name substring.
///
/// An empty query keeps everything, which is what the dock's filter field
/// starts as.
pub fn filter_instances<'a>(instances: &'a [InstanceView], query: &str) -> Vec<&'a InstanceView> {
    let needle = query.trim().to_lowercase();
    instances
        .iter()
        .filter(|view| needle.is_empty() || view.name.to_lowercase().contains(&needle))
        .collect()
}

/// Counts instances that currently hold a process.
pub fn running_count(instances: &[InstanceView]) -> usize {
    instances
        .iter()
        .filter(|view| view.status == InstanceStatus::Running)
        .count()
}

/// Counts instances in a settled idle status.
pub fn stopped_count(instances: &[InstanceView]) -> usize {
    instances
        .iter()
        .filter(|view| view.status == InstanceStatus::Stopped)
        .count()
}

/// Counts instances whose last operation failed.
pub fn failed_count(instances: &[InstanceView]) -> usize {
    instances
        .iter()
        .filter(|view| view.status == InstanceStatus::Failed)
        .count()
}

/// The three counts the status bar prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InstanceCounts {
    /// Instances with a live process.
    pub running: usize,
    /// Instances idle and ready.
    pub stopped: usize,
    /// Instances whose last operation failed.
    pub failed: usize,
}

impl InstanceCounts {
    /// Whether there is nothing to report.
    pub fn is_empty(&self) -> bool {
        self.running == 0 && self.stopped == 0 && self.failed == 0
    }
}

/// Counts every instance by the status bar's three buckets.
pub fn instance_counts(instances: &[InstanceView]) -> InstanceCounts {
    InstanceCounts {
        running: running_count(instances),
        stopped: stopped_count(instances),
        failed: failed_count(instances),
    }
}

/// Formats a byte count with a unit, e.g. `281 MB`, `1.4 GB`.
pub fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let bytes_f = bytes as f64;
    if bytes_f < KIB {
        return format!("{bytes} B");
    }
    let units = ["KB", "MB", "GB", "TB"];
    let mut value = bytes_f / KIB;
    let mut unit = 0;
    while value >= KIB && unit + 1 < units.len() {
        value /= KIB;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", units[unit])
    } else if value >= 10.0 {
        format!("{value:.1} {}", units[unit])
    } else {
        format!("{value:.2} {}", units[unit])
    }
}

/// Formats a byte count in the Indonesian decimal style the prototype uses.
///
/// `7.5 MB` becomes `7,5 MB`, so a size reads as a sentence rather than a
/// machine figure.
/// Abbreviate a path by replacing the home prefix with `~`.
pub fn abbreviate_home(path: &std::path::Path, home: Option<&std::path::Path>) -> String {
    if let Some(home) = home
        && let Ok(rest) = path.strip_prefix(home)
    {
        if rest.as_os_str().is_empty() {
            return "~".to_owned();
        }
        let rest = rest.to_string_lossy().replace('\\', "/");
        return format!("~/{rest}");
    }
    path.to_string_lossy().into_owned()
}

/// Engine log file path, derived from the real home root (R9).
pub fn engine_log_path(home_root: &std::path::Path, name: &str) -> String {
    home_root
        .join("instances")
        .join(name)
        .join("logs")
        .join("engine.log")
        .to_string_lossy()
        .into_owned()
}

pub fn format_bytes_id(bytes: u64) -> String {
    format_bytes(bytes).replace('.', ",")
}

/// Formats a transfer rate, e.g. `13.0 MB/s`.
pub fn format_speed(bytes_per_sec: u64) -> String {
    format!("{}/s", format_bytes(bytes_per_sec))
}

/// Formats a duration as `45s`, `6m 5s` or `2h 3m`.
pub fn format_eta(seconds: u64) -> String {
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    let secs = seconds % 60;
    if minutes < 60 {
        if secs == 0 {
            format!("{minutes}m")
        } else {
            format!("{minutes}m {secs}s")
        }
    } else {
        let hours = minutes / 60;
        let mins = minutes % 60;
        format!("{hours}h {mins}m")
    }
}

/// The compact `version · :port` meta line of a dock row.
pub fn dock_meta(view: &InstanceView) -> String {
    format!("{} · :{}", short_version(&view.version), view.port)
}

/// Truncates a version tag to its `major.minor`, as the dock row does.
pub fn short_version(version: &str) -> String {
    version.split('.').take(2).collect::<Vec<_>>().join(".")
}

/// The instance header's subtitle: engine, version, endpoint, state, process mode.
pub fn header_subtitle(view: &InstanceView) -> String {
    let state = status_label(view.status);
    format!(
        "{} {} · 127.0.0.1:{} · {} · detached",
        view.engine.display_name(),
        view.version,
        view.port,
        state
    )
}

/// Data directory of an instance, derived from the real home root (R9).
pub fn data_dir(home_root: &std::path::Path, name: &str) -> String {
    home_root
        .join("instances")
        .join(name)
        .join("data")
        .to_string_lossy()
        .into_owned()
}

/// Credential reference shown for an instance (`dblocal:<name>`).
pub fn credential_ref(name: &str) -> String {
    format!("dblocal:{name}")
}

/// Human label for where an instance's credential lives (`F-CRD-01` AC2).
pub fn credential_backend_label(backend: Option<CredentialBackend>) -> &'static str {
    match backend {
        Some(CredentialBackend::File) => strings::UI_CREDENTIAL_FILE,
        _ => strings::UI_CREDENTIAL_KEYCHAIN,
    }
}

/// Whether the credential fallback note must show (`F-CRD-01` AC2).
pub fn credential_is_file(backend: Option<CredentialBackend>) -> bool {
    backend == Some(CredentialBackend::File)
}

/// Engine version plus architecture, as the Detail card prints it.
pub fn engine_detail(view: &InstanceView, arch: &str) -> String {
    format!("{} {} · {}", view.engine.display_name(), view.version, arch)
}

/// Whether the instance sits on its engine's canonical port.
pub fn is_canonical_port(view: &InstanceView) -> bool {
    view.port.get() == view.engine.canonical_port()
}

/// Description of one download shown in the status-bar popover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadRow {
    /// Display title, e.g. `PostgreSQL 17.4`.
    pub title: String,
    /// Percent complete, 0-100.
    pub percent: u8,
    /// Human readable progress, e.g. `256 MB of 512 MB`.
    pub detail: String,
    /// Remaining-time label.
    pub eta: Option<String>,
}

/// Builds a popover row from a progress sample.
pub fn download_row(
    engine: EngineKey,
    version: &str,
    done: u64,
    total: u64,
    speed: u64,
    eta_secs: Option<u64>,
) -> DownloadRow {
    let percent = done
        .saturating_mul(100)
        .checked_div(total)
        .map(|value| value.min(100) as u8)
        .unwrap_or(0);
    DownloadRow {
        title: format!("{} {}", engine.display_name(), version),
        percent,
        detail: format!(
            "{} / {}  ·  {}",
            format_bytes(done),
            format_bytes(total),
            format_speed(speed)
        ),
        eta: eta_secs.map(format_eta),
    }
}

/// One line of a rendered `.env` block, split so the key can be accented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvLine {
    /// The variable name, or `None` for a blank or comment line.
    pub key: Option<String>,
    /// Everything after the `=`, or the whole line when there is no `=`.
    pub value: String,
}

/// Splits a `.env` block into key/value rows for the preview.
pub fn env_lines(block: &str) -> Vec<EnvLine> {
    block
        .lines()
        .map(|line| match line.split_once('=') {
            Some((key, value)) => EnvLine {
                key: Some(key.to_owned()),
                value: value.to_owned(),
            },
            None => EnvLine {
                key: None,
                value: line.to_owned(),
            },
        })
        .collect()
}

/// Whether the URI can be shown at all (an instance without auth still has one).
pub fn uri_available() -> bool {
    true
}

/// Active banner text from the catalog read model (`F-CAT-01`).
pub fn catalog_banner(catalog: &CatalogView) -> Option<&str> {
    catalog.banner.as_deref()
}

/// Human label for the catalog freshness indicator.
///
/// Returns the Indonesian words from `strings`, never the raw enum tag.
pub fn catalog_status_label(catalog: &CatalogView) -> &'static str {
    match catalog.status {
        CatalogStatus::Fresh => strings::CATALOG_FRESH_LABEL,
        CatalogStatus::Stale => strings::CATALOG_STALE_LABEL,
        CatalogStatus::Offline => strings::CATALOG_OFFLINE_LABEL,
        _ => strings::CATALOG_FRESH_LABEL,
    }
}

/// The single word the title bar shows for catalog freshness.
pub fn catalog_freshness_word(status: CatalogStatus) -> &'static str {
    match status {
        CatalogStatus::Fresh => strings::CATALOG_FRESH_LABEL,
        CatalogStatus::Stale => strings::CATALOG_STALE_LABEL,
        CatalogStatus::Offline => strings::CATALOG_OFFLINE_LABEL,
        _ => strings::CATALOG_FRESH_LABEL,
    }
}

/// Human sentence for a start diagnosis, next to its `DBL-NNNN` code.
pub fn diagnosis_label(diagnosis: dblocal_core::model::StartFailure) -> &'static str {
    use dblocal_core::model::StartFailure as D;
    match diagnosis {
        D::PortInUse => strings::DIAGNOSIS_PORT_IN_USE,
        D::DataDirVersionMismatch => strings::DIAGNOSIS_VERSION_MISMATCH,
        D::PermissionDenied => strings::DIAGNOSIS_PERMISSION_DENIED,
        D::BinaryMissing => strings::DIAGNOSIS_BINARY_MISSING,
        D::Unknown => strings::DIAGNOSIS_UNKNOWN,
        _ => strings::DIAGNOSIS_UNKNOWN,
    }
}

/// Whether a failed instance's banner offers the port fix (`F-LOG-02` AC1).
///
/// Only the typed port-in-use failure gets the one-click "move to the next port
/// and start"; every other diagnosis stays a text hint.
pub fn offers_port_fix(view: &InstanceView) -> bool {
    view.status == InstanceStatus::Failed
        && view.failure.as_ref().is_some_and(|failure| {
            failure.diagnosis == dblocal_core::model::StartFailure::PortInUse
        })
}

/// The port the log banner offers to move to: the suggested free port, or the
/// instance's own port plus one when the service could not propose another.
pub fn suggested_fix_port(view: &InstanceView, service_suggestion: Option<u16>) -> u16 {
    service_suggestion
        .filter(|port| *port != view.port.get())
        .unwrap_or_else(|| view.port.get().saturating_add(1))
}

/// Scripted failures the delete dialog warns about.
pub fn delete_needs_stop(status: InstanceStatus) -> bool {
    !can_delete(status)
}

/// One runtime row in the dock: what the row shows and how it is worded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeRow {
    /// `PostgreSQL 17.4`, as the row prints it.
    pub title: String,
    /// How many instances share this runtime.
    pub users: usize,
    /// Whether nothing references it (`F-RTM-04`).
    pub unused: bool,
    /// The right-hand cell text.
    pub usage: String,
}

/// Builds one runtime row from the runtime list and the instance list.
pub fn runtime_row(runtime: &Runtime, instances: &[InstanceView]) -> RuntimeRow {
    let users = instances
        .iter()
        .filter(|view| view.engine == runtime.engine() && view.version == runtime.version())
        .count();
    let unused = users == 0;
    let usage = if unused {
        strings::UI_RUNTIME_UNUSED.to_owned()
    } else {
        format!("{users} {}", strings::UI_RUNTIME_INSTANCE_COUNT)
    };
    RuntimeRow {
        title: format!("{} {}", runtime.engine().display_name(), runtime.version()),
        users,
        unused,
        usage,
    }
}

/// Every runtime row, sorted by engine then version.
pub fn runtime_rows(runtimes: &[Runtime], instances: &[InstanceView]) -> Vec<RuntimeRow> {
    let mut sorted: Vec<&Runtime> = runtimes.iter().collect();
    sorted.sort_by(|a, b| {
        a.engine()
            .as_str()
            .cmp(b.engine().as_str())
            .then_with(|| a.version().cmp(b.version()))
    });
    sorted
        .into_iter()
        .map(|runtime| runtime_row(runtime, instances))
        .collect()
}

/// Convenience wrapper so the GUI never imports the runtime type name twice.
pub fn runtime_engine(runtime: &Runtime) -> EngineKey {
    runtime.engine()
}

/// One command palette entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteAction {
    /// Open the create-instance dialog.
    NewInstance,
    /// Open an instance tab.
    Open(String),
    /// Start an instance.
    Start(String),
    /// Stop an instance.
    Stop(String),
    /// Copy an instance URI.
    CopyUri(String),
}

/// A palette row: a label and the action it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteItem {
    /// Text shown and searched.
    pub label: String,
    /// Action to run on confirm.
    pub action: PaletteAction,
}

/// Builds the full palette list for the current instances.
pub fn palette_items(instances: &[InstanceView]) -> Vec<PaletteItem> {
    let mut items = vec![PaletteItem {
        label: strings::UI_PALETTE_NEW.to_owned(),
        action: PaletteAction::NewInstance,
    }];
    for view in instances {
        if matches!(
            view.status,
            InstanceStatus::Stopped | InstanceStatus::Failed
        ) {
            items.push(PaletteItem {
                label: format!("{} {}", strings::UI_PALETTE_START, view.name),
                action: PaletteAction::Start(view.id.to_string()),
            });
        }
        if view.status == InstanceStatus::Running {
            items.push(PaletteItem {
                label: format!("{} {}", strings::UI_PALETTE_STOP, view.name),
                action: PaletteAction::Stop(view.id.to_string()),
            });
        }
        items.push(PaletteItem {
            label: format!("{} {}", strings::UI_PALETTE_OPEN, view.name),
            action: PaletteAction::Open(view.id.to_string()),
        });
        items.push(PaletteItem {
            label: format!("{} {}", strings::UI_PALETTE_COPY_URI, view.name),
            action: PaletteAction::CopyUri(view.id.to_string()),
        });
    }
    items
}

/// Case-insensitive substring filter used by the palette.
pub fn filter_palette<'a>(items: &'a [PaletteItem], query: &str) -> Vec<&'a PaletteItem> {
    let query = query.trim().to_lowercase();
    items
        .iter()
        .filter(|item| query.is_empty() || item.label.to_lowercase().contains(&query))
        .collect()
}

/// Result of validating the create form before it is submitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormValidation {
    /// Field-level error for the name, if any.
    pub name_error: Option<String>,
    /// Field-level error for the port, if any.
    pub port_error: Option<String>,
    /// Human-readable message naming the conflicting instance, if any.
    pub conflict: Option<String>,
}

impl FormValidation {
    /// Whether the form can be submitted.
    pub fn is_valid(&self) -> bool {
        self.name_error.is_none() && self.port_error.is_none() && self.conflict.is_none()
    }
}

/// Validates the create form against the current instance list.
///
/// Name rules mirror `InstanceName`; the port rule mirrors `F-PRT-01` AC3.
pub fn validate_form(name: &str, port_text: &str, instances: &[InstanceView]) -> FormValidation {
    let name_error = match dblocal_core::model::InstanceName::new(name.to_owned()) {
        Ok(_) => None,
        Err(_) => Some(strings::UI_VALIDATE_NAME.to_owned()),
    };
    let mut conflict = None;
    let port_error = if port_text.trim().is_empty() {
        None
    } else {
        match port_text.trim().parse::<u16>() {
            Ok(value) if value < dblocal_core::consts::MIN_PORT => {
                Some(strings::UI_VALIDATE_PORT_LOW.to_owned())
            }
            Ok(value) => {
                if let Some(owner) = instances.iter().find(|view| view.port.get() == value) {
                    let message = strings::validate_port_taken(value, &owner.name);
                    conflict = Some(message.clone());
                    Some(message)
                } else {
                    None
                }
            }
            Err(_) => Some(strings::UI_VALIDATE_PORT_LOW.to_owned()),
        }
    };
    FormValidation {
        name_error,
        port_error,
        conflict,
    }
}

/// Short reason why an engine cannot be chosen, or `None` when selectable.
pub fn engine_unavailable_reason(entry: &EngineEntry) -> Option<&str> {
    entry.unavailable_reason.as_deref()
}

/// Why a version cannot be installed here, or `None` when it can.
///
/// `F-CAT-02` AC2 wants an unavailable combination shown, not hidden; the
/// reason is the same sentence the catalog carries for the engine.
pub fn version_unavailable_reason(entry: &EngineEntry) -> Option<&str> {
    if entry.available {
        None
    } else {
        entry
            .unavailable_reason
            .as_deref()
            .or(Some(strings::CATALOG_ENGINE_UNAVAILABLE_PLATFORM))
    }
}

/// Whether an auth mode requires a password.
pub fn auth_needs_password(auth: AuthMode) -> bool {
    auth.requires_password()
}

/// Whether the create dialog's footer must warn about a download (`F-INS-01` AC2).
pub fn create_needs_download(version_installed: bool) -> bool {
    !version_installed
}

/// Builds the suggested instance name for an engine and version
/// (`postgres-17`, `valkey-8`, ...), as the prototype pre-fills it.
pub fn suggested_name(engine: EngineKey, version: &str) -> String {
    let prefix = match engine {
        EngineKey::Postgresql => "postgres",
        EngineKey::Mysql => "mysql",
        EngineKey::Mariadb => "mariadb",
        EngineKey::Valkey => "valkey",
        _ => "instance",
    };
    let major = version.split('.').next().unwrap_or(version);
    format!("{prefix}-{major}")
}

/// Next free port for an engine, skipping ports already allocated in the list.
pub fn next_free_port(instances: &[InstanceView], engine: EngineKey) -> u16 {
    let mut port = engine.canonical_port();
    while instances.iter().any(|view| view.port.get() == port) {
        port = port.saturating_add(1);
    }
    port
}

/// The version a freshly opened create dialog should select for an engine.
///
/// Newest non-EOL entry first, falling back to whatever the catalog lists.
pub fn default_version(entry: &EngineEntry) -> Option<&dblocal_core::service::VersionEntry> {
    entry
        .versions
        .iter()
        .find(|version| !version.eol)
        .or_else(|| entry.versions.first())
}

/// Formats the create dialog's version-list heading.
pub fn create_versions(engine: &str) -> String {
    strings::create_versions(engine)
}

/// Formats the "Unduh, <size>" badge on a catalog version.
pub fn create_download_badge(size: &str) -> String {
    strings::create_download_badge(size)
}

/// The index of the default version in the entry's list, for the radio rows.
pub fn default_version_index(entry: &EngineEntry) -> usize {
    entry
        .versions
        .iter()
        .position(|version| !version.eol)
        .unwrap_or(0)
}

/// The `os arch` cell of the status bar.
pub fn platform_label(os: &str, arch: &str) -> String {
    format!("{os} {arch}")
}

mod masking;
mod placeholders;
pub use masking::*;
pub use placeholders::*;

#[cfg(test)]
#[path = "masking_tests.rs"]
mod masking_tests;
#[cfg(test)]
#[path = "tests.rs"]
mod tests;
