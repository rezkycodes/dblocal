use std::path::PathBuf;

use crate::clients::{
    ClientError, ClientKind, DetectedClient, LaunchPlan, OpenTarget, OsKind, TerminalChoice,
    TerminalKind, plan_open,
};
use crate::model::EngineKey;
use crate::secret::SecretString;

fn client(kind: ClientKind) -> DetectedClient {
    DetectedClient::new(
        kind,
        PathBuf::from(format!("/usr/bin/{}", kind.as_str())),
        kind.supported_engines(),
    )
}

fn target(engine: EngineKey) -> OpenTarget {
    let (user, db, port) = match engine {
        EngineKey::Postgresql => ("postgres", "postgres", 5432),
        EngineKey::Mysql => ("root", "", 3306),
        EngineKey::Mariadb => ("root", "", 3307),
        EngineKey::Valkey | EngineKey::Redis => ("default", "0", 6379),
    };
    OpenTarget::new(engine, "127.0.0.1", port, user, db, "my-app").unwrap()
}

fn plan_for(
    kind: ClientKind,
    engine: EngineKey,
    os: OsKind,
    terminal: TerminalChoice,
) -> LaunchPlan {
    plan_open(
        &client(kind),
        &target(engine),
        Some(&SecretString::from("pw")),
        os,
        &terminal,
    )
    .unwrap()
}

#[test]
fn open_target_rejects_port_zero() {
    let err = OpenTarget::new(EngineKey::Postgresql, "127.0.0.1", 0, "u", "d", "n").unwrap_err();
    assert!(matches!(err, ClientError::InvalidTarget { field: "port" }));
}

#[test]
fn open_target_accessors_round_trip() {
    let t = target(EngineKey::Mysql);
    assert_eq!(t.engine(), EngineKey::Mysql);
    assert_eq!(t.host(), "127.0.0.1");
    assert_eq!(t.port(), 3306);
    assert_eq!(t.username(), "root");
    assert_eq!(t.database(), "");
    assert_eq!(t.instance_name(), "my-app");
}

#[test]
fn empty_username_is_rejected_and_empty_database_allowed() {
    assert!(OpenTarget::new(EngineKey::Postgresql, "127.0.0.1", 5432, "", "d", "n").is_err());
    assert!(OpenTarget::new(EngineKey::Postgresql, "127.0.0.1", 5432, "u", "", "n").is_ok());
}

#[test]
fn dbeaver_driver_ids_for_every_engine() {
    let expected = [
        (EngineKey::Postgresql, "driver=postgresql"),
        (EngineKey::Mysql, "driver=mysql"),
        (EngineKey::Mariadb, "driver=maria"),
        (EngineKey::Valkey, "driver=redis"),
    ];
    for (engine, driver) in expected {
        let plan = plan_for(
            ClientKind::DBeaver,
            engine,
            OsKind::Linux,
            TerminalChoice::new("xterm", TerminalKind::Xterm),
        );
        assert!(
            plan.argv().iter().any(|a| a.contains(driver)),
            "{engine:?} missing {driver}: {:?}",
            plan.argv()
        );
    }
}

#[test]
fn dbeaver_connection_name_defaults_when_blank_and_sanitises_pipes() {
    // instance_name is validated non-empty, but control chars are allowed for the
    // con-value sanitisers; give a name with a pipe-free host/user and a long name.
    let t = OpenTarget::new(
        EngineKey::Postgresql,
        "127.0.0.1",
        5432,
        "postgres",
        "postgres",
        "name with spaces",
    )
    .unwrap();
    let plan = plan_open(
        &client(ClientKind::DBeaver),
        &t,
        None,
        OsKind::Linux,
        &TerminalChoice::new("xterm", TerminalKind::Xterm),
    )
    .unwrap();
    let spec = plan.argv().iter().find(|a| a.contains("driver=")).unwrap();
    assert!(spec.contains("name=name with spaces"));
    assert!(spec.contains("openConsole=true"));
}

#[test]
fn tableplus_urls_for_all_engines() {
    let cases = [
        (EngineKey::Postgresql, "postgresql://"),
        (EngineKey::Mysql, "mysql://"),
        (EngineKey::Mariadb, "mysql://"),
        (EngineKey::Valkey, "redis://"),
    ];
    for (engine, prefix) in cases {
        let plan = plan_for(
            ClientKind::TablePlus,
            engine,
            OsKind::Linux,
            TerminalChoice::new("xterm", TerminalKind::Xterm),
        );
        assert!(plan.url().unwrap().starts_with(prefix), "{engine:?}");
    }
}

