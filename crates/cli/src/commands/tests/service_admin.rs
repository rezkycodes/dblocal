//! `db`, `user`, `password`, `update`, `open` command behaviour, driven against
//! the mock service (W4-A).

use dblocal_core::model::{AuthMode, EngineKey, InstanceName, InstanceSpec, InstanceStatus, Port};
use dblocal_core::service::mock::MockService;
use dblocal_core::update::{InstallKind, PackageManagerHint};

use crate::cli::{
    Command, DbCommand, EXIT_BAD_ARGS, EXIT_GENERAL, EXIT_OK, PasswordCommand, UpdateCommand,
    UserCommand,
};

use super::{run_cmd, run_cmd_with};

fn pg(name: &str) -> InstanceSpec {
    InstanceSpec::new(
        InstanceName::new(name).expect("name"),
        EngineKey::Postgresql,
        "17.4",
        Some(Port::new(5432).expect("port")),
        AuthMode::Password,
    )
    .expect("spec")
}

fn seeded() -> MockService {
    let svc = MockService::deterministic();
    svc.seed_instance(pg("app"), InstanceStatus::Running);
    svc
}

// --- db ----------------------------------------------------------------------

#[test]
fn db_list_prints_a_table_with_the_seeded_databases() {
    let svc = seeded();
    let (code, out, err) = run_cmd(
        &svc,
        &Command::Db {
            command: DbCommand::List {
                instance: "app".into(),
                json: false,
            },
        },
        false,
    );
    assert_eq!(code, EXIT_OK, "{err}");
    assert!(out.contains("NAME"));
    assert!(out.contains("postgres"));
}

#[test]
fn db_list_json_is_one_line() {
    let svc = seeded();
    let (code, out, _) = run_cmd(
        &svc,
        &Command::Db {
            command: DbCommand::List {
                instance: "app".into(),
                json: true,
            },
        },
        false,
    );
    assert_eq!(code, EXIT_OK);
    let parsed: serde_json::Value = serde_json::from_str(out.trim()).expect("json");
    assert!(parsed.is_array());
}

#[test]
fn db_create_then_drop_round_trips() {
    let svc = seeded();
    let created = run_cmd(
        &svc,
        &Command::Db {
            command: DbCommand::Create {
                instance: "app".into(),
                name: "shop_db".into(),
            },
        },
        false,
    );
    assert_eq!(created.0, EXIT_OK, "{}", created.2);
    assert_eq!(created.1, "", "stdout is not data for create");

    let dropped = run_cmd(
        &svc,
        &Command::Db {
            command: DbCommand::Drop {
                instance: "app".into(),
                name: "shop_db".into(),
                yes: true,
                confirm: None,
            },
        },
        false,
    );
    assert_eq!(dropped.0, EXIT_OK, "{}", dropped.2);
}

#[test]
fn db_create_rejects_an_invalid_name() {
    let svc = seeded();
    let (code, out, err) = run_cmd(
        &svc,
        &Command::Db {
            command: DbCommand::Create {
                instance: "app".into(),
                name: "bad name".into(),
            },
        },
        false,
    );
    assert_eq!(code, EXIT_BAD_ARGS);
    assert_eq!(out, "");
    assert!(err.contains("DBL-8101"), "{err}");
}

#[test]
fn db_drop_without_confirmation_is_refused_non_interactively() {
    let svc = seeded();
    let (code, _, err) = run_cmd(
        &svc,
        &Command::Db {
            command: DbCommand::Drop {
                instance: "app".into(),
                name: "postgres".into(),
                yes: false,
                confirm: None,
            },
        },
        false,
    );
    assert_eq!(code, EXIT_BAD_ARGS);
    assert!(err.contains("--confirm") || err.contains("--yes"), "{err}");
}

#[test]
fn db_drop_accepts_a_matching_confirm() {
    let svc = seeded();
    let (code, _, err) = run_cmd(
        &svc,
        &Command::Db {
            command: DbCommand::Drop {
                instance: "app".into(),
                name: "app_development".into(),
                yes: false,
                confirm: Some("app_development".into()),
            },
        },
        false,
    );
    assert_eq!(code, EXIT_OK, "{err}");
}

#[test]
fn db_drop_rejects_a_mismatched_confirm() {
    let svc = seeded();
    let (code, _, err) = run_cmd(
        &svc,
        &Command::Db {
            command: DbCommand::Drop {
                instance: "app".into(),
                name: "app_development".into(),
                yes: false,
                confirm: Some("wrong".into()),
            },
        },
        false,
    );
    assert_eq!(code, EXIT_BAD_ARGS);
    assert!(err.contains("does not match"), "{err}");
}

#[test]
fn db_drop_prompts_when_interactive() {
    let svc = seeded();
    let (code, _, _) = run_cmd_with(
        &svc,
        &Command::Db {
            command: DbCommand::Drop {
                instance: "app".into(),
                name: "app_development".into(),
                yes: false,
                confirm: None,
            },
        },
        true,
        &|| Some("app_development\n".to_owned()),
    );
    assert_eq!(code, EXIT_OK);
}

// --- user --------------------------------------------------------------------

