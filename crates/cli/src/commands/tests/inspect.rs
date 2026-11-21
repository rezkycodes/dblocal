//! `list`, `status`, `uri`, `logs`, the formatters and the exit-code table.

use super::*;

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

#[test]
fn list_json_has_stable_keys() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Running,
    );
    let (code, out, _) = run_cmd(&svc, &Command::List { json: true }, false);
    assert_eq!(code, EXIT_OK);
    let v: serde_json::Value = serde_json::from_str(out.trim()).expect("json");
    let arr = v.as_array().expect("array");
    assert_eq!(arr.len(), 1);
    let obj = arr[0].as_object().expect("obj");
    for k in ["name", "engine", "version", "port", "status"] {
        assert!(obj.contains_key(k), "missing {k}");
    }
    assert_eq!(obj["name"], "app");
    assert_eq!(obj["engine"], "postgresql");
    assert_eq!(obj["status"], "running");
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

#[test]
fn uri_prints_secret_only_on_stdout() {
    let svc = MockService::deterministic();
    let id = svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Running,
    );
    let uri = svc.connection_uri(&id, UriFormat::Uri).expect("uri");
    let secret = uri.expose().to_owned();
    assert!(
        secret.contains("pwd-") || secret.contains(":"),
        "uri should contain credential"
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
    assert!(out.trim().contains(secret.as_str()));
    assert!(!err.contains(&secret), "stderr must not contain uri secret");
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

#[test]
fn uri_env_uses_dotenv_format() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("app", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Running,
    );
    let (code, out, err) = run_cmd(
        &svc,
        &Command::Uri {
            name: "app".into(),
            env: true,
        },
        false,
    );
    assert_eq!(code, EXIT_OK);
    assert!(out.contains("DATABASE_URL=") || out.contains("DB_HOST="));
    assert!(!err.contains("DATABASE_URL=")); // stderr should be clean
}

#[test]
fn start_already_running_prints_sudah_berjalan() {
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
    assert!(err.contains("sudah berjalan"));
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
    assert!(err.contains("sudah berhenti"));
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
    assert!(err.contains("Data diarsipkan ke") || err.to_lowercase().contains("arsip"));
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
    let table = output::format_list_table(&views, false);
    assert!(table.contains("NAME"));
    assert!(table.contains("ENGINE"));
    assert!(table.contains("app"));
    assert!(table.contains("postgresql"));
    assert!(table.contains("5432"));
    assert!(table.contains("running"));

    let json = output::format_list_json(&views).unwrap();
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
