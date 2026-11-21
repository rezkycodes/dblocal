use std::path::{Path, PathBuf};

use crate::clients::{ClientKind, HostProbe, OpenMenu, detect_clients};
use crate::model::EngineKey;

#[derive(Default)]
struct FakeHost {
    bins: Vec<(String, PathBuf)>,
    desktop: Vec<String>,
    mac_apps: Vec<String>,
    win_apps: Vec<String>,
    paths: Vec<PathBuf>,
}

impl FakeHost {
    fn with_bin(mut self, name: &str, path: &str) -> Self {
        self.bins.push((name.to_owned(), PathBuf::from(path)));
        self
    }
    fn with_desktop(mut self, entry: &str) -> Self {
        self.desktop.push(entry.to_owned());
        self
    }
}

impl HostProbe for FakeHost {
    fn which(&self, name: &str) -> Option<PathBuf> {
        self.bins
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, p)| p.clone())
    }
    fn path_exists(&self, path: &Path) -> bool {
        self.paths.iter().any(|p| p == path)
    }
    fn desktop_entries(&self) -> Vec<String> {
        self.desktop.clone()
    }
    fn macos_applications(&self) -> Vec<String> {
        self.mac_apps.clone()
    }
    fn windows_app_paths(&self) -> Vec<String> {
        self.win_apps.clone()
    }
}

#[test]
fn empty_host_returns_no_clients() {
    let host = FakeHost::default();
    let clients = detect_clients(&host);
    assert!(clients.is_empty());
    let menu = OpenMenu::from_clients(clients);
    assert!(menu.copy_uri_fallback());
    assert!(menu.clients().is_empty());
}

#[test]
fn detects_psql_by_path() {
    let host = FakeHost::default().with_bin("psql", "/usr/bin/psql");
    let clients = detect_clients(&host);
    assert_eq!(clients.len(), 1);
    assert_eq!(clients[0].kind(), ClientKind::Psql);
    assert_eq!(clients[0].path(), &PathBuf::from("/usr/bin/psql"));
    assert_eq!(clients[0].engines(), &[EngineKey::Postgresql]);
}

#[test]
fn detects_all_cli_clients() {
    let host = FakeHost::default()
        .with_bin("psql", "/bin/psql")
        .with_bin("mysql", "/bin/mysql")
        .with_bin("mariadb", "/bin/mariadb")
        .with_bin("valkey-cli", "/bin/valkey-cli")
        .with_bin("redis-cli", "/bin/redis-cli");
    let clients = detect_clients(&host);
    let kinds: Vec<_> = clients.iter().map(|c| c.kind()).collect();
    assert_eq!(
        kinds,
        vec![
            ClientKind::Psql,
            ClientKind::Mysql,
            ClientKind::Mariadb,
            ClientKind::ValkeyCli,
            ClientKind::RedisCli,
        ]
    );
}

#[test]
fn detects_gui_client_from_desktop_entry() {
    let host = FakeHost::default()
        .with_bin("tableplus", "/usr/bin/tableplus")
        .with_desktop("com.tableplus.TablePlus.desktop");
    let clients = detect_clients(&host);
    assert!(clients.iter().any(|c| c.kind() == ClientKind::TablePlus));
}

#[test]
fn detects_dbeaver() {
    let host = FakeHost::default().with_bin("dbeaver", "/usr/bin/dbeaver");
    let clients = detect_clients(&host);
    let dbeaver = clients.iter().find(|c| c.kind() == ClientKind::DBeaver).unwrap();
    assert!(dbeaver.supports(EngineKey::Postgresql));
    assert!(dbeaver.supports(EngineKey::Mysql));
    assert!(dbeaver.supports(EngineKey::Mariadb));
    assert!(dbeaver.supports(EngineKey::Valkey));
}

#[test]
fn menu_without_clients_has_fallback() {
    let menu = OpenMenu::from_clients(vec![]);
    assert!(menu.copy_uri_fallback());
}

#[test]
fn menu_with_clients_has_no_fallback() {
    let host = FakeHost::default().with_bin("psql", "/bin/psql");
    let menu = OpenMenu::from_clients(detect_clients(&host));
    assert!(!menu.copy_uri_fallback());
    assert_eq!(menu.clients().len(), 1);
}
