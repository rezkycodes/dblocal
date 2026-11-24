//! The POSIX launcher hardening path (0700 launcher directory, owner checks,
//! `sh` scripts, symlinks). Declared `#[cfg(unix)]` in `tests/mod.rs`: the Windows
//! launcher is a `cmd` script exercised by `windows_script`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::clients::{
    ClientKind, DetectedClient, OpenTarget, OsKind, TerminalChoice, TerminalKind, plan_open,
};
use crate::model::EngineKey;
use crate::secret::SecretString;

use super::super::terminal::{
    current_uid, materialize_launcher_in, verify_secure_dir_for, write_script_file,
};

fn write_exec(path: &Path, body: &str) {
    crate::fsutil::write_executable_file_atomic(path, body.as_bytes()).unwrap();
}

fn secure_dir(base: &Path, name: &str) -> std::path::PathBuf {
    let dir = base.join(name);
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

fn psql_plan(password: &str, client_path: &Path) -> crate::clients::LaunchPlan {
    let target = OpenTarget::new(
        EngineKey::Postgresql,
        "127.0.0.1",
        5432,
        "postgres",
        "postgres",
        "app",
    )
    .unwrap();
    let client = DetectedClient::new(
        ClientKind::Psql,
        client_path.to_path_buf(),
        ClientKind::Psql.supported_engines(),
    );
    let terminal = TerminalChoice::new("xterm", TerminalKind::Xterm);
    plan_open(
        &client,
        &target,
        Some(&SecretString::from(password)),
        OsKind::Linux,
        &terminal,
    )
    .unwrap()
}

#[test]
fn h4_b_posix_launcher_preserves_hostile_password_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("pw.bin");
    let psql = dir.path().join("psql");
    write_exec(
        &psql,
        &format!(
            "#!/bin/sh\nprintf '%s' \"${{PGPASSWORD:-}}\" > \"{}\"\n",
            out.display()
        ),
    );

    let corpus: Vec<String> = vec![
        "it's".to_owned(),
        "a\"b".to_owned(),
        "$HOME".to_owned(),
        "$(id)".to_owned(),
        "`id`".to_owned(),
        ";".to_owned(),
        "&&".to_owned(),
        "a b".to_owned(),
        "a\\b".to_owned(),
        "%PATH%".to_owned(),
        "*".to_owned(),
        "-leading".to_owned(),
        "x".repeat(4096),
        "ünïcödé-🔐".to_owned(),
        "line\nbreak".to_owned(),
    ];

    for pw in corpus {
        let _ = fs::remove_file(&out);
        let plan = psql_plan(&pw, &psql);
        let temp = plan.temp_file().expect("launcher temp");
        let script_path = dir.path().join(temp.name_hint());
        crate::fsutil::write_private_file_atomic(&script_path, temp.contents().expose().as_bytes())
            .unwrap();

        let status = std::process::Command::new("sh")
            .arg(&script_path)
            .status()
            .unwrap();
        assert!(status.success(), "script failed for password bytes");
        let got = fs::read(&out).expect("output written");
        assert_eq!(got, pw.as_bytes(), "password mismatch");
        assert!(!script_path.exists(), "script should self-delete");
    }
}

#[test]
fn h4_d_create_new_rejects_second_write() {
    let dir = tempfile::tempdir().unwrap();
    let launcher = secure_dir(dir.path(), "launcher");
    let psql = dir.path().join("psql");
    write_exec(&psql, "#!/bin/sh\nexit 0\n");
    let plan = psql_plan("pw", &psql);
    let temp = plan.temp_file().expect("temp");

    let first = write_script_file(&launcher, temp);
    assert!(first.is_ok(), "first write should succeed");
    let second = write_script_file(&launcher, temp);
    assert!(second.is_err(), "second create_new write must fail");
}

#[test]
fn h4_f_sweep_removes_only_old_launcher_scripts() {
    let dir = tempfile::tempdir().unwrap();
    let launcher = secure_dir(dir.path(), "launcher");
    let old = launcher.join("dblocal-open-old.sh");
    let fresh = launcher.join("dblocal-open-fresh.sh");
    let unrelated = launcher.join("keep-me.txt");
    fs::write(&old, "#!/bin/sh\n").unwrap();
    fs::write(&fresh, "#!/bin/sh\n").unwrap();
    fs::write(&unrelated, "keep").unwrap();
    let old_time = SystemTime::now() - Duration::from_secs(11 * 60);
    fs::File::open(&old)
        .unwrap()
        .set_modified(old_time)
        .unwrap();

    let psql = dir.path().join("psql");
    write_exec(&psql, "#!/bin/sh\nexit 0\n");
    let plan = psql_plan("pw", &psql);
    let temp = plan.temp_file().expect("temp");
    let written = materialize_launcher_in(&launcher, temp).unwrap();

    assert!(!old.exists(), "old launcher should be swept");
    assert!(fresh.exists(), "fresh launcher should remain");
    assert!(unrelated.exists(), "unrelated file should remain");
    assert!(written.exists(), "new script should exist");
    let _ = fs::remove_file(written);
}

