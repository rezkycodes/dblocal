//! `runtime list` and `runtime remove` (`F-RTM-04`), against the mock service.

use dblocal_core::model::InstanceStatus;
use dblocal_core::service::DblService;
use dblocal_core::service::mock::MockService;

use crate::cli::{Command, EXIT_BAD_ARGS, EXIT_GENERAL, EXIT_NOT_FOUND, EXIT_OK, RuntimeCommand};

use super::{run_cmd, run_cmd_with};

fn list(json: bool) -> Command {
    Command::Runtime {
        command: RuntimeCommand::List { json },
    }
}

fn remove(engine: &str, version: &str, yes: bool) -> Command {
    Command::Runtime {
        command: RuntimeCommand::Remove {
            engine: engine.into(),
            version: version.into(),
            yes,
        },
    }
}

/// A mock with one instance on the seeded PostgreSQL 17.4 runtime.
fn used() -> MockService {
    let svc = MockService::deterministic();
    svc.seed_instance(
        super::spec(
            "app",
            dblocal_core::model::EngineKey::Postgresql,
            "17.4",
            5432,
        ),
        InstanceStatus::Stopped,
    );
    svc
}

#[test]
fn list_names_the_instances_that_use_a_runtime_and_marks_the_rest_unused() {
    let svc = used();
    let (code, out, err) = run_cmd(&svc, &list(false), false);
    assert_eq!(code, EXIT_OK, "{err}");
    // PostgreSQL 17.4 is used by `app`; the seeded Valkey 8.1.0 is not.
    let pg = out
        .lines()
        .find(|line| line.starts_with("postgresql"))
        .expect("postgresql row");
    assert!(pg.contains(" 17.4 "), "{pg}");
    assert!(pg.ends_with("app"), "the user is named: {pg}");
    let valkey = out
        .lines()
        .find(|line| line.starts_with("valkey"))
        .expect("valkey row");
    assert!(valkey.ends_with("unused"), "{valkey}");
}

#[test]
fn list_json_is_one_line_with_users() {
    let svc = used();
    let (code, out, _) = run_cmd(&svc, &list(true), false);
    assert_eq!(code, EXIT_OK);
    assert_eq!(out.lines().count(), 1, "{out}");
    let value: serde_json::Value = serde_json::from_str(out.trim()).expect("json");
    let rows = value.as_array().expect("array");
    let pg = rows
        .iter()
        .find(|row| row["engine"] == "postgresql")
        .expect("postgresql row");
    assert_eq!(pg["users"], serde_json::json!(["app"]));
    assert!(pg["size_bytes"].is_number());
    let valkey = rows
        .iter()
        .find(|row| row["engine"] == "valkey")
        .expect("valkey row");
    assert_eq!(valkey["users"], serde_json::json!([]));
}

#[test]
fn remove_without_yes_on_a_non_interactive_session_is_refused() {
    let svc = MockService::deterministic();
    let (code, _out, err) = run_cmd(&svc, &remove("valkey", "8.1.0", false), false);
    assert_eq!(code, EXIT_BAD_ARGS, "{err}");
    assert!(err.contains("--yes"), "{err}");
    assert_eq!(
        svc.list_runtimes().expect("runtimes").len(),
        2,
        "nothing removed"
    );
}

#[test]
fn remove_yes_on_an_unused_runtime_removes_it() {
    let svc = MockService::deterministic();
    let (code, _out, err) = run_cmd(&svc, &remove("valkey", "8.1.0", true), false);
    assert_eq!(code, EXIT_OK, "{err}");
    let left = svc.list_runtimes().expect("runtimes");
    assert_eq!(left.len(), 1);
    assert!(
        left.iter()
            .all(|rt| rt.engine() != dblocal_core::model::EngineKey::Valkey)
    );
}

#[test]
fn remove_of_a_used_runtime_is_refused_with_general_error_and_names_it() {
    let svc = used();
    let (code, _out, err) = run_cmd(&svc, &remove("postgresql", "17.4", true), false);
    assert_eq!(code, EXIT_GENERAL, "{err}");
    assert!(err.contains("app"), "the user is named: {err}");
    assert_eq!(svc.list_runtimes().expect("runtimes").len(), 2, "kept");
}

#[test]
fn remove_of_an_unknown_runtime_exits_3() {
    let svc = MockService::deterministic();
    let (code, _out, _err) = run_cmd(&svc, &remove("mariadb", "11.4", true), false);
    assert_eq!(code, EXIT_NOT_FOUND);
}

#[test]
fn remove_of_an_unknown_engine_exits_2() {
    let svc = MockService::deterministic();
    let (code, _out, _err) = run_cmd(&svc, &remove("frobnicate", "1.0", true), false);
    assert_eq!(code, EXIT_BAD_ARGS);
}

#[test]
fn interactive_remove_prompts_and_accepts_the_matching_version() {
    let svc = MockService::deterministic();
    let (code, _out, err) = run_cmd_with(&svc, &remove("valkey", "8.1.0", false), true, &|| {
        Some("8.1.0\n".to_owned())
    });
    assert_eq!(code, EXIT_OK, "{err}");
    assert_eq!(svc.list_runtimes().expect("runtimes").len(), 1);
}

#[test]
fn interactive_remove_refuses_a_mismatched_version() {
    let svc = MockService::deterministic();
    let (code, _out, err) = run_cmd_with(&svc, &remove("valkey", "8.1.0", false), true, &|| {
        Some("9.9.9\n".to_owned())
    });
    assert_eq!(code, EXIT_BAD_ARGS, "{err}");
    assert_eq!(svc.list_runtimes().expect("runtimes").len(), 2, "kept");
}
