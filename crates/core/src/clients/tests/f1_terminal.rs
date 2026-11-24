//! The POSIX launcher path (`sh` scripts, 0700 launcher directory, `/proc`
//! argv checks). Declared `#[cfg(unix)]` in `tests/mod.rs`: the Windows launcher
//! is a `cmd` script exercised by `windows_script`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use crate::clients::{
    ClientKind, DetectedClient, OpenTarget, OsKind, SystemSpawner, TerminalChoice, TerminalKind,
    plan_open,
};
use crate::model::EngineKey;
use crate::secret::SecretString;

fn write_exec(path: &Path, body: &str) {
    crate::fsutil::write_executable_file_atomic(path, body.as_bytes()).unwrap();
}

fn secure_launcher_dir(base: &Path, name: &str) -> std::path::PathBuf {
    let dir = base.join(name);
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

#[test]
fn fake_server_terminal_receives_password_via_script_not_env() {
    let dir = tempfile::tempdir().unwrap();
    let launcher_dir = secure_launcher_dir(dir.path(), "launcher");
    let out = dir.path().join("seen.txt");
    let term = dir.path().join("fake-terminal.sh");
    let psql = dir.path().join("psql");
    write_exec(
        &term,
        r#"#!/bin/sh
env > "$FAKE_TERM_ENV_OUT"
tr '\0' ' ' < /proc/$$/cmdline > "$FAKE_TERM_CMD_OUT"
for last do true; done
sh "$last"
"#,
    );
    write_exec(
        &psql,
        &format!(
            r#"#!/bin/sh
printf '%s' "${{PGPASSWORD:-}}" > "{}"
cat /proc/self/cmdline | tr '\0' ' ' > "{}"
"#,
            out.display(),
            dir.path().join("psql-cmd.txt").display()
        ),
    );

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
        psql.clone(),
        ClientKind::Psql.supported_engines(),
    );
    let wrapper = dir.path().join("wrapper-terminal.sh");
    write_exec(
        &wrapper,
        &format!(
            r#"#!/bin/sh
export FAKE_TERM_ENV_OUT="{term_env}"
export FAKE_TERM_CMD_OUT="{term_cmd}"
export PATH="{bin_dir}:$PATH"
exec "{term}" "$@"
"#,
            term_env = dir.path().join("term-env.txt").display(),
            term_cmd = dir.path().join("term-cmd.txt").display(),
            bin_dir = dir.path().display(),
            term = term.display()
        ),
    );
    let terminal = TerminalChoice::new(&wrapper, TerminalKind::GenericArgv);
    let secret = SecretString::from("TOPSECRET");
    let plan = plan_open(&client, &target, Some(&secret), OsKind::Linux, &terminal).unwrap();
    let script_name = plan.temp_file().unwrap().name_hint().to_owned();
    let script_path = launcher_dir.join(&script_name);

    super::resilient::spawn_resilient(&plan, &SystemSpawner::with_launcher_dir(&launcher_dir))
        .unwrap();

    super::resilient::wait_for_contents(&out, "TOPSECRET");
    let term_env = fs::read_to_string(dir.path().join("term-env.txt")).unwrap_or_default();
    let term_cmd = fs::read_to_string(dir.path().join("term-cmd.txt")).unwrap_or_default();
    assert!(
        !term_env.contains("TOPSECRET"),
        "terminal env leaked: {term_env}"
    );
    assert!(
        !term_cmd.contains("TOPSECRET"),
        "terminal cmdline leaked: {term_cmd}"
    );
    for _ in 0..50 {
        if !script_path.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !script_path.exists(),
        "script should self-delete: {}",
        script_path.display()
    );
}

