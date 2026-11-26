//! Client detection (F-OPN-01).
//!
//! Detection is pure over a [`HostProbe`] so tests can inject a fake host. Only
//! clients that are actually found are returned, each with the proof (path).

use std::path::PathBuf;

use crate::model::EngineKey;

/// Proof that a client is installed on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedClient {
    kind: ClientKind,
    path: PathBuf,
    engines: Vec<EngineKey>,
}

impl DetectedClient {
    /// Creates a detected client record.
    pub fn new(kind: ClientKind, path: PathBuf, engines: Vec<EngineKey>) -> Self {
        Self {
            kind,
            path,
            engines,
        }
    }

    /// Which client this is.
    pub fn kind(&self) -> ClientKind {
        self.kind
    }

    /// Path or proof of discovery.
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Engines this client can open.
    pub fn engines(&self) -> &[EngineKey] {
        &self.engines
    }

    /// Whether this client can open the given engine.
    pub fn supports(&self, engine: EngineKey) -> bool {
        self.engines.contains(&engine)
    }
}

/// Known external client kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ClientKind {
    /// DBDelve desktop client.
    DbDelve,
    /// TablePlus desktop client.
    TablePlus,
    /// DBeaver desktop client.
    DBeaver,
    /// PostgreSQL CLI.
    Psql,
    /// MySQL CLI.
    Mysql,
    /// MariaDB CLI.
    Mariadb,
    /// Valkey CLI.
    ValkeyCli,
    /// Redis CLI.
    RedisCli,
}

impl ClientKind {
    /// Stable identifier for logs/docs/tests.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DbDelve => "dbdelve",
            Self::TablePlus => "tableplus",
            Self::DBeaver => "dbeaver",
            Self::Psql => "psql",
            Self::Mysql => "mysql",
            Self::Mariadb => "mariadb",
            Self::ValkeyCli => "valkey-cli",
            Self::RedisCli => "redis-cli",
        }
    }

    /// Human display name.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::DbDelve => "DBDelve",
            Self::TablePlus => "TablePlus",
            Self::DBeaver => "DBeaver",
            Self::Psql => "psql",
            Self::Mysql => "mysql",
            Self::Mariadb => "mariadb",
            Self::ValkeyCli => "valkey-cli",
            Self::RedisCli => "redis-cli",
        }
    }

    /// Whether this is a GUI client.
    pub const fn is_gui(self) -> bool {
        matches!(self, Self::DbDelve | Self::TablePlus | Self::DBeaver)
    }

    /// Engines supported by this client kind.
    pub fn supported_engines(self) -> Vec<EngineKey> {
        match self {
            Self::DbDelve => vec![EngineKey::Postgresql, EngineKey::Mysql, EngineKey::Mariadb],
            Self::TablePlus => vec![
                EngineKey::Postgresql,
                EngineKey::Mysql,
                EngineKey::Mariadb,
                EngineKey::Valkey,
            ],
            Self::DBeaver => vec![
                EngineKey::Postgresql,
                EngineKey::Mysql,
                EngineKey::Mariadb,
                EngineKey::Valkey,
            ],
            Self::Psql => vec![EngineKey::Postgresql],
            Self::Mysql | Self::Mariadb => {
                vec![EngineKey::Mysql, EngineKey::Mariadb]
            }
            Self::ValkeyCli | Self::RedisCli => vec![EngineKey::Valkey],
        }
    }
}

/// Abstraction over OS-specific client discovery.
///
/// Tests inject a fake implementation. Real implementations live outside this
/// core-only task; the trait is small enough for GUI/CLI to implement.
pub trait HostProbe: Send + Sync {
    /// Looks up an executable name on PATH.
    fn which(&self, name: &str) -> Option<PathBuf>;
    /// Checks whether a path exists.
    fn path_exists(&self, path: &std::path::Path) -> bool;
    /// Linux `.desktop` entry identifiers or names visible to the host.
    fn desktop_entries(&self) -> Vec<String>;
    /// macOS application bundle names under `/Applications`.
    fn macos_applications(&self) -> Vec<String>;
    /// Windows App Paths / installed app names.
    fn windows_app_paths(&self) -> Vec<String>;
}

/// The menu shown by "Open in..." (F-OPN-01 AC2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenMenu {
    clients: Vec<DetectedClient>,
    copy_uri_fallback: bool,
}

