use super::*;

#[test]
fn cleanup_fallback_socket_removes_only_fallback() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let preferred = socket_path(&run_dir, 42000);
    fs::write(&preferred, b"").expect("preferred");
    cleanup_fallback_socket(&preferred, &run_dir, 42000);
    assert!(preferred.exists(), "preferred socket must not be removed");

    let uid = nix::unistd::geteuid().as_raw();
    let fallback_root =
        ensure_fallback_socket_dir_for(None, uid).expect("fallback dir for cleanup test");
    let fallback = fallback_root.join("m42000-abc123def456.sock");
    let _ = fs::remove_file(&fallback);
    let listener = std::os::unix::net::UnixListener::bind(&fallback).expect("bind fallback sock");
    assert!(fallback.exists());
    drop(listener);
    cleanup_fallback_socket(&fallback, &run_dir, 42000);
    assert!(!fallback.exists(), "fallback socket should be removed");
}

#[test]
fn cleanup_fallback_socket_refuses_regular_file_with_valid_name() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let fallback_root = ensure_fallback_socket_dir_for(None, uid).expect("fb");
    let fallback = fallback_root.join("m42010-abc123def456.sock");
    let _ = fs::remove_file(&fallback);
    fs::write(&fallback, b"not a socket").expect("write");
    cleanup_fallback_socket_with_candidates(&fallback, &run_dir, 42010, None, uid);
    assert!(fallback.exists(), "regular file must not be deleted");
    let _ = fs::remove_file(&fallback);
}

#[test]
fn cleanup_fallback_socket_refuses_foreign_name_and_other_port() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let fallback_root = ensure_fallback_socket_dir_for(None, uid).expect("fb");

    let foreign = fallback_root.join("not-a-fallback.sock");
    let _ = fs::remove_file(&foreign);
    let l1 = std::os::unix::net::UnixListener::bind(&foreign).expect("bind");
    drop(l1);
    cleanup_fallback_socket_with_candidates(&foreign, &run_dir, 42011, None, uid);
    assert!(foreign.exists(), "foreign name must not be deleted");
    let _ = fs::remove_file(&foreign);

    let other_port = fallback_root.join("m42012-abc123def456.sock");
    let _ = fs::remove_file(&other_port);
    let l2 = std::os::unix::net::UnixListener::bind(&other_port).expect("bind");
    drop(l2);
    cleanup_fallback_socket_with_candidates(&other_port, &run_dir, 42011, None, uid);
    assert!(other_port.exists(), "other-port socket must not be deleted");
    let _ = fs::remove_file(&other_port);
}

#[test]
fn cleanup_fallback_socket_removes_real_socket_with_right_name_and_parent() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let fallback_root = ensure_fallback_socket_dir_for(None, uid).expect("fb");
    let fallback = fallback_root.join("m42013-abc123def456.sock");
    let _ = fs::remove_file(&fallback);
    let listener = std::os::unix::net::UnixListener::bind(&fallback).expect("bind");
    drop(listener);
    assert!(fallback.exists());
    cleanup_fallback_socket_with_candidates(&fallback, &run_dir, 42013, None, uid);
    assert!(
        !fallback.exists(),
        "verified fallback socket should be removed"
    );
}

#[test]
fn cleanup_refuses_socket_outside_allowed_dirs() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let stray = dir.path().join("elsewhere");
    crate::fsutil::ensure_private_dir(&stray).expect("stray");
    let sock = stray.join("m42206-abc123def456.sock");
    let l = std::os::unix::net::UnixListener::bind(&sock).expect("bind");
    drop(l);
    cleanup_fallback_socket_with_candidates(&sock, &run_dir, 42206, None, uid);
    assert!(
        sock.exists(),
        "socket outside the candidate dirs must be left alone"
    );
}

#[test]
fn cleanup_from_my_cnf_skips_foreign_socket_and_removes_verified_one() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let xdg = dir.path().join("xdg");
    let fb = xdg.join("dblocal");
    crate::fsutil::ensure_private_dir(&fb).expect("fb");
    let cnf = run_dir.join("my.cnf");
    let foreign = dir.path().join("notes.txt");
    fs::write(&foreign, b"notes").expect("notes");
    fs::write(&cnf, format!("[mysqld]\nsocket={}\n", foreign.display())).expect("cnf");
    cleanup_fallback_socket_from_my_cnf_with_candidates(
        &cnf,
        &run_dir,
        42215,
        Some(xdg.as_os_str().to_os_string()),
        uid,
    );
    assert!(
        foreign.exists(),
        "a regular non-fallback file must survive cleanup"
    );

    let sock = fb.join("m42215-abc123def456.sock");
    let l = std::os::unix::net::UnixListener::bind(&sock).expect("bind");
    drop(l);
    fs::write(&cnf, format!("[mysqld]\nsocket={}\n", sock.display())).expect("cnf2");
    cleanup_fallback_socket_from_my_cnf_with_candidates(
        &cnf,
        &run_dir,
        42215,
        Some(xdg.as_os_str().to_os_string()),
        uid,
    );
    assert!(
        !sock.exists(),
        "the verified fallback socket must be removed"
    );
}

#[test]
fn cleanup_from_my_cnf_without_socket_line_is_a_noop() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let cnf = run_dir.join("my.cnf");
    fs::write(&cnf, "[mysqld]\nport=42232\n").expect("cnf");
    cleanup_fallback_socket_from_my_cnf_with_candidates(&cnf, &run_dir, 42232, None, uid);
}

#[test]
fn cleanup_fallback_socket_leaves_a_vanished_path_alone() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    let fb = dir.path().join("xdg").join("dblocal");
    crate::fsutil::ensure_private_dir(&fb).expect("fb");
    let sock = fb.join("m42233-abc123def456.sock");
    // The path is syntactically valid but never existed: cleanup must not fail.
    cleanup_fallback_socket_with_candidates(
        &sock,
        &run_dir,
        42233,
        Some(dir.path().join("xdg").as_os_str().to_os_string()),
        uid,
    );
    assert!(!sock.exists());
}

#[test]
fn cleanup_tolerates_socket_paths_without_parent_or_name() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let uid = nix::unistd::geteuid().as_raw();
    // No parent -> cannot be a verified fallback socket; cleanup must just skip.
    cleanup_fallback_socket_with_candidates(Path::new("/"), &run_dir, 42234, None, uid);
}
