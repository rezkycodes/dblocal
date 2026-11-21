use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use crate::clients::ClientError;
use crate::clients::{LaunchEnv, TempFile};
use crate::clients::{LaunchPlan, Spawner, SystemSpawner, run};
use crate::secret::SecretString;

struct FailingSpawner;
impl Spawner for FailingSpawner {
    fn spawn(&self, _plan: &LaunchPlan) -> crate::clients::ClientResult<()> {
        Err(ClientError::LaunchFailed {
            reason: "boom".to_owned(),
        })
    }
}

fn secure_dir(path: &std::path::Path) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    #[cfg(not(unix))]
    fs::create_dir(path).unwrap();
    path.to_path_buf()
}

fn launcher_scripts(dir: &std::path::Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("dblocal-open-"))
        })
        .collect()
}

#[test]
fn run_surfaces_spawner_error() {
    let plan = LaunchPlan::new("true", vec![], vec![], None, None, None);
    let err = run(&plan, &FailingSpawner).unwrap_err();
    assert!(matches!(err, ClientError::LaunchFailed { .. }));
}

#[test]
fn system_spawner_runs_with_temp_file_and_env() {
    let dir = tempfile::tempdir().unwrap();
    let launcher_dir = secure_dir(&dir.path().join("launcher"));
    let out = dir.path().join("out.txt");
    let script = dir.path().join("fake.sh");
    fs::write(
        &script,
        format!("#!/bin/sh\necho $MY_SECRET > {}\n", out.display()),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let temp = TempFile::new(
        "dblocal-open-test-run-extra.sh".to_owned(),
        SecretString::from("#!/bin/sh\nexit 0\n"),
        0o600,
    );
    let plan = LaunchPlan::new(
        script,
        vec![],
        vec![LaunchEnv::new("MY_SECRET", SecretString::from("sekret"))],
        None,
        Some(temp),
        None,
    );
    let res = run(&plan, &SystemSpawner::with_launcher_dir(&launcher_dir));
    res.unwrap();
    for _ in 0..50 {
        if out.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let s = fs::read_to_string(&out).unwrap();
    assert!(s.contains("sekret"));
}

#[test]
fn spawn_of_missing_program_removes_the_script_and_is_typed() {
    let dir = tempfile::tempdir().unwrap();
    let launcher_dir = secure_dir(&dir.path().join("launcher"));
    let temp = TempFile::new(
        "dblocal-open-spawn-fail.sh".to_owned(),
        SecretString::from("#!/bin/sh\nexit 0\n"),
        0o600,
    );
    let plan = LaunchPlan::new(
        dir.path().join("no-such-binary"),
        vec![],
        vec![],
        None,
        Some(temp),
        None,
    );
    let err = run(&plan, &SystemSpawner::with_launcher_dir(&launcher_dir)).unwrap_err();
    assert!(matches!(err, ClientError::ClientNotFound { .. }), "{err:?}");
    assert!(
        launcher_scripts(&launcher_dir).is_empty(),
        "script must be removed when spawn fails"
    );
}

#[test]
fn default_launcher_dir_is_used_and_cleans_up() {
    // The only test that exercises the real default launcher directory. It
    // removes every file it creates and the directory if it was absent before.
    let os = crate::clients::OsKind::current();
    let dir = super::super::terminal::launcher_dir(os).expect("launcher dir");
    let existed_before = dir.exists();

    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("ran.txt");
    let temp = TempFile::new(
        "dblocal-open-default-dir-test.sh".to_owned(),
        SecretString::from(format!(
            "#!/bin/sh\nexit 0 >/dev/null; : > {}\n",
            target.display()
        )),
        0o600,
    );
    let written = super::super::terminal::materialize_launcher(&temp, os).expect("materialize");
    assert!(written.starts_with(&dir));
    let _ = fs::remove_file(&written);

    if !existed_before {
        let _ = fs::remove_dir(&dir);
    }
    assert!(
        !dir.join(temp.name_hint()).exists(),
        "default-dir test must remove its script"
    );
}

#[test]
fn script_placeholder_is_substituted_from_the_injected_dir() {
    let dir = tempfile::tempdir().unwrap();
    let launcher_dir = secure_dir(&dir.path().join("launcher"));
    let term = dir.path().join("term.sh");
    fs::write(&term, "#!/bin/sh\nfor last do true; done\nsh \"$last\"\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&term, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let out = dir.path().join("ran.txt");
    let temp = TempFile::new(
        "dblocal-open-sub.sh".to_owned(),
        SecretString::from(format!("#!/bin/sh\necho yes > {}\n", out.display())),
        0o600,
    );
    let plan = LaunchPlan::new(
        term,
        vec!["{script}".to_owned()],
        vec![],
        None,
        Some(temp),
        None,
    );
    run(&plan, &SystemSpawner::with_launcher_dir(&launcher_dir)).unwrap();
    for _ in 0..100 {
        if out.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(fs::read_to_string(&out).unwrap().trim(), "yes");
}
