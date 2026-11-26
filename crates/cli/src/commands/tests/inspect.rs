//! `list`, `status`, `uri`, `logs`, the formatters and the exit-code table.

use super::*;
use dblocal_core::service::{OperationId, ServiceEvent};

#[test]
fn list_empty_table() {
    let svc = MockService::deterministic();
    let (code, out, err) = run_cmd(&svc, &Command::List { json: false }, false);
    assert_eq!(code, EXIT_OK);
    assert!(out.contains("NAME"));
    assert!(err.is_empty());
}

#[test]
fn list_empty_json() {
    let svc = MockService::deterministic();
    let (code, out, _) = run_cmd(&svc, &Command::List { json: true }, false);
    assert_eq!(code, EXIT_OK);
    assert_eq!(out.trim(), "[]");
}

/// `list --json` stdout is exactly one JSON line carrying the expected array.
#[test]
fn list_json_is_one_line_equal_to_the_expected_array() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Running,
    );
    let (code, out, err) = run_cmd(&svc, &Command::List { json: true }, false);
    assert_eq!(code, EXIT_OK);
    assert_eq!(err, "", "stderr must be empty");
    assert_eq!(
        out,
        "[{\"name\":\"app\",\"engine\":\"postgresql\",\"version\":\"17.4\",\"port\":5432,\"status\":\"running\"}]\n"
    );
    let parsed: serde_json::Value = serde_json::from_str(out.trim()).expect("json");
    assert_eq!(
        parsed,
        serde_json::json!([{
            "name": "app",
            "engine": "postgresql",
            "version": "17.4",
            "port": 5432,
            "status": "running"
        }])
    );
}

#[test]
fn status_missing_is_exit_3() {
    let svc = MockService::deterministic();
    let (code, _, err) = run_cmd(
        &svc,
        &Command::Status {
            name: "nope".into(),
        },
        false,
    );
    assert_eq!(code, EXIT_NOT_FOUND);
    assert!(err.contains("DBL-1002"));
}

#[test]
fn status_running_exit_0() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Running,
    );
    let (code, out, _) = run_cmd(&svc, &Command::Status { name: "app".into() }, false);
    assert_eq!(code, EXIT_OK);
    assert_eq!(out.trim(), "running");
}

#[test]
fn status_stopped_exit_1() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    let (code, out, _) = run_cmd(&svc, &Command::Status { name: "app".into() }, false);
    assert_eq!(code, EXIT_GENERAL);
    assert_eq!(out.trim(), "stopped");
}

/// F-CLI-03 AC1: `uri` stdout is the URI and one newline, with no decoration
/// of any kind, and stderr is empty.
#[test]
fn uri_stdout_is_exactly_the_uri_and_stderr_is_empty() {
    let svc = MockService::deterministic();
    let id = svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Running,
    );
    let uri = svc.connection_uri(&id, UriFormat::Uri).expect("uri");
    let expected = format!("{}\n", uri.expose());
    assert!(
        uri.expose().contains("pwd-"),
        "the fixture must carry a credential or this proves nothing"
    );
    let (code, out, err) = run_cmd(
        &svc,
        &Command::Uri {
            name: "app".into(),
            env: false,
        },
        false,
    );
    assert_eq!(code, EXIT_OK);
    assert_eq!(out, expected, "stdout must be the bare URI plus newline");
    assert_eq!(err, "", "stderr must be empty on success");
}

#[test]
fn non_uri_commands_never_print_password() {
    let svc = MockService::deterministic();
    let id = svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    let uri = svc.connection_uri(&id, UriFormat::Uri).expect("uri");
    let secret = uri.expose().to_owned();
    // extract password portion roughly
    let pwd = secret
        .split(':')
        .nth(2)
        .unwrap_or("")
        .split('@')
        .next()
        .unwrap_or("");
    assert!(!pwd.is_empty(), "expected password in uri");

    let cmds = vec![
        Command::List { json: false },
        Command::List { json: true },
        Command::Status { name: "app".into() },
        Command::Logs {
            name: "app".into(),
            follow: false,
            lines: 50,
        },
    ];
    for cmd in cmds {
        let (code, out, err) = run_cmd(&svc, &cmd, false);
        assert_no_secret(code, &out, &err, pwd);
    }
}

