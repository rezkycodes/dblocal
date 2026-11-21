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
        "{}.{} · 127.0.0.1:{} · {} · detached",
        view.engine.display_name(),
        view.version,
        view.port,
        state
    )
}

/// Data directory of an instance, as the UI prints it.
pub fn data_dir(name: &str) -> String {
    format!("~/.dblocal/instances/{name}/data")
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

/// Masks the password inside a connection URI, keeping the shape readable.
///
/// `postgres://postgres:hunter2@127.0.0.1:5432/db` becomes
/// `postgres://postgres:••••••@127.0.0.1:5432/db`.
pub fn mask_uri(uri: &str) -> String {
    let Some((scheme_sep, _)) = uri.split_once("://") else {
        return uri.to_owned();
    };
    let after_scheme = scheme_sep.len() + 3;
    let rest = &uri[after_scheme..];
    let Some(at) = rest.rfind('@') else {
        return uri.to_owned();
    };
    let credentials = &rest[..at];
    let Some((user, _password)) = credentials.split_once(':') else {
        // Redis without a user has no colon before the `@`; leave as-is.
        return uri.to_owned();
    };
    format!(
        "{}://{user}:••••••{}",
        &uri[..scheme_sep.len()],
        &rest[at..]
    )
}

/// Masks only the password value in a `.env` block.
///
/// Both the framework block (`DB_PASSWORD`) and the Valkey block
/// (`REDIS_PASSWORD`) are covered, so neither shapes leaks a secret.
pub fn mask_dotenv(block: &str) -> String {
    block
        .lines()
        .map(|line| {
            for prefix in ["DB_PASSWORD=", "REDIS_PASSWORD="] {
                if let Some(rest) = line.strip_prefix(prefix) {
                    return if rest.is_empty() {
                        line.to_owned()
                    } else {
                        format!("{prefix}••••••")
                    };
                }
            }
            line.to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
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
        && view
            .failure
            .as_ref()
            .is_some_and(|failure| failure.diagnosis == dblocal_core::model::StartFailure::PortInUse)
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

#[cfg(test)]
mod tests {
    use dblocal_core::model::{
        AuthMode, CatalogStatus, CredentialBackend, EngineKey, InstanceId, InstanceName,
        InstanceSpec, InstanceStatus, Port, Runtime, RuntimeState, StartFailure,
    };
    use dblocal_core::platform::Arch;
    use dblocal_core::service::FailureInfo;

    use super::*;

    fn view(name: &str, engine: EngineKey, status: InstanceStatus, port: u16) -> InstanceView {
        view_version(name, engine, "17.4", status, port)
    }

    fn view_version(
        name: &str,
        engine: EngineKey,
        version: &str,
        status: InstanceStatus,
        port: u16,
    ) -> InstanceView {
        let spec = InstanceSpec::new(
            InstanceName::new(name).expect("valid name"),
            engine,
            version,
            Some(Port::new(port).expect("valid port")),
            AuthMode::Password,
        )
        .expect("spec");
        let now = std::time::SystemTime::UNIX_EPOCH;
        let instance = dblocal_core::model::Instance::new(&spec, InstanceId::new(), status, now);
        InstanceView::from_instance(&instance)
    }

    fn runtime(engine: EngineKey, version: &str) -> Runtime {
        Runtime::new(
            format!("rt-{}-{version}", engine.as_str()),
            engine,
            version,
            Arch::X86_64,
            std::path::PathBuf::from("/tmp/runtime"),
            40 * 1024 * 1024,
            std::time::SystemTime::UNIX_EPOCH,
            RuntimeState::Installed,
        )
    }

    #[test]
    fn every_status_has_a_glyph_and_a_nonempty_label() {
        for status in [
            InstanceStatus::Running,
            InstanceStatus::Stopped,
            InstanceStatus::Failed,
            InstanceStatus::Provisioning,
            InstanceStatus::Initializing,
            InstanceStatus::Starting,
            InstanceStatus::Stopping,
            InstanceStatus::Deleting,
            InstanceStatus::Draft,
        ] {
            assert!(!status_label(status).is_empty(), "{status:?}");
        }
        assert_eq!(
            status_label(InstanceStatus::Running),
            strings::STATUS_RUNNING
        );
        assert_eq!(status_glyph(InstanceStatus::Running), StatusGlyph::Running);
        assert_eq!(status_glyph(InstanceStatus::Stopped), StatusGlyph::Stopped);
        assert_eq!(status_glyph(InstanceStatus::Failed), StatusGlyph::Failed);
        for busy in [
            InstanceStatus::Provisioning,
            InstanceStatus::Initializing,
            InstanceStatus::Starting,
            InstanceStatus::Stopping,
            InstanceStatus::Deleting,
        ] {
            assert_eq!(status_glyph(busy), StatusGlyph::Busy, "{busy:?}");
            assert!(is_busy(busy));
        }
        assert!(!is_busy(InstanceStatus::Running));
    }

    #[test]
    fn actions_follow_the_state_machine() {
        assert!(can_start(InstanceStatus::Stopped));
        assert!(can_start(InstanceStatus::Failed));
        assert!(!can_start(InstanceStatus::Running));
        assert!(can_stop(InstanceStatus::Running));
        assert!(!can_stop(InstanceStatus::Stopped));
        assert!(can_restart(InstanceStatus::Running));
        assert!(!can_restart(InstanceStatus::Failed));
        assert!(can_delete(InstanceStatus::Stopped));
        assert!(can_delete(InstanceStatus::Failed));
        assert!(!can_delete(InstanceStatus::Running));
        assert!(delete_needs_stop(InstanceStatus::Running));
        assert!(!delete_needs_stop(InstanceStatus::Stopped));
    }

    #[test]
    fn monograms_match_the_prototype() {
        assert_eq!(engine_monogram(EngineKey::Postgresql), "PG");
        assert_eq!(engine_monogram(EngineKey::Mysql), "MY");
        assert_eq!(engine_monogram(EngineKey::Mariadb), "MA");
        assert_eq!(engine_monogram(EngineKey::Valkey), "VK");
    }

    #[test]
    fn grouping_skips_empty_engines_and_sorts_names() {
        let instances = vec![
            view("zeta", EngineKey::Postgresql, InstanceStatus::Stopped, 5432),
            view(
                "alpha",
                EngineKey::Postgresql,
                InstanceStatus::Running,
                5433,
            ),
            view("cache", EngineKey::Valkey, InstanceStatus::Running, 6379),
        ];
        let groups = group_by_engine(&instances);
        assert_eq!(groups.len(), 2, "mysql and mariadb are empty");
        assert_eq!(groups[0].engine, EngineKey::Postgresql);
        let names: Vec<&str> = groups[0]
            .instances
            .iter()
            .map(|v| v.name.as_str())
            .collect();
        assert_eq!(names, ["alpha", "zeta"]);
        assert_eq!(groups[1].label, "Valkey");
    }

    #[test]
    fn grouping_on_no_instances_is_empty() {
        assert!(group_by_engine(&[]).is_empty());
    }

    #[test]
    fn filtering_is_case_insensitive_and_keeps_everything_when_empty() {
        let instances = vec![
            view("pg-app", EngineKey::Postgresql, InstanceStatus::Running, 5432),
            view("cache-dev", EngineKey::Valkey, InstanceStatus::Running, 6379),
        ];
        assert_eq!(filter_instances(&instances, "").len(), 2);
        assert_eq!(filter_instances(&instances, "   ").len(), 2);
        let hits = filter_instances(&instances, "CACHE");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "cache-dev");
        assert!(filter_instances(&instances, "shop").is_empty());
    }

    #[test]
    fn counts_are_bucketed_by_status() {
        let instances = vec![
            view("aa-app", EngineKey::Postgresql, InstanceStatus::Running, 5432),
            view("bb-app", EngineKey::Postgresql, InstanceStatus::Stopped, 5433),
            view("cc-app", EngineKey::Postgresql, InstanceStatus::Failed, 5434),
            view("dd-cache", EngineKey::Valkey, InstanceStatus::Starting, 6379),
        ];
        let counts = instance_counts(&instances);
        assert_eq!(counts.running, 1);
        assert_eq!(counts.stopped, 1);
        assert_eq!(counts.failed, 1);
        assert!(!counts.is_empty());
        assert_eq!(running_count(&instances), 1);
        assert_eq!(stopped_count(&instances), 1);
        assert_eq!(failed_count(&instances), 1);
        assert!(instance_counts(&[]).is_empty());
    }

    #[test]
    fn byte_formatting_uses_sensible_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(54_525_952), "52.0 MB");
        assert_eq!(format_bytes(281_000_000), "268 MB");
        assert_eq!(format_bytes(2_147_483_648), "2.00 GB");
    }

    #[test]
    fn indonesian_byte_formatting_swaps_the_decimal_point() {
        assert_eq!(format_bytes_id(7_864_320), "7,50 MB");
        assert_eq!(format_bytes_id(31_457_280), "30,0 MB");
    }

    #[test]
    fn speed_and_eta_formatting() {
        assert_eq!(format_speed(13 * 1024 * 1024), "13.0 MB/s");
        assert_eq!(format_eta(0), "0s");
        assert_eq!(format_eta(45), "45s");
        assert_eq!(format_eta(65), "1m 5s");
        assert_eq!(format_eta(120), "2m");
        assert_eq!(format_eta(7_380), "2h 3m");
    }

    #[test]
    fn dock_meta_combines_short_version_and_port() {
        let v = view("app", EngineKey::Postgresql, InstanceStatus::Stopped, 5432);
        assert_eq!(dock_meta(&v), "17.4 · :5432");
        assert_eq!(short_version("18.7.0"), "18.7");
        assert_eq!(short_version("18"), "18");
    }

    #[test]
    fn header_subtitle_names_engine_endpoint_state_and_process_mode() {
        let v = view("app", EngineKey::Postgresql, InstanceStatus::Running, 5432);
        let subtitle = header_subtitle(&v);
        assert!(subtitle.contains("PostgreSQL.17.4"));
        assert!(subtitle.contains("127.0.0.1:5432"));
        assert!(subtitle.contains(strings::STATUS_RUNNING));
        assert!(subtitle.contains("detached"));
    }

    #[test]
    fn credential_labels_follow_the_backend() {
        assert_eq!(
            credential_backend_label(Some(CredentialBackend::Keychain)),
            strings::UI_CREDENTIAL_KEYCHAIN
        );
        assert_eq!(
            credential_backend_label(Some(CredentialBackend::File)),
            strings::UI_CREDENTIAL_FILE
        );
        assert_eq!(
            credential_backend_label(None),
            strings::UI_CREDENTIAL_KEYCHAIN
        );
        assert!(credential_is_file(Some(CredentialBackend::File)));
        assert!(!credential_is_file(Some(CredentialBackend::Keychain)));
        assert!(!credential_is_file(None));
    }

    #[test]
    fn detail_values_name_the_engine_and_mark_the_canonical_port() {
        let v = view("app", EngineKey::Postgresql, InstanceStatus::Running, 5432);
        assert_eq!(engine_detail(&v, "x86_64"), "PostgreSQL 17.4 · x86_64");
        assert!(is_canonical_port(&v));
        let off = view("app2", EngineKey::Postgresql, InstanceStatus::Running, 5433);
        assert!(!is_canonical_port(&off));
        assert_eq!(data_dir("app"), "~/.dblocal/instances/app/data");
        assert_eq!(credential_ref("app"), "dblocal:app");
    }

    #[test]
    fn download_row_reports_percent_detail_and_eta() {
        let row = download_row(
            EngineKey::Postgresql,
            "17.4",
            26 * 1024 * 1024,
            52 * 1024 * 1024,
            13 * 1024 * 1024,
            Some(2),
        );
        assert_eq!(row.percent, 50);
        assert!(row.title.contains("PostgreSQL"));
        assert!(row.detail.contains("26.0 MB"));
        assert!(row.detail.contains("MB/s"));
        assert_eq!(row.eta.as_deref(), Some("2s"));
        assert_eq!(
            download_row(EngineKey::Valkey, "8.1.0", 1, 0, 0, None).percent,
            0
        );
    }

    #[test]
    fn uri_masking_hides_the_password_only() {
        let masked =
            mask_uri("postgresql://postgres:hunter2@127.0.0.1:5432/postgres?sslmode=disable");
        assert!(masked.starts_with("postgresql://postgres:••••••@127.0.0.1:5432/postgres"));
        assert!(!masked.contains("hunter2"));
        // MongoDB-style passwords with an encoded `@` still resolve on the last `@`.
        let tricky = mask_uri("redis://default:p%40ss@127.0.0.1:6379");
        assert!(!tricky.contains("p%40ss"));
        // No credentials: unchanged.
        assert_eq!(
            mask_uri("postgresql://127.0.0.1:5432/db"),
            "postgresql://127.0.0.1:5432/db"
        );
    }

    #[test]
    fn dotenv_masking_replaces_only_the_password_line() {
        let block = "DB_CONNECTION=postgresql\nDB_PASSWORD=hunter2\nDB_PORT=5432";
        let masked = mask_dotenv(block);
        assert!(masked.contains("DB_PASSWORD=••••••"));
        assert!(!masked.contains("hunter2"));
        assert!(masked.contains("DB_PORT=5432"));
        // An instance created without a password has no value to hide.
        assert_eq!(
            mask_dotenv("DB_USERNAME=postgres\nDB_PASSWORD="),
            "DB_USERNAME=postgres\nDB_PASSWORD="
        );
        // The Valkey block uses a different key and must be masked too.
        let redis = mask_dotenv("REDIS_HOST=127.0.0.1\nREDIS_PASSWORD=hunter2");
        assert!(redis.contains("REDIS_PASSWORD=••••••"));
        assert!(!redis.contains("hunter2"));
    }

    #[test]
    fn env_lines_split_key_from_value() {
        let rows = env_lines("DB_HOST=127.0.0.1\n# comment\nDB_PASSWORD=secret");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].key.as_deref(), Some("DB_HOST"));
        assert_eq!(rows[0].value, "127.0.0.1");
        assert!(rows[1].key.is_none());
        assert_eq!(rows[2].value, "secret");
    }

    #[test]
    fn catalog_banner_and_label_follow_status() {
        let catalog = CatalogView {
            status: CatalogStatus::Stale,
            banner: Some(strings::CATALOG_STALE.to_owned()),
            engines: vec![],
        };
        assert_eq!(catalog_banner(&catalog), Some(strings::CATALOG_STALE));
        assert_eq!(catalog_status_label(&catalog), strings::CATALOG_STALE_LABEL);
        assert_eq!(
            catalog_freshness_word(CatalogStatus::Stale),
            strings::CATALOG_STALE_LABEL
        );
        let offline = CatalogView {
            status: CatalogStatus::Offline,
            banner: None,
            engines: vec![],
        };
        assert_eq!(
            catalog_status_label(&offline),
            strings::CATALOG_OFFLINE_LABEL
        );
        assert_eq!(
            catalog_freshness_word(CatalogStatus::Fresh),
            strings::CATALOG_FRESH_LABEL
        );
        assert!(catalog_banner(&offline).is_none());
    }

    #[test]
    fn every_diagnosis_maps_to_readable_text_not_a_slug() {
        use dblocal_core::model::StartFailure as D;
        for (diagnosis, expected) in [
            (D::PortInUse, strings::DIAGNOSIS_PORT_IN_USE),
            (
                D::DataDirVersionMismatch,
                strings::DIAGNOSIS_VERSION_MISMATCH,
            ),
            (D::PermissionDenied, strings::DIAGNOSIS_PERMISSION_DENIED),
            (D::BinaryMissing, strings::DIAGNOSIS_BINARY_MISSING),
            (D::Unknown, strings::DIAGNOSIS_UNKNOWN),
        ] {
            let label = diagnosis_label(diagnosis);
            assert_eq!(label, expected);
            // Never the machine slug the review caught.
            assert_ne!(label, diagnosis.as_str(), "{diagnosis:?} leaked its slug");
            assert!(!label.contains('_'));
        }
    }

    #[test]
    fn only_a_port_conflict_offers_the_one_click_fix() {
        let failed = view("shop", EngineKey::Postgresql, InstanceStatus::Failed, 5434).with_failure(
            FailureInfo {
                code: "DBL-1005".to_owned(),
                message: strings::ERR_PORT_IN_USE.to_owned(),
                diagnosis: StartFailure::PortInUse,
                log_tail: vec!["bind failed".to_owned()],
            },
        );
        assert!(offers_port_fix(&failed));
        assert_eq!(suggested_fix_port(&failed, Some(5435)), 5435);
        assert_eq!(suggested_fix_port(&failed, None), 5435);
        // A suggestion equal to the current port is useless; fall through.
        assert_eq!(suggested_fix_port(&failed, Some(5434)), 5435);

        let other = view("boom", EngineKey::Valkey, InstanceStatus::Failed, 6379).with_failure(
            FailureInfo {
                code: "DBL-4001".to_owned(),
                message: strings::ERR_ENGINE.to_owned(),
                diagnosis: StartFailure::Unknown,
                log_tail: vec![],
            },
        );
        assert!(!offers_port_fix(&other));

        let running = view("ok", EngineKey::Postgresql, InstanceStatus::Running, 5432);
        assert!(!offers_port_fix(&running));
    }

    #[test]
    fn runtime_rows_name_users_or_say_unused() {
        let runtimes = vec![
            runtime(EngineKey::Postgresql, "17.4"),
            runtime(EngineKey::Postgresql, "16.6"),
        ];
        let instances = vec![
            view_version("pg-app", EngineKey::Postgresql, "17.4", InstanceStatus::Running, 5432),
            view_version("pg-legacy", EngineKey::Postgresql, "16.6", InstanceStatus::Stopped, 5433),
        ];
        let rows = runtime_rows(&runtimes, &instances);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].title, "PostgreSQL 16.6");
        assert_eq!(rows[0].users, 1);
        assert!(!rows[0].unused);
        assert_eq!(rows[0].usage, format!("1 {}", strings::UI_RUNTIME_INSTANCE_COUNT));
        assert_eq!(rows[1].title, "PostgreSQL 17.4");
        assert_eq!(rows[1].users, 1);

        let unused = runtime_row(&runtime(EngineKey::Valkey, "7.2.8"), &instances);
        assert!(unused.unused);
        assert_eq!(unused.users, 0);
        assert_eq!(unused.usage, strings::UI_RUNTIME_UNUSED);
        assert_eq!(runtime_engine(&runtimes[0]), EngineKey::Postgresql);
    }

    #[test]
    fn palette_lists_start_stop_open_and_copy() {
        let running = view("run", EngineKey::Postgresql, InstanceStatus::Running, 5432);
        let stopped = view("stop", EngineKey::Mysql, InstanceStatus::Stopped, 3306);
        let items = palette_items(&[running, stopped]);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&strings::UI_PALETTE_NEW));
        assert!(
            labels
                .iter()
                .any(|l| l.starts_with(strings::UI_PALETTE_START) && l.contains("stop"))
        );
        assert!(
            labels
                .iter()
                .any(|l| l.starts_with(strings::UI_PALETTE_STOP) && l.contains("run"))
        );
        assert!(
            labels
                .iter()
                .any(|l| l.starts_with(strings::UI_PALETTE_COPY_URI))
        );
        assert!(matches!(items[0].action, PaletteAction::NewInstance));
    }

    #[test]
    fn palette_filter_is_case_insensitive_substring() {
        let items = palette_items(&[view(
            "shop",
            EngineKey::Mysql,
            InstanceStatus::Stopped,
            3306,
        )]);
        assert_eq!(filter_palette(&items, "").len(), items.len());
        let hits = filter_palette(&items, "SHOP");
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|i| i.label.contains("shop")));
        assert!(filter_palette(&items, "zzz").is_empty());
    }

    #[test]
    fn form_validation_rejects_short_names_and_low_ports() {
        let bad_name = validate_form("a", "5432", &[]);
        assert!(bad_name.name_error.is_some());
        assert!(!bad_name.is_valid());

        let low_port = validate_form("app-dev", "80", &[]);
        assert!(low_port.port_error.is_some());

        let ok = validate_form("app-dev", "5432", &[]);
        assert!(ok.is_valid());
        assert!(ok.name_error.is_none() && ok.port_error.is_none());
    }

    #[test]
    fn form_validation_reports_the_port_owner() {
        let instances = vec![view(
            "taken",
            EngineKey::Postgresql,
            InstanceStatus::Stopped,
            5432,
        )];
        let result = validate_form("fresh", "5432", &instances);
        assert!(!result.is_valid());
        assert_eq!(
            result.conflict.as_deref(),
            Some(dblocal_core::strings::validate_port_taken(5432, "taken").as_str())
        );
    }

    #[test]
    fn form_validation_allows_an_empty_port_for_auto_allocation() {
        let result = validate_form("app-dev", "", &[]);
        assert!(result.is_valid());
    }

    #[test]
    fn engine_unavailability_and_auth_expose_their_state() {
        let entry = EngineEntry {
            engine: EngineKey::Mariadb,
            available: false,
            unavailable_reason: Some(strings::CATALOG_ENGINE_UNAVAILABLE_PLATFORM.to_owned()),
            versions: vec![],
        };
        assert_eq!(
            engine_unavailable_reason(&entry),
            Some(strings::CATALOG_ENGINE_UNAVAILABLE_PLATFORM)
        );
        assert_eq!(
            version_unavailable_reason(&entry),
            Some(strings::CATALOG_ENGINE_UNAVAILABLE_PLATFORM)
        );
        assert!(auth_needs_password(AuthMode::Password));
        assert!(!auth_needs_password(AuthMode::None));
    }

    #[test]
    fn an_available_engine_has_no_version_blocker() {
        let entry = EngineEntry {
            engine: EngineKey::Postgresql,
            available: true,
            unavailable_reason: None,
            versions: vec![],
        };
        assert!(version_unavailable_reason(&entry).is_none());
        assert!(engine_unavailable_reason(&entry).is_none());
        // Unavailable without a reason still names the platform (F-CAT-02 AC2).
        let bare = EngineEntry {
            engine: EngineKey::Valkey,
            available: false,
            unavailable_reason: None,
            versions: vec![],
        };
        assert_eq!(
            version_unavailable_reason(&bare),
            Some(strings::CATALOG_ENGINE_UNAVAILABLE_PLATFORM)
        );
    }

    #[test]
    fn create_defaults_follow_the_prototype_prefill() {
        assert_eq!(suggested_name(EngineKey::Postgresql, "17.4"), "postgres-17");
        assert_eq!(suggested_name(EngineKey::Valkey, "8.1.0"), "valkey-8");
        assert_eq!(suggested_name(EngineKey::Mysql, "8.4.4"), "mysql-8");
        assert_eq!(suggested_name(EngineKey::Mariadb, "11.4.2"), "mariadb-11");
        assert!(create_needs_download(false));
        assert!(!create_needs_download(true));
    }

    #[test]
    fn next_free_port_skips_allocated_ports() {
        let instances = vec![
            view("aa-app", EngineKey::Postgresql, InstanceStatus::Running, 5432),
            view("bb-app", EngineKey::Postgresql, InstanceStatus::Running, 5433),
        ];
        assert_eq!(next_free_port(&instances, EngineKey::Postgresql), 5434);
        assert_eq!(next_free_port(&[], EngineKey::Valkey), 6379);
    }

    #[test]
    fn default_version_prefers_the_newest_supported_entry() {
        use dblocal_core::service::VersionEntry;
        let entry = EngineEntry {
            engine: EngineKey::Postgresql,
            available: true,
            unavailable_reason: None,
            versions: vec![
                VersionEntry {
                    version: "17.4".to_owned(),
                    eol: false,
                    installed: true,
                    size_bytes: 1,
                },
                VersionEntry {
                    version: "15.10".to_owned(),
                    eol: true,
                    installed: false,
                    size_bytes: 1,
                },
            ],
        };
        assert_eq!(default_version(&entry).map(|v| v.version.as_str()), Some("17.4"));
        assert_eq!(default_version_index(&entry), 0);

        let eol_only = EngineEntry {
            engine: EngineKey::Mysql,
            available: true,
            unavailable_reason: None,
            versions: vec![VersionEntry {
                version: "8.0.40".to_owned(),
                eol: true,
                installed: false,
                size_bytes: 1,
            }],
        };
        assert_eq!(
            default_version(&eol_only).map(|v| v.version.as_str()),
            Some("8.0.40")
        );
        assert_eq!(default_version_index(&eol_only), 0);
    }

    #[test]
    fn platform_label_joins_os_and_arch() {
        assert_eq!(platform_label("linux", "x86_64"), "linux x86_64");
    }
}
