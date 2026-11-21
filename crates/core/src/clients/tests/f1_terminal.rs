use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use crate::clients::{
    ClientKind, DetectedClient, OpenTarget, OsKind, SystemSpawner, TerminalChoice, TerminalKind,
    plan_open, run,
};
use crate::model::EngineKey;
use crate::secret::SecretString;

fn write_exec(path: &std::path::Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn fake_server_terminal_receives_password_via_script_not_env() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("seen.txt");
    let term = dir.path().join("fake-terminal.sh");
    let psql = dir.path().join("psql");
    // Fake terminal ignores its own env and just runs the last arg as: sh <script>
    write_exec(
        &term,
        r#"#!/bin/sh
# simulate single-instance server terminal: do not forward env; just run script
# write our own env and cmdline for assertion
env > "$FAKE_TERM_ENV_OUT"
tr '\0' ' ' < /proc/$$/cmdline > "$FAKE_TERM_CMD_OUT"
# last arg is script path
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
    let terminal = TerminalChoice::new(&term, TerminalKind::GenericArgv);
    let secret = SecretString::from("TOPSECRET");
    let plan = plan_open(&client, &target, Some(&secret), OsKind::Linux, &terminal).unwrap();

    // Run with env vars telling fake terminal where to write evidence
    let _cmd_plan = plan.clone();
    // We can't inject env into SystemSpawner; instead wrap by running a script that sets them.
    // Easier: create a wrapper terminal that sets evidence paths.
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
    let plan = plan_open(&client, &target, Some(&secret), OsKind::Linux, &terminal).unwrap();

    run(&plan, &SystemSpawner).unwrap();

    let script_name = plan.temp_file().unwrap().name_hint().to_owned();
    let xdg = std::env::var("XDG_RUNTIME_DIR").unwrap();
    let script_path = PathBuf::from(xdg).join("dblocal").join(&script_name);
    for _ in 0..100 {
        if out.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let seen = fs::read_to_string(&out).expect("seen");
    assert_eq!(seen, "TOPSECRET");
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
        run(&plan, &SystemSpawner).unwrap();
        for _ in 0..100 {
            if out.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(fs::read_to_string(out).unwrap(), *pw);
    }
}

#[test]
#[ignore = "e2e: real ptyxis terminal - run with DBLOCAL_REAL_TERMINAL=1"]
fn real_ptyxis_receives_password_via_script() {
    if std::env::var("DBLOCAL_REAL_TERMINAL").as_deref() != Ok("1") {
        eprintln!("skipped: set DBLOCAL_REAL_TERMINAL=1");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("seen.txt");
    let psql = dir.path().join("psql");
    write_exec(
        &psql,
        &format!(
            "#!/bin/sh\nprintf '%s' \"${{PGPASSWORD:-}}\" > \"{}\"\n",
            out.display()
        ),
    );
    // wrapper to put psql on PATH
    let wrapper_term = dir.path().join("term-wrap.sh");
    write_exec(
        &wrapper_term,
        &format!(
            "#!/bin/sh\nexport PATH=\"{}:$PATH\"\nexec ptyxis --new-window -- \"$@\"\n",
            dir.path().display()
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
    let terminal = TerminalChoice::new(&wrapper_term, TerminalKind::Ptyxis);
    let plan = plan_open(
        &client,
        &target,
        Some(&SecretString::from("TOPSECRET")),
        OsKind::Linux,
        &terminal,
    )
    .unwrap();
    run(&plan, &SystemSpawner).unwrap();
    for _ in 0..200 {
        if out.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(fs::read_to_string(out).unwrap(), "TOPSECRET");
}
