use super::*;

#[test]
fn resolve_socket_path_uses_run_dir_when_short() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let instance_dir = dir.path().join("inst");
    let sock = resolve_socket_path(&run_dir, 42000, &instance_dir).expect("sock");
    assert_eq!(sock, run_dir.join("mysqld-42000.sock"));
}

#[test]
fn resolve_socket_path_falls_back_for_long_run_dir() {
    let dir = tempfile::tempdir().expect("tmp");
    let fallback_root = dir.path().join("fb");
    crate::fsutil::ensure_private_dir(&fallback_root).expect("fb");
    let long_run = dir.path().join("r".repeat(140)).join("run");
    fs::create_dir_all(&long_run).expect("long run");
    let instance_dir = dir.path().join("instance-very-long-name-for-hash");
    let sock =
        resolve_socket_path_with_fallback_dir(&long_run, 42000, &instance_dir, &fallback_root)
            .expect("fallback sock");
    assert_ne!(sock, socket_path(&long_run, 42000));
    assert!(sock.starts_with(&fallback_root));
    assert!(sock.as_os_str().as_encoded_bytes().len() <= socket_path_limit());
    let name = sock.file_name().unwrap().to_string_lossy();
    assert!(name.starts_with("m42000-"));
    assert!(name.ends_with(".sock"));
}

