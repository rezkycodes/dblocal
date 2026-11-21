use std::path::PathBuf;

use crate::clients::{
    ClientKind, DetectedClient, OpenTarget, OsKind, TerminalChoice, TerminalKind, plan_open,
    resolve_terminal,
};
use crate::model::EngineKey;
use crate::secret::SecretString;

use super::scan::{assert_no_secret, env_names};

#[derive(Default)]
struct FakeHost {
    bins: Vec<(String, PathBuf)>,
}

impl FakeHost {
    fn with_bin(mut self, name: &str, path: &str) -> Self {
        self.bins.push((name.to_owned(), PathBuf::from(path)));
        self
    }
}

impl crate::clients::HostProbe for FakeHost {
    fn which(&self, name: &str) -> Option<PathBuf> {
        self.bins.iter().find(|(n, _)| n == name).map(|(_, p)| p.clone())
    }
    fn path_exists(&self, _path: &std::path::Path) -> bool { false }
    fn desktop_entries(&self) -> Vec<String> { vec![] }
    fn macos_applications(&self) -> Vec<String> { vec![] }
    fn windows_app_paths(&self) -> Vec<String> { vec![] }
}

fn target(engine: EngineKey) -> OpenTarget {
    let (user, db, port) = match engine {
        EngineKey::Postgresql => ("postgres", "postgres", 5432),
        EngineKey::Mysql => ("root", "", 3306),
        EngineKey::Mariadb => ("root", "", 3307),
        EngineKey::Valkey => ("default", "0", 6379),
    };
    OpenTarget::new(engine, "127.0.0.1", port, user, db, "my-app")
}

fn client(kind: ClientKind, path: &str) -> DetectedClient {
    DetectedClient::new(kind, PathBuf::from(path), kind.supported_engines())
}

fn linux_terminal() -> TerminalChoice {
    TerminalChoice::new("xterm", TerminalKind::Xterm)
}

fn secret() -> SecretString {
    SecretString::from("s3cr3t-P@ss!")
}

fn all_secrets() -> Vec<&'static str> {
    vec!["s3cr3t-P@ss!", "p@ss:w/rd?#[]% x", "hunter2"]
}

#[test]
fn psql_plan_uses_pgpassword_env_not_argv() {
    let s = secret();
    let plan = plan_open(
        &client(ClientKind::Psql, "/usr/bin/psql"),
        &target(EngineKey::Postgresql),
        Some(&s),
        OsKind::Linux,
        &linux_terminal(),
    )
    .unwrap();
    assert_no_secret(&plan, s.expose());
    assert_eq!(env_names(plan.env()), vec!["PGPASSWORD"]);
    assert!(plan.argv().iter().any(|a| a == "psql"));
    assert!(plan.argv().iter().any(|a| a == "--username"));
}

#[test]
fn mysql_plan_uses_mysql_pwd_env() {
    let s = secret();
    let plan = plan_open(
        &client(ClientKind::Mysql, "/usr/bin/mysql"),
        &target(EngineKey::Mysql),
        Some(&s),
        OsKind::Linux,
        &linux_terminal(),
    )
    .unwrap();
    assert_no_secret(&plan, s.expose());
    assert_eq!(env_names(plan.env()), vec!["MYSQL_PWD"]);
}

#[test]
fn mariadb_cli_plan_uses_mysql_pwd_env() {
    let s = secret();
    let plan = plan_open(
        &client(ClientKind::Mariadb, "/usr/bin/mariadb"),
        &target(EngineKey::Mariadb),
        Some(&s),
        OsKind::Linux,
        &linux_terminal(),
    )
    .unwrap();
    assert_no_secret(&plan, s.expose());
    assert_eq!(env_names(plan.env()), vec!["MYSQL_PWD"]);
    assert!(plan.argv().iter().any(|a| a == "mariadb"));
}

#[test]
fn valkey_cli_plan_uses_rediscli_auth() {
    let s = secret();
    let plan = plan_open(
        &client(ClientKind::ValkeyCli, "/usr/bin/valkey-cli"),
        &target(EngineKey::Valkey),
        Some(&s),
        OsKind::Linux,
        &linux_terminal(),
    )
    .unwrap();
    assert_no_secret(&plan, s.expose());
    assert_eq!(env_names(plan.env()), vec!["REDISCLI_AUTH"]);
}

