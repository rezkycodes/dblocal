//! `create`, `start`, `stop` and `delete` against the mock service.

use super::*;

#[test]
fn create_installed_runtime_prints_uri_and_exits_0() {
    let svc = MockService::deterministic();
    let (code, out, err) = run_cmd(
        &svc,
        &Command::Create {
            engine: "postgresql".into(),
            version: "17.4".into(),
            name: "made".into(),
            port: None,
            no_password: false,
        },
        false,
    );
    assert_eq!(code, EXIT_OK, "stderr: {err}");
    assert!(out.trim().starts_with("postgresql://"), "out: {out}");
    assert_eq!(svc.list_instances().unwrap().len(), 1);
}

#[test]
fn create_no_password_has_no_secret() {
    let svc = MockService::deterministic();
    let (code, out, err) = run_cmd(
        &svc,
        &Command::Create {
            engine: "valkey".into(),
            version: "8.1.0".into(),
            name: "open".into(),
            port: Some(25000),
            no_password: true,
        },
        false,
    );
    assert_eq!(code, EXIT_OK, "stderr: {err}");
    assert!(
        out.trim().starts_with("redis://127.0.0.1:25000"),
        "out: {out}"
    );
    let view = svc.list_instances().unwrap().into_iter().next().unwrap();
    assert_eq!(view.auth, AuthMode::None);
}

#[test]
fn create_unknown_engine_is_bad_arguments() {
    let svc = MockService::deterministic();
    let (code, _out, err) = run_cmd(
        &svc,
        &Command::Create {
            engine: "oracle".into(),
            version: "1".into(),
            name: "x1".into(),
            port: None,
            no_password: false,
        },
        false,
    );
    assert_eq!(code, EXIT_BAD_ARGS);
    assert!(err.contains("DBL-1001"));
    assert!(svc.list_instances().unwrap().is_empty());
}

#[test]
fn create_bad_name_and_port_are_bad_arguments() {
    let svc = MockService::deterministic();
    let (code, _o, _e) = run_cmd(
        &svc,
        &Command::Create {
            engine: "postgresql".into(),
            version: "17.4".into(),
            name: "Bad_Name".into(),
            port: None,
            no_password: false,
        },
        false,
    );
    assert_eq!(code, EXIT_BAD_ARGS);
    let (code, _o, _e) = run_cmd(
        &svc,
        &Command::Create {
            engine: "postgresql".into(),
            version: "17.4".into(),
            name: "okname".into(),
            port: Some(80),
            no_password: false,
        },
        false,
    );
    assert_eq!(code, EXIT_BAD_ARGS);
}

#[test]
fn create_duplicate_name_is_bad_arguments() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("taken", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    let (code, _o, err) = run_cmd(
        &svc,
        &Command::Create {
            engine: "postgresql".into(),
            version: "17.4".into(),
            name: "taken".into(),
            port: None,
            no_password: false,
        },
        false,
    );
    assert_eq!(code, EXIT_BAD_ARGS);
    assert!(err.contains("DBL-1003"));
}

#[test]
fn start_then_stop_through_the_mock() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("life", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    let (code, _o, err) = run_cmd(
        &svc,
        &Command::Start {
            name: "life".into(),
            timeout: 10,
        },
        false,
    );
    assert_eq!(code, EXIT_OK, "stderr: {err}");
    assert!(err.contains("Siap.") || err.contains("Siap"));
    let view = svc.list_instances().unwrap().into_iter().next().unwrap();
    assert_eq!(view.status, InstanceStatus::Running);

    let (code, _o, err) = run_cmd(
        &svc,
        &Command::Stop {
            name: "life".into(),
        },
        false,
    );
    assert_eq!(code, EXIT_OK, "stderr: {err}");
    let view = svc.list_instances().unwrap().into_iter().next().unwrap();
    assert_eq!(view.status, InstanceStatus::Stopped);
}

#[test]
fn start_missing_instance_is_exit_3() {
    let svc = MockService::deterministic();
    let (code, _o, _e) = run_cmd(
        &svc,
        &Command::Start {
            name: "ghost".into(),
            timeout: 1,
        },
        false,
    );
    assert_eq!(code, EXIT_NOT_FOUND);
}

