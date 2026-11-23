//! `dblocal config show|set|restore|path` against the mock service.

use super::*;
use dblocal_core::config::messages;
use dblocal_core::service::DblService;

fn seeded() -> (MockService, InstanceId) {
    let svc = MockService::deterministic();
    svc.seed_instance(
        spec("app", EngineKey::Valkey, "8.1.0", 6379),
        InstanceStatus::Stopped,
    );
    let id = svc.list_instances().expect("list")[0].id.clone();
    (svc, id)
}

#[test]
fn show_prints_the_settings_table() {
    let (svc, _id) = seeded();
    let (code, out, err) = run_cmd(
        &svc,
        &Command::Config {
            command: crate::cli::ConfigCommand::Show {
                name: "app".into(),
                json: false,
            },
        },
        false,
    );
    assert_eq!(code, EXIT_OK, "stderr: {err}");
    assert!(out.contains(messages::HEADER_SETTING), "{out}");
    assert!(out.contains("maxmemory"), "{out}");
    assert!(out.contains(messages::HEADER_DEFAULT), "{out}");
}

#[test]
fn show_json_is_parseable_and_has_the_settings() {
    let (svc, _id) = seeded();
    let (code, out, _err) = run_cmd(
        &svc,
        &Command::Config {
            command: crate::cli::ConfigCommand::Show {
                name: "app".into(),
                json: true,
            },
        },
        false,
    );
    assert_eq!(code, EXIT_OK);
    let parsed: serde_json::Value = serde_json::from_str(out.trim()).expect("json");
    assert_eq!(parsed["engine"], "valkey");
    assert!(parsed["settings"].is_array());
    assert!(parsed["raw_path"].is_string());
}

#[test]
fn set_applies_and_prints_restart_and_backup_to_stderr() {
    let (svc, _id) = seeded();
    let (code, out, err) = run_cmd(
        &svc,
        &Command::Config {
            command: crate::cli::ConfigCommand::Set {
                name: "app".into(),
                changes: vec!["maxmemory=1gb".into()],
            },
        },
        false,
    );
    assert_eq!(code, EXIT_OK, "err: {err}");
    assert!(out.is_empty(), "set prints no data on stdout: {out}");
    assert!(err.contains(messages::MSG_SAVED), "{err}");
    assert!(err.contains(messages::MSG_BACKUP_AT), "{err}");
}

#[test]
fn set_without_equals_is_a_bad_argument() {
    let (svc, _id) = seeded();
    let (code, _out, err) = run_cmd(
        &svc,
        &Command::Config {
            command: crate::cli::ConfigCommand::Set {
                name: "app".into(),
                changes: vec!["maxmemory".into()],
            },
        },
        false,
    );
    assert_eq!(code, EXIT_BAD_ARGS);
    assert!(err.contains(messages::ERR_SET_FORMAT), "{err}");
}

#[test]
fn set_with_no_changes_is_a_bad_argument() {
    let (svc, _id) = seeded();
    let (code, _out, err) = run_cmd(
        &svc,
        &Command::Config {
            command: crate::cli::ConfigCommand::Set {
                name: "app".into(),
                changes: Vec::new(),
            },
        },
        false,
    );
    assert_eq!(code, EXIT_BAD_ARGS);
    assert!(err.contains(messages::ERR_SET_FORMAT), "{err}");
}

#[test]
fn set_refuses_a_managed_key_with_the_module_code() {
    let (svc, _id) = seeded();
    let (code, _out, err) = run_cmd(
        &svc,
        &Command::Config {
            command: crate::cli::ConfigCommand::Set {
                name: "app".into(),
                changes: vec!["port=6400".into()],
            },
        },
        false,
    );
    assert_ne!(code, EXIT_OK);
    assert!(
        err.contains(dblocal_core::config::error::CODE_MANAGED_KEY),
        "{err}"
    );
}

#[test]
fn restore_without_a_backup_reports_the_code() {
    let (svc, _id) = seeded();
    let (code, _out, err) = run_cmd(
        &svc,
        &Command::Config {
            command: crate::cli::ConfigCommand::Restore { name: "app".into() },
        },
        false,
    );
    assert_ne!(code, EXIT_OK);
    assert!(
        err.contains(dblocal_core::config::error::CODE_NO_BACKUP),
        "{err}"
    );
}

#[test]
fn restore_after_set_succeeds() {
    let (svc, _id) = seeded();
    let _ = run_cmd(
        &svc,
        &Command::Config {
            command: crate::cli::ConfigCommand::Set {
                name: "app".into(),
                changes: vec!["maxmemory=1gb".into()],
            },
        },
        false,
    );
    let (code, _out, err) = run_cmd(
        &svc,
        &Command::Config {
            command: crate::cli::ConfigCommand::Restore { name: "app".into() },
        },
        false,
    );
    assert_eq!(code, EXIT_OK, "err: {err}");
    assert!(err.contains(messages::MSG_RESTORED), "{err}");
}

#[test]
fn path_prints_the_raw_config_path() {
    let (svc, _id) = seeded();
    let (code, out, _err) = run_cmd(
        &svc,
        &Command::Config {
            command: crate::cli::ConfigCommand::Path { name: "app".into() },
        },
        false,
    );
    assert_eq!(code, EXIT_OK);
    assert!(out.trim().ends_with("valkey.conf"), "{out}");
}

#[test]
fn an_unknown_instance_is_not_found() {
    let svc = MockService::deterministic();
    let (code, _out, err) = run_cmd(
        &svc,
        &Command::Config {
            command: crate::cli::ConfigCommand::Show {
                name: "nope".into(),
                json: false,
            },
        },
        false,
    );
    assert_eq!(code, EXIT_NOT_FOUND);
    assert!(err.contains("nope"), "{err}");
}

#[test]
fn the_cli_never_prints_a_requirepass_value() {
    // The mock's view never carries requirepass, so neither the table nor the
    // JSON can leak it. This is the canary at the CLI boundary.
    let (svc, _id) = seeded();
    let (code, out, err) = run_cmd(
        &svc,
        &Command::Config {
            command: crate::cli::ConfigCommand::Show {
                name: "app".into(),
                json: true,
            },
        },
        false,
    );
    assert_eq!(code, EXIT_OK);
    assert!(!out.contains("requirepass"));
    assert!(!err.contains("requirepass"));
}
