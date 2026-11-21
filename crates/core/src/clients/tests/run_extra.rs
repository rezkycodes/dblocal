use std::fs;
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

#[test]
fn run_surfaces_spawner_error() {
    let plan = LaunchPlan::new("true", vec![], vec![], None, None, None);
    let err = run(&plan, &FailingSpawner).unwrap_err();
    assert!(matches!(err, ClientError::LaunchFailed { .. }));
}

#[test]
fn system_spawner_runs_with_temp_file_and_env() {
    let dir = tempfile::tempdir().unwrap();
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
        format!("dblocal-open-test-{}.sh", std::process::id()),
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
    let res = run(&plan, &SystemSpawner);
    if let Err(e) = &res {
        eprintln!("ERR: {e:?}");
    }
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