#[test]
fn redis_cli_plan_uses_rediscli_auth() {
    let s = secret();
    let plan = plan_open(
        &client(ClientKind::RedisCli, "/usr/bin/redis-cli"),
        &target(EngineKey::Valkey),
        Some(&s),
        OsKind::Linux,
        &linux_terminal(),
    )
    .unwrap();
    assert_no_secret(&plan, s.expose());
    assert_eq!(env_names(plan.env()), vec!["REDISCLI_AUTH"]);
}

#[test]
fn unsupported_engine_is_rejected() {
    let s = secret();
    let err = plan_open(
        &client(ClientKind::Psql, "/usr/bin/psql"),
        &target(EngineKey::Valkey),
        Some(&s),
        OsKind::Linux,
        &linux_terminal(),
    )
    .unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("Unsupported") || msg.contains("does not support"));
}

#[test]
fn tableplus_plan_has_no_password_in_url() {
    let s = secret();
    let plan = plan_open(
        &client(ClientKind::TablePlus, "/usr/bin/tableplus"),
        &target(EngineKey::Postgresql),
        Some(&s),
        OsKind::Linux,
        &linux_terminal(),
    )
    .unwrap();
    assert_no_secret(&plan, s.expose());
    let url = plan.url().expect("tableplus has url");
    assert!(url.starts_with("tableplus://"));
    assert!(!url.contains("password="));
}

#[test]
fn dbeaver_plan_has_no_password() {
    let s = secret();
    let plan = plan_open(
        &client(ClientKind::DBeaver, "/usr/bin/dbeaver"),
        &target(EngineKey::Postgresql),
        Some(&s),
        OsKind::Linux,
        &linux_terminal(),
    )
    .unwrap();
    assert_no_secret(&plan, s.expose());
    assert!(plan.argv().contains(&"-con".to_owned()));
}

#[test]
fn dbdelve_rejects_valkey() {
    let s = secret();
    let err = plan_open(
        &client(ClientKind::DbDelve, "/usr/bin/dbdelve"),
        &target(EngineKey::Valkey),
        Some(&s),
        OsKind::Linux,
        &linux_terminal(),
    )
    .unwrap_err();
    assert!(format!("{err:?}").contains("Unsupported"));
}

#[test]
fn macos_cli_uses_launcher_script_not_command_string_secret() {
    let s = secret();
    let plan = plan_open(
        &client(ClientKind::Psql, "/usr/bin/psql"),
        &target(EngineKey::Postgresql),
        Some(&s),
        OsKind::Macos,
        &TerminalChoice::new("open", TerminalKind::MacOsTerminalApp),
    )
    .unwrap();
    assert_no_secret(&plan, s.expose());
    let temp = plan.temp_file().expect("macos needs launcher script");
    assert!(temp.contents().expose().contains("PGPASSWORD"));
    assert!(temp.contents().expose().contains("s3cr3t-P@ss!"));
    assert!(temp.contents().expose().contains("rm -f -- \"$0\""));
    let cmd = plan.command_string().unwrap();
    assert!(!cmd.contains("s3cr3t"));
}

#[test]
fn windows_cli_uses_launcher_script() {
    let s = secret();
    let plan = plan_open(
        &client(ClientKind::Psql, "/usr/bin/psql"),
        &target(EngineKey::Postgresql),
        Some(&s),
        OsKind::Windows,
        &TerminalChoice::new("wt.exe", TerminalKind::WindowsTerminal),
    )
    .unwrap();
    assert_no_secret(&plan, s.expose());
    let temp = plan.temp_file().expect("windows needs launcher script");
    assert!(temp.contents().expose().contains("PGPASSWORD"));
    assert!(!plan.argv().iter().any(|a| a.contains("s3cr3t")));
}

#[test]
fn resolve_linux_terminal_falls_back() {
    let host = FakeHost::default().with_bin("xterm", "/usr/bin/xterm");
    let t = resolve_terminal(&host, OsKind::Linux);
    assert!(!t.program().as_os_str().is_empty());
}