#[test]
fn start_failure_reports_the_diagnosis_and_exits_4() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("broken", EngineKey::Valkey, "8.1.0", 6379),
        InstanceStatus::Stopped,
    );
    svc.push_failure(dblocal_core::service::mock::ScriptedFailure::StartTimeout);
    let (code, _o, err) = run_cmd(
        &svc,
        &Command::Start {
            name: "broken".into(),
            timeout: 10,
        },
        false,
    );
    assert_eq!(code, 4, "stderr: {err}");
    assert!(err.contains("DBL-4001"), "stderr: {err}");
    assert!(err.contains("diagnosis:"), "stderr: {err}");
}

#[test]
fn stop_missing_instance_is_exit_3_and_stop_invalid_is_general() {
    let svc = MockService::deterministic();
    let (code, _o, _e) = run_cmd(
        &svc,
        &Command::Stop {
            name: "ghost".into(),
        },
        false,
    );
    assert_eq!(code, EXIT_NOT_FOUND);
}

#[test]
fn delete_missing_instance_is_exit_3() {
    let svc = MockService::deterministic();
    let (code, _o, _e) = run_cmd(
        &svc,
        &Command::Delete {
            name: "ghost".into(),
            yes: true,
            purge: true,
        },
        false,
    );
    assert_eq!(code, EXIT_NOT_FOUND);
}

#[test]
fn logs_missing_instance_is_exit_3() {
    let svc = MockService::deterministic();
    let (code, _o, _e) = run_cmd(
        &svc,
        &Command::Logs {
            name: "ghost".into(),
            follow: false,
            lines: 10,
        },
        false,
    );
    assert_eq!(code, EXIT_NOT_FOUND);
}

#[test]
fn uri_missing_instance_is_exit_3_and_uri_is_exit_0() {
    let svc = MockService::deterministic();
    let (code, _o, _e) = run_cmd(
        &svc,
        &Command::Uri {
            name: "ghost".into(),
            env: false,
        },
        false,
    );
    assert_eq!(code, EXIT_NOT_FOUND);
}

#[test]
fn delete_then_purge_archive_round_trip() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("arch", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    let (code, _o, err) = run_cmd(
        &svc,
        &Command::Delete {
            name: "arch".into(),
            yes: true,
            purge: false,
        },
        false,
    );
    assert_eq!(code, EXIT_OK, "stderr: {err}");
    let archives = svc.list_archives().unwrap();
    assert_eq!(archives.len(), 1);
    assert!(
        err.contains(&archives[0].path().display().to_string()),
        "stderr: {err}"
    );
}

#[test]
fn interactive_delete_requires_the_exact_name() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("guard", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    let wrong: &dyn Fn() -> Option<String> = &|| Some("other".to_owned());
    let (code, _o, err) = run_cmd_with(
        &svc,
        &Command::Delete {
            name: "guard".into(),
            yes: false,
            purge: true,
        },
        true,
        wrong,
    );
    assert_eq!(code, EXIT_BAD_ARGS);
    assert!(err.contains("DBL-1001"));
    assert_eq!(
        svc.list_instances().unwrap().len(),
        1,
        "a refused delete keeps the row"
    );

    let right: &dyn Fn() -> Option<String> = &|| Some("guard".to_owned());
    let (code, _o, err) = run_cmd_with(
        &svc,
        &Command::Delete {
            name: "guard".into(),
            yes: false,
            purge: true,
        },
        true,
        right,
    );
    assert_eq!(code, EXIT_OK, "stderr: {err}");
    assert!(svc.list_instances().unwrap().is_empty());
}

#[test]
fn interactive_delete_with_no_input_is_refused() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("quiet", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    let none = &|| None;
    let (code, _o, err) = run_cmd_with(
        &svc,
        &Command::Delete {
            name: "quiet".into(),
            yes: false,
            purge: true,
        },
        true,
        none,
    );
    assert_eq!(code, EXIT_BAD_ARGS);
    assert!(err.contains("gagal membaca konfirmasi"));
    assert_eq!(svc.list_instances().unwrap().len(), 1);
}