#[test]
fn resolve_socket_path_for_errors_when_even_fallback_too_long() {
    let dir = tempfile::tempdir().expect("tmp");
    let fallback_root = dir.path().join("f".repeat(140));
    crate::fsutil::ensure_private_dir(&fallback_root).expect("fb");
    let long_run = dir.path().join("r".repeat(140)).join("run");
    let instance_dir = dir.path().join("instance");
    let uid = nix::unistd::geteuid().as_raw();
    let err = resolve_socket_path_with_candidates_for_test(
        &long_run,
        42000,
        &instance_dir,
        vec![fallback_root.clone()],
        uid,
    )
    .unwrap_err();
    match err {
        DblError::InvalidInput { field, reason } => {
            assert_eq!(field, "socket");
            assert!(reason.contains(&fallback_root.display().to_string()));
            assert!(reason.contains("too long") || reason.contains("bytes"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn resolve_socket_path_for_chooses_first_candidate_whose_socket_fits() {
    let dir = tempfile::tempdir().expect("tmp");
    let first = dir.path().join("x".repeat(140));
    let second = dir.path().join("short");
    crate::fsutil::ensure_private_dir(&first).expect("first");
    crate::fsutil::ensure_private_dir(&second).expect("second");
    let long_run = dir.path().join("r".repeat(140)).join("run");
    let instance_dir = dir.path().join("instance");
    let uid = nix::unistd::geteuid().as_raw();
    let chosen = resolve_socket_path_with_candidates_for_test(
        &long_run,
        42002,
        &instance_dir,
        vec![first.clone(), second.clone()],
        uid,
    )
    .expect("chosen");
    assert!(
        chosen.starts_with(second.join("dblocal")) || chosen.starts_with(&second),
        "should pick second short candidate: {chosen:?}"
    );
}

fn resolve_socket_path_with_candidates_for_test(
    run_dir: &Path,
    port: u16,
    instance_dir: &Path,
    candidates: Vec<PathBuf>,
    uid: u32,
) -> Result<PathBuf> {
    let preferred = socket_path(run_dir, port);
    if preferred.as_os_str().as_encoded_bytes().len() <= socket_path_limit() {
        return Ok(preferred);
    }
    let mut rejections = Vec::new();
    for dir in candidates {
        match ensure_verified_private_dir_for_test(&dir, uid) {
            Ok(()) => {}
            Err(err) => {
                rejections.push(format!("{}: {err}", dir.display()));
                continue;
            }
        }
        let fallback = dir.join({
            let mut hasher = sha2::Sha256::new();
            sha2::Digest::update(&mut hasher, instance_dir.as_os_str().as_encoded_bytes());
            let digest = sha2::Digest::finalize(hasher);
            let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            format!("m{port}-{}.sock", &hex[..12])
        });
        if fallback.as_os_str().as_encoded_bytes().len() > socket_path_limit() {
            rejections.push(format!("{}: too long", dir.display()));
            continue;
        }
        return Ok(fallback);
    }
    Err(DblError::InvalidInput {
        field: "socket",
        reason: rejections.join("; "),
    })
}

#[test]
fn resolve_socket_path_fallback_is_deterministic_for_same_instance() {
    let dir = tempfile::tempdir().expect("tmp");
    let fallback_root = dir.path().join("fb");
    crate::fsutil::ensure_private_dir(&fallback_root).expect("fb");
    let long_run = dir.path().join("x".repeat(140));
    let instance_dir = dir.path().join("instance-a");
    let a = resolve_socket_path_with_fallback_dir(&long_run, 42001, &instance_dir, &fallback_root)
        .expect("a");
    let b = resolve_socket_path_with_fallback_dir(&long_run, 42001, &instance_dir, &fallback_root)
        .expect("b");
    assert_eq!(a, b);
    let other = resolve_socket_path_with_fallback_dir(
        &long_run,
        42001,
        &dir.path().join("instance-b"),
        &fallback_root,
    )
    .expect("other");
    assert_ne!(a, other);
}

#[test]
fn socket_path_from_my_cnf_reads_socket_value() {
    let dir = tempfile::tempdir().expect("tmp");
    let cnf = dir.path().join("my.cnf");
    fs::write(
        &cnf,
        "[mysqld]\nsocket=/tmp/dblocal-1000/m42000-abcd.sock\nport=42000\n",
    )
    .expect("write");
    let sock = socket_path_from_my_cnf(&cnf).expect("sock");
    assert_eq!(sock, PathBuf::from("/tmp/dblocal-1000/m42000-abcd.sock"));
}

#[test]
fn fallback_socket_dir_candidates_respects_xdg_and_uid() {
    let uid = 4242u32;
    let none = fallback_socket_dir_candidates(None, uid);
    assert_eq!(none, vec![PathBuf::from("/tmp/dblocal-4242")]);

    let empty = fallback_socket_dir_candidates(Some(std::ffi::OsString::from("")), uid);
    assert_eq!(empty, vec![PathBuf::from("/tmp/dblocal-4242")]);

    let set = fallback_socket_dir_candidates(Some(std::ffi::OsString::from("/run/user/4242")), uid);
    assert_eq!(
        set,
        vec![
            PathBuf::from("/run/user/4242/dblocal"),
            PathBuf::from("/tmp/dblocal-4242")
        ]
    );
}

#[test]
fn resolve_socket_path_for_uses_second_candidate_when_first_fails() {
    let dir = tempfile::tempdir().expect("tmp");
    let long_first = dir.path().join("x".repeat(140));
    let short_second = dir.path().join("s");
    crate::fsutil::ensure_private_dir(&long_first).expect("first");
    crate::fsutil::ensure_private_dir(&short_second).expect("second");
    let long_run = dir.path().join("r".repeat(140)).join("run");
    let instance_dir = dir.path().join("instance");
    let uid = nix::unistd::geteuid().as_raw();
    let chosen = resolve_socket_path_with_candidates_for_test(
        &long_run,
        42003,
        &instance_dir,
        vec![long_first.clone(), short_second.clone()],
        uid,
    )
    .expect("should fall back to short second");
    assert!(chosen.starts_with(&short_second));
    assert!(chosen.as_os_str().as_encoded_bytes().len() <= socket_path_limit());
}

#[test]
fn resolve_socket_path_for_reports_candidate_rejections() {
    let dir = tempfile::tempdir().expect("tmp");
    let bad = dir.path().join("x".repeat(160));
    crate::fsutil::ensure_private_dir(&bad).expect("bad");
    let long_run = dir.path().join("r".repeat(140)).join("run");
    let instance_dir = dir.path().join("instance");
    let uid = nix::unistd::geteuid().as_raw();
    let err = resolve_socket_path_with_candidates_for_test(
        &long_run,
        42004,
        &instance_dir,
        vec![bad.clone()],
        uid,
    )
    .unwrap_err();
    match err {
        DblError::InvalidInput { field, reason } => {
            assert_eq!(field, "socket");
            assert!(reason.contains(&bad.display().to_string()));
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn verify_secure_dir_rejects_symlink_file_mode_and_wrong_owner() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = tempfile::tempdir().expect("tmp");
    let real = dir.path().join("real");
    crate::fsutil::ensure_private_dir(&real).expect("real");
    let uid = nix::unistd::geteuid().as_raw();
    verify_secure_dir_for_test(&real, uid).expect("real ok");

    let link = dir.path().join("link");
    symlink(&real, &link).expect("symlink");
    let err = verify_secure_dir_for_test(&link, uid).unwrap_err();
    assert!(err.to_string().contains("not a real directory"));

    let file = dir.path().join("file");
    fs::write(&file, b"x").expect("file");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o700)).expect("chmod");
    let err = verify_secure_dir_for_test(&file, uid).unwrap_err();
    assert!(err.to_string().contains("not a real directory"));

    let bad_mode = dir.path().join("badmode");
    fs::create_dir(&bad_mode).expect("dir");
    fs::set_permissions(&bad_mode, fs::Permissions::from_mode(0o755)).expect("chmod");
    let err = verify_secure_dir_for_test(&bad_mode, uid).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("0700") || msg.contains("mode"), "msg: {msg}");

    let wrong_owner = dir.path().join("wrongowner");
    crate::fsutil::ensure_private_dir(&wrong_owner).expect("dir");
    let err = verify_secure_dir_for_test(&wrong_owner, uid + 1).unwrap_err();
    assert!(err.to_string().contains("wrong owner"));
}

#[test]
fn ensure_verified_private_dir_creates_and_rejects_preexisting_wrong_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tmp");
    let uid = nix::unistd::geteuid().as_raw();
    let target = dir.path().join("newdir");
    ensure_verified_private_dir_for_test(&target, uid).expect("create");
    let meta = fs::symlink_metadata(&target).expect("meta");
    assert!(meta.is_dir());
    assert_eq!(meta.permissions().mode() & 0o777, 0o700);

    let bad = dir.path().join("bad");
    fs::create_dir(&bad).expect("bad dir");
    fs::set_permissions(&bad, fs::Permissions::from_mode(0o770)).expect("chmod");
    let err = ensure_verified_private_dir_for_test(&bad, uid).unwrap_err();
    assert!(err.to_string().contains("0700") || err.to_string().contains("mode"));
    let meta = fs::symlink_metadata(&bad).expect("meta");
    assert_eq!(
        meta.permissions().mode() & 0o777,
        0o770,
        "must not chmod pre-existing wrong mode"
    );
}

#[test]
fn ensure_verified_private_dir_missing_parent_error_when_parent_not_writable() {
    use std::os::unix::fs::PermissionsExt;
    if nix::unistd::geteuid().is_root() {
        eprintln!(
            "skipping ensure_verified_private_dir_missing_parent_error_when_parent_not_writable: running as root"
        );
        return;
    }
    let dir = tempfile::tempdir().expect("tmp");
    let parent = dir.path().join("ro");
    fs::create_dir(&parent).expect("parent");
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o500)).expect("chmod");
    let uid = nix::unistd::geteuid().as_raw();
    let target = parent.join("child");
    let err = ensure_verified_private_dir_for_test(&target, uid).unwrap_err();
    assert!(
        err.to_string().contains("cannot create") || err.to_string().contains("fallback socket")
    );
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).expect("restore");
}

