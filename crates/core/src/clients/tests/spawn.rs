use std::fs;
use std::time::Duration;

use crate::clients::{LaunchPlan, SystemSpawner, run};

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
    run(&plan, &SystemSpawner).expect("spawn");
    // wait briefly for script
    for _ in 0..50 {
        if out.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(fs::read_to_string(&out).expect("read"), "ok\n");
}

#[test]
fn missing_client_returns_typed_error_not_panic() {
    let plan = LaunchPlan::new("nope", vec![], vec![], None, None, None)
        .with_required_path("/tmp/dblocal-definitely-missing-client-bin");
    let err = run(&plan, &SystemSpawner).unwrap_err();
    assert!(matches!(err, crate::error::DblError::NotFound { .. }));
}