impl OpenMenu {
    /// Builds a menu from detected clients.
    pub fn from_clients(clients: Vec<DetectedClient>) -> Self {
        let copy_uri_fallback = clients.is_empty();
        Self {
            clients,
            copy_uri_fallback,
        }
    }

    /// Detected clients, in detection order.
    pub fn clients(&self) -> &[DetectedClient] {
        &self.clients
    }

    /// Whether the menu should show "Copy URI" instead of an empty list.
    pub fn copy_uri_fallback(&self) -> bool {
        self.copy_uri_fallback
    }
}

/// Detects installed clients on the host.
///
/// Only clients that are really found are returned, each with proof.
pub fn detect_clients(host: &dyn HostProbe) -> Vec<DetectedClient> {
    let mut out = Vec::new();
    detect_gui_clients(host, &mut out);
    detect_cli_clients(host, &mut out);
    out
}

fn detect_gui_clients(host: &dyn HostProbe, out: &mut Vec<DetectedClient>) {
    detect_dbdelve(host, out);
    detect_tableplus(host, out);
    detect_dbeaver(host, out);
}

fn detect_dbdelve(host: &dyn HostProbe, out: &mut Vec<DetectedClient>) {
    let kind = ClientKind::DbDelve;
    if let Some(path) = find_gui_client(host, &["dbdelve", "DBDelve"], &["dbdelve"]) {
        out.push(DetectedClient::new(kind, path, kind.supported_engines()));
    }
}

fn detect_tableplus(host: &dyn HostProbe, out: &mut Vec<DetectedClient>) {
    let kind = ClientKind::TablePlus;
    if let Some(path) = find_gui_client(host, &["tableplus", "TablePlus"], &["tableplus"]) {
        out.push(DetectedClient::new(kind, path, kind.supported_engines()));
    }
}

fn detect_dbeaver(host: &dyn HostProbe, out: &mut Vec<DetectedClient>) {
    let kind = ClientKind::DBeaver;
    let path = find_gui_client(
        host,
        &["dbeaver", "dbeaver-ce", "dbeaver-ee", "DBeaver"],
        &["dbeaver"],
    );
    if let Some(path) = path {
        out.push(DetectedClient::new(kind, path, kind.supported_engines()));
    }
}

fn find_gui_client(
    host: &dyn HostProbe,
    bin_names: &[&str],
    desktop_tokens: &[&str],
) -> Option<PathBuf> {
    for name in bin_names {
        if let Some(path) = host.which(name) {
            return Some(path);
        }
    }
    if has_desktop_token(host, desktop_tokens) {
        for name in bin_names {
            if let Some(path) = host.which(name) {
                return Some(path);
            }
        }
        return Some(PathBuf::from(bin_names[0]));
    }
    if has_macos_app(host, desktop_tokens) {
        return Some(PathBuf::from(format!("/Applications/{}.app", bin_names[0])));
    }
    if has_windows_app(host, desktop_tokens) {
        return Some(PathBuf::from(bin_names[0]));
    }
    None
}

fn has_desktop_token(host: &dyn HostProbe, tokens: &[&str]) -> bool {
    let entries = host.desktop_entries();
    entries.iter().any(|entry| {
        let lower = entry.to_ascii_lowercase();
        tokens.iter().any(|token| lower.contains(token))
    })
}

fn has_macos_app(host: &dyn HostProbe, tokens: &[&str]) -> bool {
    let apps = host.macos_applications();
    apps.iter().any(|app| {
        let lower = app.to_ascii_lowercase();
        tokens.iter().any(|token| lower.contains(token))
    })
}

fn has_windows_app(host: &dyn HostProbe, tokens: &[&str]) -> bool {
    let apps = host.windows_app_paths();
    apps.iter().any(|app| {
        let lower = app.to_ascii_lowercase();
        tokens.iter().any(|token| lower.contains(token))
    })
}

fn detect_cli_clients(host: &dyn HostProbe, out: &mut Vec<DetectedClient>) {
    for (kind, bin) in [
        (ClientKind::Psql, "psql"),
        (ClientKind::Mysql, "mysql"),
        (ClientKind::Mariadb, "mariadb"),
        (ClientKind::ValkeyCli, "valkey-cli"),
        (ClientKind::RedisCli, "redis-cli"),
    ] {
        if let Some(path) = host.which(bin) {
            out.push(DetectedClient::new(kind, path, kind.supported_engines()));
        }
    }
}