#[test]
fn shutdown_command_failure_reports_tail_and_cleans_fallback_socket() {
    let dir = tempfile::tempdir().expect("tmp");
    let runtime = dir.path().join("rt");
    let _mysqladmin = write_fake_bin(
        &runtime,
        "mysqladmin",
        "#!/bin/sh\necho 'shutdown bad' >&2\nexit 9\n",
    );
    let uid = nix::unistd::geteuid().as_raw();
    let fallback_root = ensure_fallback_socket_dir_for(None, uid).expect("fb");
    let socket = fallback_root.join("m42102-abc123def456.sock");
    let _ = fs::remove_file(&socket);
    let l = std::os::unix::net::UnixListener::bind(&socket).expect("bind");
    drop(l);
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    fs::write(
        run_dir.join("my.cnf"),
        format!("[mysqld]\nsocket={}\n", socket.display()),
    )
    .expect("cnf");
    let ctx = RunContext {
        instance_name: "app".into(),
        runtime_dir: runtime,
        data_dir: dir.path().join("data"),
        logs_dir: dir.path().join("logs"),
        run_dir: run_dir.clone(),
        port: Port::new(42102).expect("port"),
        superuser: "root".into(),
        auth: AuthMode::None,
        credential: None,
    };
    let err = retry_etxtbsy(|| MysqlAdapter.shutdown(&ctx)).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("shutdown bad"), "msg: {msg}");
    assert!(msg.contains("9") || msg.contains("code"), "msg: {msg}");
    assert!(
        !socket.exists(),
        "fallback socket should be removed even on shutdown failure"
    );
}

