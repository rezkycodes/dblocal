use std::path::PathBuf;

use crate::clients::{
    ClientKind, DetectedClient, OpenTarget, OsKind, TerminalChoice, TerminalKind, plan_open,
};
use crate::model::EngineKey;
use crate::secret::SecretString;

use super::scan::assert_no_secret;

fn client(kind: ClientKind) -> DetectedClient {
    DetectedClient::new(
        kind,
        PathBuf::from(format!("/usr/bin/{}", kind.as_str())),
        kind.supported_engines(),
    )
}
fn term(os: OsKind) -> TerminalChoice {
    match os {
        OsKind::Linux => TerminalChoice::new("xterm", TerminalKind::Xterm),
        OsKind::Macos => TerminalChoice::new("open", TerminalKind::MacOsTerminalApp),
        OsKind::Windows => TerminalChoice::new("wt.exe", TerminalKind::WindowsTerminal),
    }
}
fn target(engine: EngineKey) -> OpenTarget {
    match engine {
        EngineKey::Postgresql => {
            OpenTarget::new(engine, "127.0.0.1", 5432, "postgres", "postgres", "my-app").unwrap()
        }
        EngineKey::Mysql => {
            OpenTarget::new(engine, "127.0.0.1", 3306, "root", "", "my-app").unwrap()
        }
        EngineKey::Mariadb => {
            OpenTarget::new(engine, "127.0.0.1", 3307, "root", "", "my-app").unwrap()
        }
        EngineKey::Valkey | EngineKey::Redis => {
            OpenTarget::new(engine, "127.0.0.1", 6379, "default", "0", "my-app").unwrap()
        }
        EngineKey::Rustfs => {
            OpenTarget::new(engine, "127.0.0.1", 9000, "DBLKEY", "", "my-app").unwrap()
        }
    }
}

#[test]
fn golden_psql_linux() {
    let s = SecretString::from("PW");
    let plan = plan_open(
        &client(ClientKind::Psql),
        &target(EngineKey::Postgresql),
        Some(&s),
        OsKind::Linux,
        &term(OsKind::Linux),
    )
    .unwrap();
    assert_eq!(plan.program(), &PathBuf::from("xterm"));
    assert_eq!(plan.argv()[0], "-e");
    assert_eq!(plan.argv()[1], "sh");
    assert_eq!(plan.argv()[2], "{script}");
    let script = plan.temp_file().unwrap().contents().expose();
    assert!(script.contains("PGPASSWORD="));
    assert!(script.contains("/usr/bin/psql"));
    assert!(script.contains("postgresql://postgres@127.0.0.1:5432/postgres"));
    assert!(script.contains("rm -f -- \"$0\""));
    assert_no_secret(&plan, "PW");
}

#[test]
fn golden_mysql_linux() {
    let s = SecretString::from("PW");
    let plan = plan_open(
        &client(ClientKind::Mysql),
        &target(EngineKey::Mysql),
        Some(&s),
        OsKind::Linux,
        &term(OsKind::Linux),
    )
    .unwrap();
    assert_eq!(plan.argv()[0], "-e");
    assert_eq!(plan.argv()[1], "sh");
    let script = plan.temp_file().unwrap().contents().expose();
    assert!(script.contains("MYSQL_PWD="));
    assert!(script.contains("/usr/bin/mysql"));
    assert!(script.contains("--protocol=tcp"));
    assert_no_secret(&plan, "PW");
}

#[test]
fn golden_valkey_cli_linux() {
    let s = SecretString::from("PW");
    let plan = plan_open(
        &client(ClientKind::ValkeyCli),
        &target(EngineKey::Valkey),
        Some(&s),
        OsKind::Linux,
        &term(OsKind::Linux),
    )
    .unwrap();
    assert_eq!(plan.argv()[0], "-e");
    assert_eq!(plan.argv()[1], "sh");
    let script = plan.temp_file().unwrap().contents().expose();
    assert!(script.contains("REDISCLI_AUTH="));
    assert!(script.contains("/usr/bin/valkey-cli"));
    assert_no_secret(&plan, "PW");
}

#[test]
fn golden_tableplus_linux_url() {
    let s = SecretString::from("PW");
    let plan = plan_open(
        &client(ClientKind::TablePlus),
        &target(EngineKey::Postgresql),
        Some(&s),
        OsKind::Linux,
        &term(OsKind::Linux),
    )
    .unwrap();
    assert_eq!(plan.program(), &PathBuf::from("/usr/bin/tableplus"));
    assert!(plan.url().unwrap().starts_with("postgresql://"));
    assert_no_secret(&plan, "PW");
}

#[test]
fn golden_tableplus_windows() {
    let s = SecretString::from("PW");
    let plan = plan_open(
        &client(ClientKind::TablePlus),
        &target(EngineKey::Postgresql),
        Some(&s),
        OsKind::Windows,
        &term(OsKind::Windows),
    )
    .unwrap();
    assert_eq!(plan.program(), &PathBuf::from("/usr/bin/tableplus"));
    assert!(plan.url().unwrap().starts_with("postgresql://"));
    assert_no_secret(&plan, "PW");
}

#[test]
fn golden_tableplus_macos() {
    let s = SecretString::from("PW");
    let plan = plan_open(
        &client(ClientKind::TablePlus),
        &target(EngineKey::Postgresql),
        Some(&s),
        OsKind::Macos,
        &term(OsKind::Macos),
    )
    .unwrap();
    assert_eq!(plan.program(), &PathBuf::from("open"));
    assert_eq!(plan.argv()[0], "-a");
    assert_eq!(plan.argv()[1], "TablePlus");
    assert_no_secret(&plan, "PW");
}

#[test]
fn golden_psql_macos_uses_script() {
    let s = SecretString::from("PW");
    let plan = plan_open(
        &client(ClientKind::Psql),
        &target(EngineKey::Postgresql),
        Some(&s),
        OsKind::Macos,
        &term(OsKind::Macos),
    )
    .unwrap();
    assert_eq!(plan.program(), &PathBuf::from("osascript"));
    assert!(plan.temp_file().is_some());
    assert!(plan.command_string().unwrap().contains("{script}"));
    assert_no_secret(&plan, "PW");
}

#[test]
fn golden_psql_windows_uses_script() {
    let s = SecretString::from("PW");
    let plan = plan_open(
        &client(ClientKind::Psql),
        &target(EngineKey::Postgresql),
        Some(&s),
        OsKind::Windows,
        &term(OsKind::Windows),
    )
    .unwrap();
    assert_eq!(plan.program(), &PathBuf::from("wt.exe"));
    assert!(plan.temp_file().is_some());
    assert!(plan.argv().iter().any(|a| a.contains("{script}")));
    assert_no_secret(&plan, "PW");
}