#[test]
fn delete_refuses_without_yes_when_not_interactive() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    let (code, _out, err) = run_cmd(
        &svc,
        &Command::Delete {
            name: "app".into(),
            yes: false,
            purge: false,
        },
        false,
    );
    assert_eq!(code, EXIT_BAD_ARGS);
    assert!(err.contains("--yes"));
    // instance still exists
    assert_eq!(svc.list_instances().unwrap().len(), 1);
}

/// `uri --env` prints the core builder's `.env` block verbatim plus one newline;
/// the expectation is the service's own value, not a substring of it.
#[test]
fn uri_env_stdout_equals_the_core_dotenv_block() {
    let svc = MockService::deterministic();
    let id = svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Running,
    );
    let block = svc.connection_uri(&id, UriFormat::DotEnv).expect("dotenv");
    let expected = format!("{}\n", block.expose());
    let (code, out, err) = run_cmd(
        &svc,
        &Command::Uri {
            name: "app".into(),
            env: true,
        },
        false,
    );
    assert_eq!(code, EXIT_OK);
    assert_eq!(out, expected, "stdout must be the core .env block verbatim");
    assert_eq!(err, "", "stderr must be empty on success");
}

/// A no-password instance's `uri` is exit 0 and the no-password shape, exactly.
///
/// This is the W3-D round-1 defect pinned at the CLI boundary: the reviewer saw
/// `dblocal uri` exit 3 (`DBL-1002`) for an instance created with
/// `--no-password`. The expected strings are written out literally here, so a
/// service (real or mock) that reintroduces a credential requirement, a colon,
/// or a `default:` userinfo fails this test rather than agreeing with itself.
#[test]
fn uri_of_a_no_password_instance_is_the_no_password_shape() {
    let cases: &[(EngineKey, u16, &str)] = &[
        (
            EngineKey::Postgresql,
            5432,
            "postgresql://postgres@127.0.0.1:5432/postgres",
        ),
        (EngineKey::Mysql, 3306, "mysql://root@127.0.0.1:3306/"),
        (EngineKey::Mariadb, 3307, "mysql://root@127.0.0.1:3307/"),
        (EngineKey::Valkey, 6379, "redis://127.0.0.1:6379/0"),
    ];
    for &(engine, port, expected) in cases {
        let svc = MockService::deterministic();
        svc.seed_instance(
            no_password_spec("open", engine, "17.4", port),
            InstanceStatus::Running,
        );
        let (code, out, err) = run_cmd(
            &svc,
            &Command::Uri {
                name: "open".into(),
                env: false,
            },
            false,
        );
        assert_eq!(code, EXIT_OK, "{engine:?}: {err}");
        assert_eq!(out, format!("{expected}\n"), "{engine:?} uri stdout");
        assert_eq!(err, "", "{engine:?}: stderr must be empty");
        // The userinfo, if any, carries no colon: `user@` and never `user:@`,
        // and Valkey has no userinfo at all.
        let authority = out.split("://").nth(1).unwrap_or("");
        if let Some((userinfo, _host)) = authority.split_once('@') {
            assert!(
                !userinfo.contains(':'),
                "{engine:?} put a colon in the userinfo: {out}"
            );
        } else if engine != EngineKey::Valkey {
            panic!("{engine:?} should carry userinfo: {out}");
        }
        if engine == EngineKey::Valkey {
            assert!(!out.contains('@'), "{engine:?} printed userinfo: {out}");
        }
    }
}

/// A no-password Valkey `uri --env` has no password key and no empty value.
#[test]
fn uri_env_of_a_no_password_instance_has_no_password_line() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        no_password_spec("open", EngineKey::Valkey, "9.2.0", 6379),
        InstanceStatus::Running,
    );
    let (code, out, err) = run_cmd(
        &svc,
        &Command::Uri {
            name: "open".into(),
            env: true,
        },
        false,
    );
    assert_eq!(code, EXIT_OK, "{err}");
    assert_eq!(
        out, "REDIS_URL=redis://127.0.0.1:6379/0\nREDIS_HOST=127.0.0.1\nREDIS_PORT=6379\n\n",
        "the .env block must be exactly the no-password block"
    );
    assert!(
        !out.contains("PASSWORD"),
        "a password key must not appear: {out}"
    );
    assert!(!out.contains('@'), "an empty userinfo leaked: {out}");
}