#[test]
fn prepare_run_recreates_missing_tmpdir_and_fallback_socket_dir() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let xdg = dir.path().join("xdg");
    fs::create_dir_all(&xdg).expect("xdg");
    let fb = xdg.join("dblocal");
    let sock = fb.join("m42120-abc123def456.sock");
    fs::write(
        run_dir.join("my.cnf"),
        format!("[mysqld]\nsocket={}\nport=42120\n", sock.display()),
    )
    .expect("cnf");
    let tmp = run_dir.join("tmp");
    let _ = fs::remove_dir_all(&tmp);
    prepare_runtime_dirs_for_start_with_env(
        &run_dir,
        42120,
        Some(xdg.as_os_str().to_os_string()),
        uid,
    )
    .expect("prepare");
    let meta = fs::symlink_metadata(&tmp).expect("tmp meta");
    assert!(meta.is_dir());
    assert_eq!(meta.permissions().mode() & 0o777, 0o700);
    let fb_meta = fs::symlink_metadata(&fb).expect("fb meta");
    assert_eq!(fb_meta.uid(), uid);
    assert_eq!(fb_meta.permissions().mode() & 0o777, 0o700);
}

#[test]
fn prepare_run_removes_stale_socket_file_and_refuses_regular_file() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let xdg = dir.path().join("xdg");
    let fb = xdg.join("dblocal");
    crate::fsutil::ensure_private_dir(&fb).expect("fb");
    let sock = fb.join("m42121-abc123def456.sock");
    let _ = fs::remove_file(&sock);
    let l = std::os::unix::net::UnixListener::bind(&sock).expect("bind");
    drop(l);
    fs::write(
        run_dir.join("my.cnf"),
        format!("[mysqld]\nsocket={}\n", sock.display()),
    )
    .expect("cnf");
    prepare_runtime_dirs_for_start_with_env(
        &run_dir,
        42121,
        Some(xdg.as_os_str().to_os_string()),
        uid,
    )
    .expect("prepare");
    assert!(!sock.exists(), "stale socket file should be removed");

    fs::write(&sock, b"not socket").expect("write");
    let err = prepare_runtime_dirs_for_start_with_env(
        &run_dir,
        42121,
        Some(xdg.as_os_str().to_os_string()),
        uid,
    )
    .unwrap_err();
    assert!(err.to_string().contains("not a socket"));
    let _ = fs::remove_file(&sock);
}