#[test]
fn user_create_prints_the_password_once_to_stdout() {
    let svc = seeded();
    let (code, out, err) = run_cmd(
        &svc,
        &Command::User {
            command: UserCommand::Create {
                instance: "app".into(),
                name: "report_user".into(),
                db: "app_development".into(),
                level: "ro".into(),
            },
        },
        false,
    );
    assert_eq!(code, EXIT_OK, "{err}");
    assert!(
        out.starts_with("pwd-user-"),
        "stdout is the password: {out}"
    );
    assert!(err.contains("only once"), "{err}");
}

#[test]
fn user_create_rejects_an_unknown_level() {
    let svc = seeded();
    let (code, _, err) = run_cmd(
        &svc,
        &Command::User {
            command: UserCommand::Create {
                instance: "app".into(),
                name: "report_user".into(),
                db: "app_development".into(),
                level: "admin".into(),
            },
        },
        false,
    );
    assert_eq!(code, EXIT_BAD_ARGS);
    assert!(err.contains("ro") && err.contains("rw"), "{err}");
}

// --- password ----------------------------------------------------------------

#[test]
fn password_reset_prints_the_new_uri_once() {
    let svc = seeded();
    let (code, out, err) = run_cmd(
        &svc,
        &Command::Password {
            command: PasswordCommand::Reset {
                instance: "app".into(),
            },
        },
        false,
    );
    assert_eq!(code, EXIT_OK, "{err}");
    assert!(out.starts_with("postgresql://"), "{out}");
    assert!(
        out.contains("pwd-reset-"),
        "the URI carries the new password: {out}"
    );
    assert!(err.contains("changed"), "{err}");
}

// --- update ------------------------------------------------------------------

#[test]
fn update_check_reports_the_available_version() {
    let svc = MockService::deterministic();
    let (code, out, _) = run_cmd(
        &svc,
        &Command::Update {
            command: UpdateCommand::Check { json: false },
        },
        false,
    );
    assert_eq!(code, EXIT_OK);
    assert!(out.contains("0.9.0"), "{out}");
}

#[test]
fn update_check_json_has_the_expected_shape() {
    let svc = MockService::deterministic();
    let (code, out, _) = run_cmd(
        &svc,
        &Command::Update {
            command: UpdateCommand::Check { json: true },
        },
        false,
    );
    assert_eq!(code, EXIT_OK);
    let parsed: serde_json::Value = serde_json::from_str(out.trim()).expect("json");
    assert_eq!(parsed["can_self_update"], true);
    assert_eq!(parsed["available"]["version"], "0.9.0");
}

#[test]
fn update_apply_needs_yes_non_interactively() {
    let svc = MockService::deterministic();
    let (code, _, err) = run_cmd(
        &svc,
        &Command::Update {
            command: UpdateCommand::Apply { yes: false },
        },
        false,
    );
    assert_eq!(code, EXIT_BAD_ARGS);
    assert!(err.contains("--yes"), "{err}");
}

#[test]
fn update_apply_succeeds_for_a_self_update_kind() {
    let svc = MockService::deterministic();
    let (code, _, err) = run_cmd(
        &svc,
        &Command::Update {
            command: UpdateCommand::Apply { yes: true },
        },
        false,
    );
    assert_eq!(code, EXIT_OK, "{err}");
}

#[test]
fn update_apply_refuses_a_package_manager_install() {
    let svc = MockService::deterministic();
    svc.set_install_kind(InstallKind::DistPackage {
        exe_path: "/usr/bin/dblocal".into(),
        hint: PackageManagerHint::Dpkg,
    });
    let (code, _, err) = run_cmd(
        &svc,
        &Command::Update {
            command: UpdateCommand::Apply { yes: true },
        },
        false,
    );
    assert_eq!(code, EXIT_GENERAL);
    assert!(err.contains("DBL-8008"), "{err}");
    assert!(
        err.contains("package manager"),
        "the hint names the manager: {err}"
    );
}

// --- open --------------------------------------------------------------------

#[test]
fn open_unknown_client_is_a_bad_argument() {
    let svc = seeded();
    let (code, _, err) = run_cmd(
        &svc,
        &Command::Open {
            instance: "app".into(),
            client: "nope".into(),
            dry_run: true,
        },
        false,
    );
    assert_eq!(code, EXIT_BAD_ARGS);
    assert!(err.contains("Unknown client"), "{err}");
}

#[test]
fn open_dry_run_prints_the_plan_without_a_password() {
    let svc = seeded();
    let (code, out, err) = run_cmd(
        &svc,
        &Command::Open {
            instance: "app".into(),
            client: "tableplus".into(),
            dry_run: true,
        },
        false,
    );
    assert_eq!(code, EXIT_OK, "{err}");
    assert!(out.contains("program:"), "{out}");
    assert!(
        !out.contains("pwd-"),
        "the plan must not carry a password: {out}"
    );
}

#[test]
fn open_unknown_instance_is_exit_3() {
    let svc = seeded();
    let (code, _, _) = run_cmd(
        &svc,
        &Command::Open {
            instance: "ghost".into(),
            client: "tableplus".into(),
            dry_run: true,
        },
        false,
    );
    assert_eq!(code, crate::cli::EXIT_NOT_FOUND);
}