/// A no-password PostgreSQL `.env` keeps `DATABASE_URL` and omits `DB_PASSWORD`.
#[test]
fn postgres_uri_env_without_a_password_has_no_db_password_line() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        no_password_spec("open", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Running,
    );
    let (code, out, err) = run_cmd(
        &svc,
        &Command::Uri {
            name: "open".into(),
            env: true,
        },
        false,
    );
    assert_eq!(code, EXIT_OK, "{err}");
    assert_eq!(
        out,
        "DATABASE_URL=postgresql://postgres@127.0.0.1:5432/postgres\n\
         DB_HOST=127.0.0.1\n\
         DB_PORT=5432\n\
         DB_DATABASE=postgres\n\
         DB_USERNAME=postgres\n\
         \n",
        "the .env block must be exactly the no-password block"
    );
    assert!(
        !out.contains("DB_PASSWORD"),
        "DB_PASSWORD must be omitted: {out}"
    );
}

#[test]
fn start_already_running_prints_already_running() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Running,
    );
    let (code, _out, err) = run_cmd(
        &svc,
        &Command::Start {
            name: "app".into(),
            timeout: 5,
        },
        false,
    );
    assert_eq!(code, EXIT_OK);
    assert!(err.contains("already running"));
}

#[test]
fn stop_already_stopped_is_ok() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    let (code, _out, err) = run_cmd(&svc, &Command::Stop { name: "app".into() }, false);
    assert_eq!(code, EXIT_OK);
    assert!(err.contains("already stopped"));
}

#[test]
fn delete_with_yes_purges() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    let (code, _out, err) = run_cmd(
        &svc,
        &Command::Delete {
            name: "app".into(),
            yes: true,
            purge: true,
        },
        false,
    );
    assert_eq!(code, EXIT_OK, "err={err}");
    assert!(svc.list_instances().unwrap().is_empty());
}

#[test]
fn delete_with_yes_archives_reports_path() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    let (code, _out, err) = run_cmd(
        &svc,
        &Command::Delete {
            name: "app".into(),
            yes: true,
            purge: false,
        },
        false,
    );
    assert_eq!(code, EXIT_OK, "err={err}");
    assert!(err.contains("Data archived to") || err.to_lowercase().contains("archiv"));
}

#[test]
fn logs_returns_lines() {
    let svc = MockService::deterministic();
    let id = svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    let _ = id;
    let (code, out, _) = run_cmd(
        &svc,
        &Command::Logs {
            name: "app".into(),
            follow: false,
            lines: 200,
        },
        false,
    );
    assert_eq!(code, EXIT_OK);
    // mock seeds at least one log line
    assert!(!out.trim().is_empty());
}

#[test]
fn formatters_table_and_json_golden() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Running,
    );
    let views = svc.list_instances().unwrap();
    let table = output::format_list_table(&views);
    assert!(table.contains("NAME"));
    assert!(table.contains("ENGINE"));
    assert!(table.contains("app"));
    assert!(table.contains("postgresql"));
    assert!(table.contains("5432"));
    assert!(table.contains("running"));

    let json = output::format_list_json(&views);
    assert_eq!(
        json,
        "[{\"name\":\"app\",\"engine\":\"postgresql\",\"version\":\"17.4\",\"port\":5432,\"status\":\"running\"}]"
    );
}

#[test]
fn exit_code_table_maps_codes() {
    use dblocal_core::DblError;
    assert_eq!(
        output::code_for(&DblError::NotFound {
            kind: "instance",
            name: "x".into()
        }),
        3
    );
    assert_eq!(
        output::code_for(&DblError::Engine { reason: "x".into() }),
        4
    );
    assert_eq!(
        output::code_for(&DblError::InvalidInput {
            field: "x",
            reason: "y".into()
        }),
        2
    );
    assert_eq!(
        output::code_for(&DblError::InvalidTransition { from: "a", to: "b" }),
        2
    );
    assert_eq!(
        output::code_for(&DblError::Storage { reason: "x".into() }),
        1
    );
}