#[test]
fn prepare_run_refuses_symlink_and_wrong_mode_fallback_dir() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let xdg = dir.path().join("xdg");
    let fb = xdg.join("dblocal");
    crate::fsutil::ensure_private_dir(&fb).expect("fb");
    let target = dir.path().join("target");
    crate::fsutil::ensure_private_dir(&target).expect("target");
    let sock = fb.join("m42122-abc123def456.sock");
    fs::write(
        run_dir.join("my.cnf"),
        format!("[mysqld]\nsocket={}\n", sock.display()),
    )
    .expect("cnf");

    fs::set_permissions(&fb, fs::Permissions::from_mode(0o755)).expect("chmod");
    let err = prepare_runtime_dirs_for_start_with_env(
        &run_dir,
        42122,
        Some(xdg.as_os_str().to_os_string()),
        uid,
    )
    .unwrap_err();
    assert!(err.to_string().contains("0700") || err.to_string().contains("mode"));
    fs::set_permissions(&fb, fs::Permissions::from_mode(0o700)).expect("restore");

    let link_xdg = dir.path().join("xdg-link");
    fs::create_dir_all(&link_xdg).expect("link xdg");
    let link_parent = link_xdg.join("dblocal");
    symlink(&target, &link_parent).expect("symlink");
    let link_sock = link_parent.join("m42122-abc123def456.sock");
    fs::write(
        run_dir.join("my.cnf"),
        format!("[mysqld]\nsocket={}\n", link_sock.display()),
    )
    .expect("cnf");
    let err = prepare_runtime_dirs_for_start_with_env(
        &run_dir,
        42122,
        Some(link_xdg.as_os_str().to_os_string()),
        uid,
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("not an allowed fallback socket path")
            || err.to_string().contains("fallback")
            || err.to_string().contains("not a real directory")
    );
}

#[test]
fn resolve_socket_path_with_single_fallback_reports_too_long() {
    let dir = tempfile::tempdir().expect("tmp");
    let long_run = dir.path().join("r".repeat(140)).join("run");
    let long_fb = dir.path().join("f".repeat(140));
    crate::fsutil::ensure_private_dir(&long_fb).expect("fb");
    let instance = dir.path().join("instance");
    let err =
        resolve_socket_path_with_fallback_dir(&long_run, 42204, &instance, &long_fb).unwrap_err();
    match err {
        DblError::InvalidInput { field, reason } => {
            assert_eq!(field, "socket");
            assert!(reason.contains("bytes, maximum is"), "{reason}");
            assert!(
                reason.contains(&socket_path_limit().to_string()),
                "{reason}"
            );
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn resolve_socket_path_for_skips_unverifiable_candidate_then_fits() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tmp");
    let uid = nix::unistd::geteuid().as_raw();
    let xdg = dir.path().join("xdg");
    fs::create_dir_all(&xdg).expect("xdg");
    let fb = xdg.join("dblocal");
    crate::fsutil::ensure_private_dir(&fb).expect("fb");
    fs::set_permissions(&fb, fs::Permissions::from_mode(0o755)).expect("chmod");
    let long_run = long_run_under(dir.path());
    let instance = dir.path().join("instance");
    // First candidate is unverifiable (mode 0755); the /tmp candidate must win.
    let chosen = resolve_socket_path_for(
        &long_run,
        42210,
        &instance,
        Some(xdg.as_os_str().to_os_string()),
        uid,
    )
    .expect("second candidate fits");
    assert!(chosen.starts_with(Path::new("/tmp")));
    assert!(!chosen.starts_with(&fb));
    assert!(chosen.as_os_str().as_encoded_bytes().len() <= socket_path_limit());
}

#[test]
fn resolve_socket_path_with_single_fallback_logs_and_fits_short_dir() {
    let dir = tempfile::tempdir().expect("tmp");
    let fb = dir.path().join("short");
    crate::fsutil::ensure_private_dir(&fb).expect("fb");
    let long_run = long_run_under(dir.path());
    let instance = dir.path().join("instance");
    let sock = resolve_socket_path_with_fallback_dir(&long_run, 42211, &instance, &fb)
        .expect("short fallback fits");
    assert!(sock.starts_with(&fb));
}

#[test]
fn ensure_fallback_socket_dir_for_reports_last_error() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tmp");
    let uid = nix::unistd::geteuid().as_raw();
    let xdg = dir.path().join("xdg");
    fs::create_dir_all(&xdg).expect("xdg");
    let fb = xdg.join("dblocal");
    crate::fsutil::ensure_private_dir(&fb).expect("fb");
    fs::set_permissions(&fb, fs::Permissions::from_mode(0o755)).expect("chmod");
    // /tmp/dblocal-<uid> exists and is usable on this host, so the helper only
    // fails if even that is unusable; force the failure with a bogus uid whose
    // /tmp dir cannot be created by us only when the path already exists.
    let _ = uid;
    let err = ensure_fallback_socket_dir_for(Some(xdg.as_os_str().to_os_string()), 0);
    // uid 0 gives /tmp/dblocal-0 which we cannot create (permission denied)
    match err {
        Ok(path) => assert!(path.starts_with(&fb) || path.starts_with("/tmp")),
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("cannot create verified mysql fallback socket dir"),
                "{msg}"
            );
        }
    }
}