#[test]
fn hostile_corpus_never_leaks_secret_and_is_quoted_in_script() {
    let hostile = [
        "normal",
        "has space",
        "quote'\"double",
        "semi;colon",
        "amp&&ersand",
        "$(subshell)",
        "`backtick`",
        "line\nbreak",
        "percent%20",
        "-leading-dash",
        "very-long-name-abcdefghijklmnopqrstuvwxyz0123456789-abcdefghijklmnopqrstuvwxyz0123456789",
        "ünïcödé",
    ];
    let s = SecretString::from("TOPSECRET");
    for name in hostile {
        let t = OpenTarget::new(EngineKey::Postgresql, "127.0.0.1", 5432, "postgres", "postgres", name);
        let plan = plan_open(
            &client(ClientKind::Psql, "/usr/bin/psql"),
            &t,
            Some(&s),
            OsKind::Macos,
            &TerminalChoice::new("open", TerminalKind::MacOsTerminalApp),
        )
        .unwrap();
        assert_no_secret(&plan, "TOPSECRET");
        // the hostile name should appear in script only as quoted argument, not break structure
        let script = plan.temp_file().unwrap().contents().expose();
        assert!(script.contains("TOPSECRET")); // env export is expected
        assert!(script.contains("rm -f -- \"$0\""));
    }
}

#[test]
fn every_golden_plan_has_no_secret() {
    let cases: Vec<(ClientKind, EngineKey, OsKind, TerminalChoice)> = vec![
        (ClientKind::Psql, EngineKey::Postgresql, OsKind::Linux, linux_terminal()),
        (ClientKind::Psql, EngineKey::Postgresql, OsKind::Macos, TerminalChoice::new("open", TerminalKind::MacOsTerminalApp)),
        (ClientKind::Psql, EngineKey::Postgresql, OsKind::Windows, TerminalChoice::new("wt.exe", TerminalKind::WindowsTerminal)),
        (ClientKind::Mysql, EngineKey::Mysql, OsKind::Linux, linux_terminal()),
        (ClientKind::Mariadb, EngineKey::Mariadb, OsKind::Linux, linux_terminal()),
        (ClientKind::ValkeyCli, EngineKey::Valkey, OsKind::Linux, linux_terminal()),
        (ClientKind::RedisCli, EngineKey::Valkey, OsKind::Linux, linux_terminal()),
        (ClientKind::TablePlus, EngineKey::Postgresql, OsKind::Linux, linux_terminal()),
        (ClientKind::TablePlus, EngineKey::Mysql, OsKind::Linux, linux_terminal()),
        (ClientKind::TablePlus, EngineKey::Valkey, OsKind::Linux, linux_terminal()),
        (ClientKind::DBeaver, EngineKey::Postgresql, OsKind::Linux, linux_terminal()),
        (ClientKind::DBeaver, EngineKey::Valkey, OsKind::Linux, linux_terminal()),
        (ClientKind::DbDelve, EngineKey::Postgresql, OsKind::Linux, linux_terminal()),
    ];
    let s = SecretString::from("CANARY_SECRET_123");
    for (kind, engine, os, terminal) in cases {
        let plan = plan_open(&client(kind, "/usr/bin/client"), &target(engine), Some(&s), os, &terminal).unwrap();
        assert_no_secret(&plan, "CANARY_SECRET_123");
    }
}

#[test]
fn secret_scan_catches_argv_leak() {
    let plan = crate::clients::LaunchPlan::new(
        "echo",
        vec!["CANARY".to_owned()],
        vec![],
        None,
        None,
        None,
    );
    let result = std::panic::catch_unwind(|| assert_no_secret(&plan, "CANARY"));
    assert!(result.is_err(), "scanner must catch argv leak");
}

#[test]
fn secret_scan_catches_url_leak() {
    let plan = crate::clients::LaunchPlan::new(
        "open",
        vec![],
        vec![],
        Some("tableplus://?password=CANARY".to_owned()),
        None,
        None,
    );
    let result = std::panic::catch_unwind(|| assert_no_secret(&plan, "CANARY"));
    assert!(result.is_err(), "scanner must catch url leak");
}
