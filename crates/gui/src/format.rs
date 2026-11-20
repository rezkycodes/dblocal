//! Pure view-model helpers. No GPUI types, no I/O — every function here is
//! unit-tested and drives the render code.

use dblocal_core::model::{AuthMode, EngineKey, InstanceStatus};
use dblocal_core::service::{CatalogView, EngineEntry, InstanceView};
use dblocal_core::strings;

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

/// Whether the engine is running (or transitioning to it).
pub fn is_running(status: InstanceStatus) -> bool {
    matches!(
        status,
        InstanceStatus::Running | InstanceStatus::Starting | InstanceStatus::Stopping
    )
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

/// Counts instances that currently hold a process.
pub fn running_count(instances: &[InstanceView]) -> usize {
    instances
        .iter()
        .filter(|view| view.status == InstanceStatus::Running)
        .count()
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

/// Builds the compact "`version` · `port`" meta line of an instance row.
pub fn instance_meta(view: &InstanceView) -> String {
    format!("{}  ·  {}", view.version, view.port)
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
pub fn mask_dotenv(block: &str) -> String {
    block
        .lines()
        .map(|line| {
            if let Some(rest) = line.strip_prefix("DB_PASSWORD=") {
                if rest.is_empty() {
                    line.to_owned()
                } else {
                    "DB_PASSWORD=••••••".to_owned()
                }
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Active banner text from the catalog read model (F-CAT-01).
pub fn catalog_banner(catalog: &CatalogView) -> Option<&str> {
    catalog.banner.as_deref()
}

/// Short label for the catalog freshness indicator.
pub fn catalog_status_label(catalog: &CatalogView) -> &'static str {
    match catalog.status {
        dblocal_core::model::CatalogStatus::Fresh => "Fresh",
        dblocal_core::model::CatalogStatus::Stale => "Stale",
        dblocal_core::model::CatalogStatus::Offline => "Offline",
        _ => "Fresh",
    }
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

/// Whether an auth mode requires a password.
pub fn auth_needs_password(auth: AuthMode) -> bool {
    auth.requires_password()
}

#[cfg(test)]
mod tests {
    use dblocal_core::model::{
        AuthMode, CatalogStatus, CredentialBackend, EngineKey, InstanceId, InstanceName,
        InstanceSpec, InstanceStatus, Port, StartFailure,
    };
    use dblocal_core::service::FailureInfo;

    use super::*;

    fn view(name: &str, engine: EngineKey, status: InstanceStatus, port: u16) -> InstanceView {
        let spec = InstanceSpec::new(
            InstanceName::new(name).expect("valid name"),
            engine,
            "17.4",
            Some(Port::new(port).expect("valid port")),
            AuthMode::Password,
        )
        .expect("spec");
        let now = std::time::SystemTime::UNIX_EPOCH;
        let instance = dblocal_core::model::Instance::new(&spec, InstanceId::new(), status, now);
        InstanceView::from_instance(&instance)
    }

    #[test]
    fn every_status_has_a_nonempty_label() {
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
    fn running_count_only_counts_running() {
        let instances = vec![
            view("aa", EngineKey::Postgresql, InstanceStatus::Running, 5432),
            view("bb", EngineKey::Postgresql, InstanceStatus::Stopped, 5433),
            view("cc", EngineKey::Valkey, InstanceStatus::Starting, 6379),
        ];
        assert_eq!(running_count(&instances), 1);
        assert!(is_running(InstanceStatus::Starting));
        assert!(!is_running(InstanceStatus::Failed));
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
    fn speed_and_eta_formatting() {
        assert_eq!(format_speed(13 * 1024 * 1024), "13.0 MB/s");
        assert_eq!(format_eta(0), "0s");
        assert_eq!(format_eta(45), "45s");
        assert_eq!(format_eta(65), "1m 5s");
        assert_eq!(format_eta(120), "2m");
        assert_eq!(format_eta(7_380), "2h 3m");
    }

    #[test]
    fn meta_line_combines_version_and_port() {
        let v = view("app", EngineKey::Postgresql, InstanceStatus::Stopped, 5432);
        assert_eq!(instance_meta(&v), "17.4  ·  5432");
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
    }

    #[test]
    fn catalog_banner_and_label_follow_status() {
        let catalog = CatalogView {
            status: CatalogStatus::Stale,
            banner: Some(strings::CATALOG_STALE.to_owned()),
            engines: vec![],
        };
        assert_eq!(catalog_banner(&catalog), Some(strings::CATALOG_STALE));
        assert_eq!(catalog_status_label(&catalog), "Stale");
        let offline = CatalogView {
            status: CatalogStatus::Offline,
            banner: None,
            engines: vec![],
        };
        assert_eq!(catalog_status_label(&offline), "Offline");
        assert!(catalog_banner(&offline).is_none());
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
        assert!(auth_needs_password(AuthMode::Password));
        assert!(!auth_needs_password(AuthMode::None));
    }

    #[test]
    fn failure_view_fields_are_reachable() {
        let v = view("boom", EngineKey::Valkey, InstanceStatus::Failed, 6379).with_failure(
            FailureInfo {
                code: "DBL-4001".to_owned(),
                message: strings::ERR_ENGINE.to_owned(),
                diagnosis: StartFailure::PortInUse,
                log_tail: vec!["[error] bind failed".to_owned()],
            },
        );
        assert_eq!(v.failure.as_ref().expect("failure").code, "DBL-4001");
        assert_eq!(status_label(v.status), strings::STATUS_FAILED);
        let _ = CredentialBackend::File; // keeps the import used in this module set
    }
}