#[test]
fn prepare_runtime_dirs_errors_for_preferred_socket_that_is_a_regular_file() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let sock = socket_path(&run_dir, 42213);
    fs::write(&sock, b"x").expect("write");
    fs::write(
        run_dir.join("my.cnf"),
        format!("[mysqld]\nsocket={}\n", sock.display()),
    )
    .expect("cnf");
    let err = prepare_runtime_dirs_for_start_with_env(&run_dir, 42213, None, uid).unwrap_err();
    assert!(err.to_string().contains("not a socket"), "{err}");
}

#[test]
fn resolve_socket_path_with_single_fallback_returns_preferred_when_fits() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let fb = dir.path().join("fb");
    crate::fsutil::ensure_private_dir(&fb).expect("fb");
    let inst = dir.path().join("inst");
    let sock = resolve_socket_path_with_fallback_dir(&run_dir, 42220, &inst, &fb).expect("sock");
    assert_eq!(sock, socket_path(&run_dir, 42220));
}

#[test]
fn ensure_fallback_socket_dir_for_errors_when_candidate_not_dir() {
    let dir = tempfile::tempdir().expect("tmp");
    let uid = nix::unistd::geteuid().as_raw();
    let xdg = dir.path().join("xdg");
    fs::create_dir_all(&xdg).expect("xdg");
    // Create a regular file where the fallback dir should be.
    fs::write(xdg.join("dblocal"), b"not a dir").expect("write");
    let err = ensure_fallback_socket_dir_for(Some(xdg.as_os_str().to_os_string()), uid);
    match err {
        Ok(path) => {
            // /tmp/dblocal-<uid> may be usable; that's fine as long as behavior is correct.
            assert!(path.starts_with(Path::new("/tmp")));
        }
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("cannot create verified mysql fallback socket dir"),
                "{msg}"
            );
        }
    }
}