#[test]
fn start_failure_without_diagnosis_reports_the_code() {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("portless", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    svc.push_failure(dblocal_core::service::mock::ScriptedFailure::PortInUse { port: 5432 });
    let (code, _o, err) = run_cmd(
        &svc,
        &Command::Start {
            name: "portless".into(),
            timeout: 5,
        },
        false,
    );
    assert_ne!(code, EXIT_OK);
    assert!(err.contains("DBL-1005"), "stderr: {err}");
}

#[test]
fn logs_follow_returns_when_the_instance_disappears() {
    let svc = std::sync::Arc::new(MockService::deterministic());
    let id = svc.seed_instance(
        spec("gone", EngineKey::Postgresql, "17.4", 5432),
        InstanceStatus::Stopped,
    );
    // The follower polls `list_instances` each interval; once the row is gone it
    // returns 0 instead of following nothing forever.
    let follower_service = std::sync::Arc::clone(&svc);
    let follower = std::thread::spawn(move || {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let no_confirm = &|| None;
        let mut ctx = Ctx {
            service: follower_service.as_ref(),
            out: &mut out,
            err: &mut err,
            is_interactive: false,
            stdout_is_tty: false,
            stderr_is_tty: false,
            confirm: no_confirm,
        };
        crate::commands::logs::run(&mut ctx, "gone", true, 10)
    });
    std::thread::sleep(std::time::Duration::from_millis(200));
    svc.delete_instance(&id, dblocal_core::model::DeleteMode::PurgeData)
        .expect("delete accepted");
    let code = follower.join().expect("follower thread");
    assert_eq!(code, EXIT_OK);
}

#[test]
fn create_port_in_use_exits_bad_arguments() {
    let svc = MockService::deterministic();
    svc.push_failure(dblocal_core::service::mock::ScriptedFailure::PortInUse { port: 5432 });
    let (code, _o, err) = run_cmd(
        &svc,
        &Command::Create {
            engine: "postgresql".into(),
            version: "17.4".into(),
            name: "conflicted".into(),
            port: Some(5432),
            no_password: false,
        },
        false,
    );
    assert_eq!(code, EXIT_BAD_ARGS, "stderr: {err}");
    assert!(err.contains("DBL-1005"));
}

#[test]
fn create_download_failure_is_general_and_rolls_back() {
    let svc = MockService::deterministic();
    svc.push_failure(dblocal_core::service::mock::ScriptedFailure::DownloadFailed);
    let (code, _o, err) = run_cmd(
        &svc,
        &Command::Create {
            engine: "mariadb".into(),
            version: "11.4.2".into(),
            name: "dlfail".into(),
            port: None,
            no_password: false,
        },
        false,
    );
    assert_eq!(code, EXIT_GENERAL, "stderr: {err}");
    assert!(err.contains("DBL-3002"), "stderr: {err}");
}

#[test]
fn create_rolled_back_exits_engine_failed() {
    let svc = MockService::deterministic();
    svc.push_failure(dblocal_core::service::mock::ScriptedFailure::CreateRolledBack);
    let (code, _o, err) = run_cmd(
        &svc,
        &Command::Create {
            engine: "postgresql".into(),
            version: "17.4".into(),
            name: "rollback".into(),
            port: None,
            no_password: false,
        },
        false,
    );
    assert_eq!(code, 4, "stderr: {err}");
    assert!(err.contains("DBL-4001"), "stderr: {err}");
    assert!(svc.list_instances().unwrap().is_empty());
}

#[test]
fn create_downloads_a_missing_runtime_then_prints_the_uri() {
    let svc = MockService::deterministic();
    // Mysql 8.4.4 is not pre-installed in the mock's runtime set.
    let (code, out, err) = run_cmd(
        &svc,
        &Command::Create {
            engine: "mysql".into(),
            version: "8.4.4".into(),
            name: "fresh-mysql".into(),
            port: None,
            no_password: false,
        },
        false,
    );
    assert_eq!(code, EXIT_OK, "stderr: {err}");
    assert!(out.trim().starts_with("mysql://"), "out: {out}");
}

#[test]
fn create_download_progress_goes_to_stderr_only() {
    let svc = MockService::deterministic();
    let (code, out, err) = run_cmd(
        &svc,
        &Command::Create {
            engine: "mariadb".into(),
            version: "11.4.2".into(),
            name: "progress".into(),
            port: None,
            no_password: false,
        },
        false,
    );
    assert_eq!(code, EXIT_OK, "stderr: {err}");
    // stdout carries the URI and nothing else.
    assert_eq!(out.trim().lines().count(), 1, "out: {out}");
    assert!(
        err.contains("unduhan") || err.contains("downloading"),
        "err: {err}"
    );
}
