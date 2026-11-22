use super::*;

#[test]
fn prepare_runtime_dirs_creates_missing_tmpdir_and_skips_without_my_cnf() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    prepare_runtime_dirs_for_start_with_env(&run_dir, 42212, None, uid).expect("prepare");
    let tmp = run_dir.join("tmp");
    let meta = fs::symlink_metadata(&tmp).expect("meta");
    assert_eq!(meta.permissions().mode() & 0o777, 0o700);
}

#[test]
fn prepare_runtime_dirs_errors_when_tmpdir_cannot_be_created() {
    use std::os::unix::fs::PermissionsExt;
    if nix::unistd::geteuid().is_root() {
        eprintln!("skipping prepare_runtime_dirs_errors_when_tmpdir_cannot_be_created as root");
        return;
    }
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    // A regular file where tmp must be created makes ensure_verified_private_dir fail.
    fs::write(run_dir.join("tmp"), b"x").expect("write");
    let err = prepare_runtime_dirs_for_start_with_env(&run_dir, 42214, None, uid).unwrap_err();
    assert!(
        err.to_string().contains("cannot prepare mysql tmpdir"),
        "{err}"
    );
    let _ = fs::remove_file(run_dir.join("tmp"));

    // Parent not writable -> cannot create tmpdir.
    let dir2 = tempfile::tempdir().expect("tmp2");
    let ro_run = dir2.path().join("run");
    fs::create_dir_all(&ro_run).expect("run");
    fs::set_permissions(&ro_run, fs::Permissions::from_mode(0o500)).expect("chmod");
    let err = prepare_runtime_dirs_for_start_with_env(&ro_run, 42214, None, uid).unwrap_err();
    fs::set_permissions(&ro_run, fs::Permissions::from_mode(0o700)).expect("restore");
    assert!(
        err.to_string().contains("cannot prepare mysql tmpdir"),
        "{err}"
    );
}

#[test]
fn public_prepare_run_creates_the_private_tmpdir() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let ctx = RunContext {
        instance_name: "app".into(),
        runtime_dir: dir.path().join("rt"),
        data_dir: dir.path().join("data"),
        logs_dir: dir.path().join("logs"),
        run_dir: run_dir.clone(),
        port: Port::new(42250).expect("port"),
        superuser: "root".into(),
        auth: AuthMode::None,
        credential: None,
    };
    prepare_run(&ctx).expect("prepare_run");
    let meta = fs::symlink_metadata(run_dir.join("tmp")).expect("tmp meta");
    assert_eq!(meta.permissions().mode() & 0o777, 0o700);
}