#[test]
fn h2_cli_launcher_uses_detected_absolute_client_path() {
    let dir = tempfile::tempdir().unwrap();
    let psql = dir.path().join("bin").join("psql-16");
    fs::create_dir(dir.path().join("bin")).unwrap();
    write_exec(&psql, "#!/bin/sh\nexit 0\n");

    for kind in [
        ClientKind::Psql,
        ClientKind::Mysql,
        ClientKind::Mariadb,
        ClientKind::ValkeyCli,
        ClientKind::RedisCli,
    ] {
        let engine = match kind {
            ClientKind::Psql => EngineKey::Postgresql,
            ClientKind::Mysql | ClientKind::Mariadb => EngineKey::Mysql,
            _ => EngineKey::Valkey,
        };
        let target =
            OpenTarget::new(engine, "127.0.0.1", 5432, "postgres", "postgres", "app").unwrap();
        let client = DetectedClient::new(kind, psql.clone(), kind.supported_engines());
        let terminal = TerminalChoice::new("xterm", TerminalKind::Xterm);
        let plan = plan_open(
            &client,
            &target,
            Some(&SecretString::from("pw")),
            OsKind::Linux,
            &terminal,
        )
        .unwrap();
        let script = plan.temp_file().unwrap().contents().expose();
        assert!(
            script.contains(&psql.to_string_lossy().into_owned()),
            "{kind:?} must exec the detected path, got: {script}"
        );
        assert_eq!(plan.required_path(), Some(&psql));
    }
}

#[test]
fn h4_g_i_secure_dir_checks() {
    let dir = tempfile::tempdir().unwrap();
    let uid = current_uid();

    let ok = dir.path().join("ok");
    fs::create_dir(&ok).unwrap();
    fs::set_permissions(&ok, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(verify_secure_dir_for(&ok, uid).is_ok());

    let bad_mode = dir.path().join("bad-mode");
    fs::create_dir(&bad_mode).unwrap();
    fs::set_permissions(&bad_mode, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(verify_secure_dir_for(&bad_mode, uid).is_err());

    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&ok, &link).unwrap();
    assert!(verify_secure_dir_for(&link, uid).is_err());

    // A regular file with mode 0700 passes the mode/owner checks, so this case
    // is the only one that proves the `is_dir` guard, not just the mode guard.
    let regular = dir.path().join("regular-file");
    fs::write(&regular, "x").unwrap();
    fs::set_permissions(&regular, fs::Permissions::from_mode(0o700)).unwrap();
    let err = verify_secure_dir_for(&regular, uid).unwrap_err();
    assert!(
        format!("{err:?}").contains("ScriptError"),
        "a regular file must be refused: {err:?}"
    );

    let wrong_uid = dir.path().join("wrong-uid");
    fs::create_dir(&wrong_uid).unwrap();
    fs::set_permissions(&wrong_uid, fs::Permissions::from_mode(0o700)).unwrap();
    let bad_uid = uid.wrapping_add(1);
    assert_ne!(bad_uid, uid);
    assert!(verify_secure_dir_for(&wrong_uid, bad_uid).is_err());
}

#[test]
fn h1_current_uid_matches_owner_of_created_dir() {
    use std::os::unix::fs::MetadataExt;
    let dir = tempfile::tempdir().unwrap();
    let meta = fs::symlink_metadata(dir.path()).unwrap();
    assert_eq!(current_uid(), meta.uid(), "uid must come from the OS");
    assert_ne!(current_uid(), u32::MAX);
}

#[test]
fn materialize_creates_missing_secure_dir() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("created").join("dblocal");
    let psql = dir.path().join("psql");
    write_exec(&psql, "#!/bin/sh\nexit 0\n");
    let plan = psql_plan("pw", &psql);
    let temp = plan.temp_file().expect("temp");
    let written = materialize_launcher_in(&target, temp).unwrap();
    assert!(written.exists());
    assert_eq!(written.parent(), Some(target.as_path()));
}