#[test]
fn two_concurrent_launches_get_different_scripts_and_passwords() {
    let dir = tempfile::tempdir().unwrap();
    let launcher_dir = secure_launcher_dir(dir.path(), "launcher");
    let term = dir.path().join("fake-terminal.sh");
    write_exec(&term, "#!/bin/sh\nfor last do true; done\nsh \"$last\"\n");
    let psql = dir.path().join("psql");
    write_exec(
        &psql,
        "#!/bin/sh\nprintf '%s' \"${PGPASSWORD:-}\" > \"$OUT\"\n",
    );

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
        psql.clone(),
        ClientKind::Psql.supported_engines(),
    );
    let terminal = TerminalChoice::new(&term, TerminalKind::GenericArgv);

    let mut plans = Vec::new();
    for i in 0..2 {
        let s = SecretString::from(format!("PW{i}"));
        let plan = plan_open(&client, &target, Some(&s), OsKind::Linux, &terminal).unwrap();
        plans.push((plan, format!("PW{i}")));
    }
    let n0 = plans[0].0.temp_file().unwrap().name_hint().to_owned();
    let n1 = plans[1].0.temp_file().unwrap().name_hint().to_owned();
    assert_ne!(n0, n1);

    for (i, (_plan, pw)) in plans.iter().enumerate() {
        let out = dir.path().join(format!("out{i}.txt"));
        let wrapper = dir.path().join(format!("w{i}.sh"));
        write_exec(
            &wrapper,
            &format!(
                "#!/bin/sh\nexport PATH=\"{}:$PATH\"\nexport OUT=\"{}\"\nexec \"{}\" \"$@\"\n",
                dir.path().display(),
                out.display(),
                term.display()
            ),
        );
        let terminal = TerminalChoice::new(&wrapper, TerminalKind::GenericArgv);
        let target = OpenTarget::new(
            EngineKey::Postgresql,
            "127.0.0.1",
            5432,
            "postgres",
            "postgres",
            "app",
        )
        .unwrap();
        let plan = plan_open(
            &client,
            &target,
            Some(&SecretString::from(pw.as_str())),
            OsKind::Linux,
            &terminal,
        )
        .unwrap();
        super::resilient::spawn_resilient(&plan, &SystemSpawner::with_launcher_dir(&launcher_dir))
            .unwrap();
        super::resilient::wait_for_contents(&out, pw);
    }
}

#[test]
#[ignore = "e2e: real ptyxis terminal - run with DBLOCAL_REAL_TERMINAL=1"]
fn real_ptyxis_receives_password_via_script() {
    if std::env::var("DBLOCAL_REAL_TERMINAL").as_deref() != Ok("1") {
        eprintln!("skipped: set DBLOCAL_REAL_TERMINAL=1");
        return;
    }
    let ptyxis = ["/usr/bin/ptyxis", "/usr/local/bin/ptyxis"]
        .into_iter()
        .map(std::path::PathBuf::from)
        .find(|p| p.exists())
        .or_else(|| {
            std::process::Command::new("which")
                .arg("ptyxis")
                .output()
                .ok()
                .and_then(|o| {
                    if !o.status.success() {
                        return None;
                    }
                    let s = String::from_utf8_lossy(&o.stdout).trim().to_owned();
                    if s.is_empty() {
                        None
                    } else {
                        Some(std::path::PathBuf::from(s))
                    }
                })
        })
        .expect("ptyxis not found");

    let dir = tempfile::tempdir().unwrap();
    let launcher_dir = secure_launcher_dir(dir.path(), "launcher");
    let out = dir.path().join("seen.txt");
    let psql = dir.path().join("psql");
    write_exec(
        &psql,
        &format!(
            "#!/bin/sh\nprintf '%s' \"${{PGPASSWORD:-}}\" > \"{}\"\n",
            out.display()
        ),
    );

    let target = OpenTarget::new(
        EngineKey::Postgresql,
        "127.0.0.1",
        5432,
        "postgres",
        "postgres",
        "app",
    )
    .unwrap();
    let client = DetectedClient::new(ClientKind::Psql, psql, ClientKind::Psql.supported_engines());
    let terminal = TerminalChoice::new(ptyxis, TerminalKind::Ptyxis);
    let plan = plan_open(
        &client,
        &target,
        Some(&SecretString::from("TOPSECRET")),
        OsKind::Linux,
        &terminal,
    )
    .unwrap();
    super::resilient::spawn_resilient(&plan, &SystemSpawner::with_launcher_dir(&launcher_dir))
        .unwrap();

    super::resilient::wait_for_contents(&out, "TOPSECRET");

    let leftover: Vec<_> = fs::read_dir(&launcher_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("dblocal-open-"))
        })
        .collect();
    assert!(
        leftover.is_empty(),
        "launcher scripts must be cleaned: {leftover:?}"
    );
}