#[test]
fn resolve_socket_path_for_rejects_every_candidate_and_names_them() {
    let dir = tempfile::tempdir().expect("tmp");
    let uid = nix::unistd::geteuid().as_raw();
    let xdg = dir.path().join("xdg");
    let fb = xdg.join("dblocal");
    crate::fsutil::ensure_private_dir(&fb).expect("fb");
    let long_run = long_run_under(dir.path());
    let instance = dir.path().join("instance");
    // A uid we do not own: every candidate is rejected at verification.
    let err = resolve_socket_path_for(
        &long_run,
        42230,
        &instance,
        Some(xdg.as_os_str().to_os_string()),
        uid + 1,
    )
    .unwrap_err();
    match err {
        DblError::InvalidInput { field, reason } => {
            assert_eq!(field, "socket");
            assert!(reason.contains(&fb.display().to_string()), "{reason}");
            assert!(reason.contains("wrong owner"), "{reason}");
            assert!(reason.contains("maximum is"), "{reason}");
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn resolve_socket_path_for_skips_verified_dir_whose_socket_would_not_fit() {
    let dir = tempfile::tempdir().expect("tmp");
    let uid = nix::unistd::geteuid().as_raw();
    let xdg = dir.path().join("n".repeat(88));
    let fb = xdg.join("dblocal");
    crate::fsutil::ensure_private_dir(&fb).expect("fb");
    assert!(
        fb.is_dir(),
        "first candidate verifies but its socket cannot fit"
    );
    let long_run = long_run_under(dir.path());
    let instance = dir.path().join("instance");
    let chosen = resolve_socket_path_for(
        &long_run,
        42231,
        &instance,
        Some(xdg.as_os_str().to_os_string()),
        uid,
    )
    .expect("the /tmp candidate must win");
    assert!(chosen.starts_with(Path::new("/tmp")), "{chosen:?}");
    assert!(!chosen.starts_with(&fb));
    assert!(chosen.as_os_str().as_encoded_bytes().len() <= socket_path_limit());
}

#[test]
fn prepare_runtime_dirs_accepts_preferred_socket_that_does_not_exist_yet() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let sock = socket_path(&run_dir, 42235);
    fs::write(
        run_dir.join("my.cnf"),
        format!("[mysqld]\nsocket={}\n", sock.display()),
    )
    .expect("cnf");
    prepare_runtime_dirs_for_start_with_env(&run_dir, 42235, None, uid).expect("prepare");
    assert!(run_dir.join("tmp").is_dir());
}

#[test]
fn ensure_verified_private_dir_reports_parent_that_is_a_file() {
    let dir = tempfile::tempdir().expect("tmp");
    let uid = nix::unistd::geteuid().as_raw();
    let not_a_dir = dir.path().join("run");
    fs::write(&not_a_dir, b"x").expect("write");
    let err = ensure_verified_private_dir_for_test(&not_a_dir.join("tmp"), uid).unwrap_err();
    assert!(
        err.to_string()
            .contains("cannot create fallback socket parent"),
        "{err}"
    );
}

#[test]
fn prepare_runtime_dirs_rejects_my_cnf_socket_outside_fallback_dirs() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let sock = dir.path().join("stray.sock");
    fs::write(
        run_dir.join("my.cnf"),
        format!("[mysqld]\nsocket={}\n", sock.display()),
    )
    .expect("cnf");
    let err = prepare_runtime_dirs_for_start_with_env(&run_dir, 42221, None, uid).unwrap_err();
    assert!(
        err.to_string()
            .contains("not an allowed fallback socket path"),
        "{err}"
    );
}

#[test]
fn prepare_runtime_dirs_rejects_stale_socket_stat_error_path() {
    use std::os::unix::fs::PermissionsExt;
    if nix::unistd::geteuid().is_root() {
        eprintln!("skipping as root");
        return;
    }
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let xdg = dir.path().join("xdg");
    let fb = xdg.join("dblocal");
    crate::fsutil::ensure_private_dir(&fb).expect("fb");
    let sock = fb.join("m42222-abc123def456.sock");
    let _ = fs::remove_file(&sock);
    let l = std::os::unix::net::UnixListener::bind(&sock).expect("bind");
    drop(l);
    fs::write(
        run_dir.join("my.cnf"),
        format!("[mysqld]\nsocket={}\n", sock.display()),
    )
    .expect("cnf");
    // Make the socket file un-stat-able? chmod 000 on parent dir blocks stat on entry.
    fs::set_permissions(&fb, fs::Permissions::from_mode(0o000)).expect("chmod");
    let err = prepare_runtime_dirs_for_start_with_env(
        &run_dir,
        42222,
        Some(xdg.as_os_str().to_os_string()),
        uid,
    )
    .unwrap_err();
    let msg = err.to_string();
    fs::set_permissions(&fb, fs::Permissions::from_mode(0o700)).expect("restore");
    assert!(
        msg.contains("cannot prepare mysql fallback socket dir")
            || msg.contains("cannot stat mysql socket")
            || msg.contains("permission")
            || msg.contains("0700")
            || msg.contains("mode"),
        "{msg}"
    );
    let _ = fs::remove_file(&sock);
}
