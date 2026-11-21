use std::fs;

use crate::clients::{LaunchPlan, SystemSpawner, run};

use super::resilient::spawn_resilient;

#[test]
fn real_spawn_runs_harmless_fake_program() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("fake-client.sh");
    let out = dir.path().join("out.txt");
    fs::write(&script, format!("#!/bin/sh\necho ok > {}\n", out.display())).expect("write");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let plan = LaunchPlan::new(script.clone(), vec![], vec![], None, None, None);
    spawn_resilient(&plan, &SystemSpawner::default()).expect("spawn");
    super::resilient::wait_for_contents(&out, "ok\n");
}

#[test]
fn missing_client_returns_typed_error_not_panic() {
    let plan = LaunchPlan::new("nope", vec![], vec![], None, None, None)
        .with_required_path("/tmp/dblocal-definitely-missing-client-bin");
    let err = run(&plan, &SystemSpawner::default()).unwrap_err();
    assert!(matches!(
        err,
        crate::clients::ClientError::ClientNotFound { .. }
    ));
}