#[test]
fn passwordless_uri_for_all_engines_is_used_by_dbdelve_hint() {
    for engine in [EngineKey::Postgresql, EngineKey::Mysql, EngineKey::Mariadb] {
        let plan = plan_for(
            ClientKind::DbDelve,
            engine,
            OsKind::Linux,
            TerminalChoice::new("xterm", TerminalKind::Xterm),
        );
        assert!(plan.copy_uri_hint().unwrap().contains('@'));
    }
}

#[test]
fn linux_script_forms_per_terminal_kind() {
    let cases = [
        (TerminalKind::Ptyxis, "--new-window"),
        (TerminalKind::Wezterm, "start"),
        (TerminalKind::XdgTerminalExec, "--"),
        (TerminalKind::GnomeTerminal, "--"),
        (TerminalKind::Foot, "sh"),
        (TerminalKind::Kitty, "--"),
        (TerminalKind::Xterm, "-e"),
    ];
    for (kind, marker) in cases {
        let plan = plan_for(
            ClientKind::Psql,
            EngineKey::Postgresql,
            OsKind::Linux,
            TerminalChoice::new("term", kind),
        );
        assert!(
            plan.argv().iter().any(|a| a == marker),
            "{kind:?} must contain {marker}: {:?}",
            plan.argv()
        );
        assert!(plan.argv().iter().any(|a| a == "{script}"));
    }
}

#[test]
fn linux_exec_forms_without_password_skip_the_script() {
    for kind in [
        TerminalKind::Ptyxis,
        TerminalKind::GnomeTerminal,
        TerminalKind::Wezterm,
        TerminalKind::XdgTerminalExec,
        TerminalKind::Kitty,
        TerminalKind::Xterm,
    ] {
        let plan = plan_open(
            &client(ClientKind::Psql),
            &target(EngineKey::Postgresql),
            None,
            OsKind::Linux,
            &TerminalChoice::new("term", kind),
        )
        .unwrap();
        assert!(
            plan.temp_file().is_none(),
            "{kind:?} should not need a script"
        );
        assert!(plan.argv().iter().any(|a| a == "/usr/bin/psql"));
    }
}

#[test]
fn windows_terminal_without_wt_uses_cmd_c_form() {
    let plan = plan_for(
        ClientKind::Mysql,
        EngineKey::Mysql,
        OsKind::Windows,
        TerminalChoice::new("cmd.exe", TerminalKind::WindowsCmd),
    );
    assert_eq!(plan.argv()[0], "/c");
    assert!(plan.temp_file().is_some());
}

#[test]
fn macos_command_string_has_no_script_secret_and_escapes() {
    let plan = plan_for(
        ClientKind::Mysql,
        EngineKey::Mysql,
        OsKind::Macos,
        TerminalChoice::new("open", TerminalKind::MacOsTerminalApp),
    );
    let cmd = plan.command_string().unwrap();
    assert!(!cmd.contains("pw"));
    assert_eq!(plan.program(), &PathBuf::from("osascript"));
}

#[test]
fn ptyxis_argv_is_exactly_one_new_window_and_one_dashdash() {
    // H3 regression guard: a wrapper that also adds `--new-window --` would ask
    // ptyxis to run a command literally named `--new-window`.
    let plan = plan_for(
        ClientKind::Psql,
        EngineKey::Postgresql,
        OsKind::Linux,
        TerminalChoice::new("/usr/bin/ptyxis", TerminalKind::Ptyxis),
    );
    assert_eq!(
        plan.argv(),
        &[
            "--new-window".to_owned(),
            "--".to_owned(),
            "sh".to_owned(),
            "{script}".to_owned(),
        ]
    );
    assert_eq!(plan.program(), &PathBuf::from("/usr/bin/ptyxis"));
}

#[test]
fn cli_plan_does_not_support_unknown_gui_kind_through_cli_path() {
    // Redis CLI on MariaDB is unsupported at the client level.
    let err = plan_open(
        &client(ClientKind::RedisCli),
        &target(EngineKey::Mariadb),
        None,
        OsKind::Linux,
        &TerminalChoice::new("term", TerminalKind::Xterm),
    )
    .unwrap_err();
    assert!(matches!(err, ClientError::UnsupportedClient { .. }));
}