/// A service whose every read fails, so the handlers' error branches run.
struct BrokenService;

macro_rules! broken {
    ($($sig:tt)*) => {
        $($sig)* {
            Err(dblocal_core::DblError::Storage {
                reason: "store unavailable".to_owned(),
            })
        }
    };
}

impl DblService for BrokenService {
    fn list_instances(&self) -> dblocal_core::Result<Vec<dblocal_core::service::InstanceView>> {
        Err(dblocal_core::DblError::Storage {
            reason: "store unavailable".to_owned(),
        })
    }
    fn catalog(&self) -> dblocal_core::Result<dblocal_core::service::CatalogView> {
        Err(dblocal_core::DblError::Storage {
            reason: "store unavailable".to_owned(),
        })
    }
    broken!(fn refresh_catalog(&self, _f: bool) -> dblocal_core::Result<dblocal_core::service::OperationId>);
    fn validate_spec(&self, _s: &InstanceSpec) -> dblocal_core::Result<()> {
        Ok(())
    }
    broken!(fn suggest_port(&self, _e: EngineKey) -> dblocal_core::Result<Port>);
    broken!(fn create_instance(&self, _s: InstanceSpec) -> dblocal_core::Result<dblocal_core::service::OperationId>);
    broken!(fn start_instance(&self, _i: &InstanceId) -> dblocal_core::Result<dblocal_core::service::OperationId>);
    broken!(fn stop_instance(&self, _i: &InstanceId) -> dblocal_core::Result<dblocal_core::service::OperationId>);
    broken!(fn delete_instance(&self, _i: &InstanceId, _m: dblocal_core::model::DeleteMode) -> dblocal_core::Result<dblocal_core::service::OperationId>);
    fn change_port(&self, _i: &InstanceId, _p: Port) -> dblocal_core::Result<()> {
        Ok(())
    }
    fn data_size(&self, _i: &InstanceId) -> dblocal_core::Result<u64> {
        Ok(0)
    }
    fn set_autostart(&self, _i: &InstanceId, _e: bool) -> dblocal_core::Result<()> {
        Ok(())
    }
    fn cancel_operation(&self, _o: &OperationId) -> dblocal_core::Result<()> {
        Ok(())
    }
    broken!(fn connection_uri(&self, _i: &InstanceId, _f: dblocal_core::service::UriFormat) -> dblocal_core::Result<dblocal_core::secret::SecretString>);
    broken!(fn tail_logs(&self, _i: &InstanceId, _n: usize) -> dblocal_core::Result<Vec<String>>);
    fn watch_logs(&self, _i: &InstanceId, _e: bool) -> dblocal_core::Result<()> {
        Ok(())
    }
    fn list_runtimes(&self) -> dblocal_core::Result<Vec<dblocal_core::model::Runtime>> {
        Ok(Vec::new())
    }
    fn remove_runtime(&self, _r: &str) -> dblocal_core::Result<()> {
        Ok(())
    }
    fn list_archives(&self) -> dblocal_core::Result<Vec<dblocal_core::model::ArchivedData>> {
        Ok(Vec::new())
    }
    fn purge_archive(&self, _a: &str) -> dblocal_core::Result<()> {
        Ok(())
    }
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<ServiceEvent> {
        let (tx, rx) = tokio::sync::broadcast::channel(4);
        drop(tx);
        rx
    }
}

#[test]
fn list_and_inspection_propagate_a_store_failure() {
    let service = BrokenService;
    for cmd in [
        Command::List { json: false },
        Command::List { json: true },
        Command::Status { name: "x".into() },
        Command::Uri {
            name: "x".into(),
            env: false,
        },
        Command::Logs {
            name: "x".into(),
            follow: false,
            lines: 5,
        },
    ] {
        let (code, out, err) = run_cmd(&service, &cmd, false);
        assert_eq!(code, EXIT_GENERAL, "{cmd:?}: {err}");
        assert!(err.contains("DBL-3003"), "{cmd:?}: {err}");
        assert!(!out.contains("DBL-3003"), "the code belongs on stderr");
    }
}
