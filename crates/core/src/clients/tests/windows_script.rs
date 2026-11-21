use std::path::{Path, PathBuf};

use crate::clients::{
    ClientError, ClientKind, ClientResult, DetectedClient, LaunchEnv, LaunchPlan, OpenTarget,
    OsKind, TempFile, TerminalChoice, TerminalKind, plan_open,
};
use crate::model::EngineKey;
use crate::secret::SecretString;

use super::super::terminal::launcher_temp_file;

const PROGRAM: &str = r"C:\bin\psql.exe";

fn windows_env(password: &str) -> Vec<LaunchEnv> {
    vec![LaunchEnv::new("PGPASSWORD", SecretString::from(password))]
}

fn windows_temp(password: &str) -> ClientResult<TempFile> {
    launcher_temp_file(
        Path::new(PROGRAM),
        &[String::from("postgresql://u@h:5432/d")],
        &windows_env(password),
        OsKind::Windows,
    )
}

fn windows_plan(password: &str) -> ClientResult<LaunchPlan> {
    let target = OpenTarget::new(
        EngineKey::Postgresql,
        "127.0.0.1",
        5432,
        "postgres",
        "postgres",
        "app",
    )
    .expect("valid target");
    let client = DetectedClient::new(
        ClientKind::Psql,
        PathBuf::from(PROGRAM),
        ClientKind::Psql.supported_engines(),
    );
    let terminal = TerminalChoice::new("wt.exe", TerminalKind::WindowsTerminal);
    plan_open(
        &client,
        &target,
        Some(&SecretString::from(password)),
        OsKind::Windows,
        &terminal,
    )
}

#[test]
fn windows_launcher_doubles_every_percent_in_password() {
    let cases = [
        ("%PATH%", "%%PATH%%"),
        ("100%", "100%%"),
        ("a%%b", "a%%%%b"),
        ("%", "%%"),
    ];
    for (input, doubled) in cases {
        let temp = windows_temp(input).expect("windows launcher");
        let contents = temp.contents().expose();
        let expected = format!("  set \"PGPASSWORD={doubled}\"\r\n");
        assert!(
            contents.contains(&expected),
            "password {input:?} must be written as {expected:?}, got:\n{contents}"
        );
        let raw = format!("  set \"PGPASSWORD={input}\"\r\n");
        assert!(
            !contents.contains(&raw),
            "password {input:?} must not appear un-doubled as {raw:?}"
        );
        assert!(
            contents.contains(PROGRAM),
            "client line must be untouched: {contents}"
        );
    }
}

#[test]
fn windows_launcher_rejects_quote_cr_lf_before_building_anything() {
    for bad in ["a\"b", "a\rb", "a\nb"] {
        let err = windows_temp(bad).expect_err("windows launcher must reject");
        assert!(
            matches!(err, ClientError::InvalidTarget { field: "password" }),
            "password {bad:?} must give typed InvalidTarget(password), got {err:?}"
        );

        let plan_err = windows_plan(bad).expect_err("windows plan must reject");
        assert!(
            matches!(plan_err, ClientError::InvalidTarget { field: "password" }),
            "plan for {bad:?} must give typed InvalidTarget(password), got {plan_err:?}"
        );
    }
}

#[test]
fn posix_launcher_accepts_quote_cr_lf_password() {
    for os in [OsKind::Linux, OsKind::Macos] {
        for value in ["a\"b", "a\rb", "a\nb"] {
            let temp = launcher_temp_file(
                Path::new("/usr/bin/psql"),
                &[String::from("postgresql://u@h:5432/d")],
                &windows_env(value),
                os,
            )
            .unwrap_or_else(|e| panic!("{os:?} must accept {value:?}: {e:?}"));
            assert!(temp.contents().expose().contains("PGPASSWORD="));
        }
    }
}

#[test]
fn windows_launcher_structure_is_pinned() {
    let temp = windows_temp("pw").expect("windows launcher");
    let contents = temp.contents().expose();

    assert!(
        contents.starts_with("@echo off\r\n(\r\n"),
        "must start with @echo off, CRLF, opening paren: {contents:?}"
    );
    assert!(
        contents.ends_with(")\r\n"),
        "must end with closing paren and CRLF: {contents:?}"
    );

    let bytes = contents.as_bytes();
    for (i, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            assert!(i > 0 && bytes[i - 1] == b'\r', "bare LF at byte {i}");
        }
        if *byte == b'\r' {
            assert!(bytes.get(i + 1) == Some(&b'\n'), "bare CR at byte {i}");
        }
    }

    let lines: Vec<&str> = contents.split("\r\n").collect();
    assert_eq!(lines[0], "@echo off", "first line");
    assert_eq!(lines[1], "(", "opening paren line");
    assert!(lines.last() == Some(&""), "file ends with a CRLF");

    let del_index = lines
        .iter()
        .position(|l| l.trim_start().starts_with("del \"%~f0\""))
        .expect("self-delete line");
    let close_index = lines
        .iter()
        .rposition(|l| *l == ")")
        .expect("closing paren line");
    assert_eq!(
        close_index,
        lines.len() - 2,
        "only the trailing empty line follows `)`"
    );

    let client_index = close_index - 1;
    let client_line = lines[client_index];
    assert!(
        client_line.contains(PROGRAM),
        "client line must be last inside the block: {client_line:?}"
    );
    assert!(
        del_index < client_index,
        "self-delete must run before the client line"
    );
    assert_eq!(
        lines[close_index + 1],
        "",
        "nothing after the closing paren"
    );

    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let is_set = trimmed.starts_with("set \"");
        if is_set || index == client_index {
            assert!(
                index > del_index && index < close_index,
                "line {index} must be inside the block and after del: {line:?}"
            );
        }
        if is_set {
            assert!(index < client_index, "set {index} must precede client");
        }
    }
}
