use super::prelude::*;

// prepare_run
// ---------------------------------------------------------------------------

struct Layout {
    _dir: tempfile::TempDir,
    run: PathBuf,
    config: PathBuf,
    natural: PathBuf,
    tmpdir: PathBuf,
}

fn layout() -> Layout {
    let dir = tempdir().expect("tmp");
    let inst = dir.path().join("inst");
    let data = inst.join("data");
    let run = inst.join("run");
    fs::create_dir_all(&data).expect("data");
    fs::create_dir_all(&run).expect("run");
    Layout {
        run: run.clone(),
        config: run.join("my.cnf"),
        natural: run.join("mysqld-42200.sock"),
        tmpdir: run.join("tmp"),
        _dir: dir,
    }
}

fn spec<'a>(layout: &'a Layout, xdg: Option<OsString>) -> PrepareSpec<'a> {
    PrepareSpec {
        config: &layout.config,
        natural_socket: &layout.natural,
        port: 42200,
        tmpdir: &layout.tmpdir,
        xdg_runtime_dir: xdg,
        uid: uid(),
    }
}

#[test]
fn prepare_run_recreates_the_tmpdir_and_a_fallback_dir() {
    let l = layout();
    let xdg = l._dir.path().join("xdg");
    fs::create_dir_all(&xdg).expect("xdg");
    let fb = xdg.join("dblocal");
    let sock = fb.join("m42200-abc123def456.sock");
    fs::write(
        &l.config,
        format!("[mysqld]\nsocket={}\nport=42200\n", sock.display()),
    )
    .expect("cnf");

    prepare_run(&spec(&l, Some(xdg.as_os_str().to_os_string()))).expect("prepare");

    let meta = fs::symlink_metadata(&l.tmpdir).expect("tmpdir meta");
    assert!(meta.is_dir());
    assert_eq!(meta.permissions().mode() & 0o777, 0o700);
    assert_eq!(meta.uid(), uid());
    let fb_meta = fs::symlink_metadata(&fb).expect("fb meta");
    assert_eq!(fb_meta.permissions().mode() & 0o777, 0o700);
    assert_eq!(fb_meta.uid(), uid());
}

#[test]
fn prepare_run_creates_the_natural_run_dir() {
    let l = layout();
    fs::remove_dir_all(&l.run).expect("remove run");
    fs::create_dir_all(&l.run).expect("recreate run");
    fs::write(
        &l.config,
        format!("[mysqld]\nsocket={}\n", l.natural.display()),
    )
    .expect("cnf");
    prepare_run(&spec(&l, None)).expect("prepare");
    assert!(l.natural.parent().unwrap().is_dir());
    assert!(l.tmpdir.is_dir());
}

#[test]
fn prepare_run_removes_a_stale_socket_and_refuses_a_regular_file() {
    let l = layout();
    let xdg = l._dir.path().join("xdg");
    fs::create_dir_all(&xdg).expect("xdg");
    let fb = private(&xdg.join("dblocal"));
    let sock = fb.join("m42200-abc123def456.sock");
    bind_socket(&sock);
    fs::write(&l.config, format!("[mysqld]\nsocket={}\n", sock.display())).expect("cnf");

    prepare_run(&spec(&l, Some(xdg.as_os_str().to_os_string()))).expect("prepare");
    assert!(!sock.exists(), "a stale socket left by a crash is removed");

    fs::write(&sock, b"not socket").expect("write");
    let err = prepare_run(&spec(&l, Some(xdg.as_os_str().to_os_string())))
        .expect_err("a regular file at the socket path must fail the start");
    assert!(err.to_string().contains("not a socket"), "{err}");
}

#[test]
fn prepare_run_refuses_a_symlinked_or_loose_fallback_dir() {
    let l = layout();
    let xdg = l._dir.path().join("xdg");
    fs::create_dir_all(&xdg).expect("xdg");
    let target = private(&l._dir.path().join("target"));
    let link = xdg.join("dblocal");
    symlink(&target, &link).expect("symlink");
    let sock = link.join("m42200-abc123def456.sock");
    fs::write(&l.config, format!("[mysqld]\nsocket={}\n", sock.display())).expect("cnf");
    let err = prepare_run(&spec(&l, Some(xdg.as_os_str().to_os_string()))).expect_err("symlink");
    assert!(err.to_string().contains("not a real directory"), "{err}");

    let loose = l._dir.path().join("loose-xdg").join("dblocal");
    fs::create_dir_all(&loose).expect("loose");
    fs::set_permissions(&loose, fs::Permissions::from_mode(0o755)).expect("chmod");
    let sock = loose.join("m42200-abc123def456.sock");
    fs::write(&l.config, format!("[mysqld]\nsocket={}\n", sock.display())).expect("cnf");
    let err = prepare_run(&spec(
        &l,
        Some(l._dir.path().join("loose-xdg").as_os_str().to_os_string()),
    ))
    .expect_err("wrong mode");
    assert!(err.to_string().contains("0700"), "{err}");
    assert_eq!(mode(&loose), 0o755, "prepare must not chmod a foreign dir");
}

#[test]
fn prepare_run_rejects_a_socket_name_that_is_not_for_this_port() {
    let l = layout();
    let dir = l._dir.path().join("private");
    let private_dir = private(&dir);
    let sock = private_dir.join("mariadb-42200-aabb.sock");
    fs::write(&l.config, format!("[mysqld]\nsocket={}\n", sock.display())).expect("cnf");
    let err = prepare_run(&spec(&l, None)).expect_err("foreign socket name");
    assert!(
        err.to_string()
            .contains("not an allowed fallback socket path"),
        "{err}"
    );
    assert!(!sock.exists());
}

#[test]
fn prepare_run_clears_stale_tmpdir_contents() {
    let l = layout();
    fs::create_dir_all(&l.tmpdir).expect("tmpdir");
    fs::write(l.tmpdir.join("#sql-temptable-stale"), b"old").expect("stale");
    fs::create_dir_all(l.tmpdir.join("nested")).expect("nested");
    fs::write(l.tmpdir.join("nested/file"), b"x").expect("nested file");
    fs::write(
        &l.config,
        format!("[mysqld]\nsocket={}\n", l.natural.display()),
    )
    .expect("cnf");
    prepare_run(&spec(&l, None)).expect("prepare");
    assert_eq!(
        fs::read_dir(&l.tmpdir).expect("read").count(),
        0,
        "stale tmpdir contents must be cleared on every start"
    );
}

#[test]
fn prepare_run_skips_a_missing_config_and_rejects_one_without_socket() {
    let l = layout();
    prepare_run(&spec(&l, None)).expect("no config yet is not an error");
    assert!(l.tmpdir.is_dir(), "the tmpdir is still prepared");

    fs::write(&l.config, "[mysqld]\nport=42200\n").expect("cnf");
    let err = prepare_run(&spec(&l, None)).expect_err("no socket directive");
    assert!(
        err.to_string().contains("has no `socket` directive"),
        "{err}"
    );
}

#[test]
fn prepare_run_errors_when_the_tmpdir_cannot_be_created() {
    let l = layout();
    fs::write(&l.tmpdir, b"x").expect("a regular file where the tmpdir must be");
    let err = prepare_run(&spec(&l, None)).expect_err("tmpdir is a file");
    assert!(
        err.to_string().contains("cannot prepare private tmpdir"),
        "{err}"
    );
}

#[test]
fn prepare_run_errors_on_an_unreadable_config_that_exists() {
    if uid() == 0 {
        eprintln!("skipping as root: mode 000 does not block reads");
        return;
    }
    let l = layout();
    fs::write(&l.config, "[mysqld]\nsocket=/tmp/x.sock\n").expect("cnf");
    fs::set_permissions(&l.config, fs::Permissions::from_mode(0o000)).expect("chmod");
    let err = prepare_run(&spec(&l, None)).expect_err("unreadable config");
    fs::set_permissions(&l.config, fs::Permissions::from_mode(0o600)).expect("restore");
    assert!(err.to_string().contains("cannot read"), "{err}");
}

#[test]
fn prepare_run_errors_when_the_tmpdir_cannot_be_listed() {
    let l = layout();
    // A regular file at the tmpdir path makes the listing fail after the
    // ownership/tighten step would have refused it; either way it is an Engine
    // error naming the tmpdir.
    fs::write(&l.tmpdir, b"not a dir").expect("file");
    fs::write(
        &l.config,
        format!("[mysqld]\nsocket={}\n", l.natural.display()),
    )
    .expect("cnf");
    let err = prepare_run(&spec(&l, None)).expect_err("tmpdir is a file");
    assert!(
        err.to_string().contains("cannot prepare private tmpdir"),
        "{err}"
    );
}

#[test]
fn clear_dir_contents_removes_files_and_nested_directories() {
    let dir = tempdir().expect("tmp");
    let root = dir.path().join("root");
    fs::create_dir_all(root.join("sub")).expect("sub");
    fs::write(root.join("f"), b"x").expect("f");
    fs::write(root.join("sub/g"), b"x").expect("g");
    clear_dir_contents(&root).expect("clear");
    assert_eq!(fs::read_dir(&root).expect("read").count(), 0);
    assert!(root.is_dir(), "the directory itself stays");

    let err = clear_dir_contents(&dir.path().join("missing")).expect_err("missing dir");
    assert!(err.to_string().contains("cannot read private dir"), "{err}");
}

#[test]
fn prepare_run_removes_a_stale_natural_socket() {
    let l = layout();
    // A natural socket left by a crash must be removed before the next start.
    bind_socket(&l.natural);
    fs::write(
        &l.config,
        format!("[mysqld]\nsocket={}\n", l.natural.display()),
    )
    .expect("cnf");
    prepare_run(&spec(&l, None)).expect("prepare");
    assert!(!l.natural.exists(), "a stale natural socket is removed");
}

#[test]
fn prepare_run_reports_a_parent_that_is_a_file_for_the_natural_socket() {
    let l = layout();
    // The run dir is a regular file, so the natural socket's parent cannot be
    // prepared; the caller gets an Engine error naming it.
    let bad_run = l._dir.path().join("run-file");
    fs::write(&bad_run, b"x").expect("write run file");
    let config = l._dir.path().join("cfg/my.cnf");
    fs::create_dir_all(config.parent().unwrap()).expect("cfg dir");
    let natural = bad_run.join("mysqld-42200.sock");
    fs::write(&config, format!("[mysqld]\nsocket={}\n", natural.display())).expect("cnf");
    let spec = PrepareSpec {
        config: &config,
        natural_socket: &natural,
        port: 42200,
        tmpdir: &l.tmpdir,
        xdg_runtime_dir: None,
        uid: uid(),
    };
    let err = prepare_run(&spec).expect_err("run dir is a file");
    assert!(
        err.to_string().contains("cannot prepare run directory"),
        "{err}"
    );
}

#[test]
fn cleanup_tolerates_a_socket_that_cannot_be_stat_ed() {
    use std::os::unix::fs::PermissionsExt;
    if uid() == 0 {
        eprintln!("skipping as root: mode 000 on a parent does not block root");
        return;
    }
    let dir = tempdir().expect("tmp");
    let (xdg, fb) = xdg_candidate(dir.path());
    let sock = fb.join("m42500-abc123def456.sock");
    bind_socket(&sock);
    // A 0000 parent makes the entry un-stattable: cleanup must not fail, and it
    // must not delete a path it cannot verify.
    fs::set_permissions(&fb, fs::Permissions::from_mode(0o000)).expect("chmod");
    cleanup_socket(&sock, 42500, Some(xdg), uid());
    fs::set_permissions(&fb, fs::Permissions::from_mode(0o700)).expect("restore");
    assert!(sock.exists(), "an unverifiable path is left alone");
}

#[test]
fn prepare_run_reports_a_fallback_parent_that_is_a_file() {
    let l = layout();
    let not_dir = l._dir.path().join("fb-file");
    fs::write(&not_dir, b"x").expect("file");
    let sock = not_dir.join("m42200-abc123def456.sock");
    fs::write(&l.config, format!("[mysqld]\nsocket={}\n", sock.display())).expect("cnf");
    let err = prepare_run(&spec(&l, None)).expect_err("fallback parent is a file");
    assert!(
        err.to_string()
            .contains("cannot prepare fallback socket dir"),
        "{err}"
    );
}

#[test]
fn ensure_secure_dir_reports_a_regular_file_and_a_missing_parent_for_creation() {
    let dir = tempdir().expect("tmp");
    let file = dir.path().join("file");
    fs::write(&file, b"x").expect("file");
    let err = ensure_secure_dir(&file, uid()).expect_err("regular file");
    assert!(err.to_string().contains("not a real directory"), "{err}");
}

#[test]
fn ensure_secure_dir_reports_a_parent_it_cannot_create_and_a_dir_it_cannot_create() {
    use std::os::unix::fs::PermissionsExt;
    if uid() == 0 {
        eprintln!("skipping as root: a read-only parent does not block root");
        return;
    }
    let dir = tempdir().expect("tmp");
    let ro = dir.path().join("ro");
    fs::create_dir(&ro).expect("ro");
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o500)).expect("chmod");

    // `ro/x/y`: the immediate parent `ro/x` cannot be created.
    let err = ensure_secure_dir(&ro.join("x").join("y"), uid()).expect_err("parent create");
    assert!(err.to_string().contains("cannot create parent"), "{err}");

    // `ro/x`: `ro` exists, but the child cannot be created inside it.
    let err = ensure_secure_dir(&ro.join("x"), uid()).expect_err("dir create");
    assert!(
        err.to_string().contains("cannot create private dir"),
        "{err}"
    );

    fs::set_permissions(&ro, fs::Permissions::from_mode(0o700)).expect("restore");
}

#[test]
fn cleanup_warns_and_leaves_the_socket_when_removal_fails() {
    use std::os::unix::fs::PermissionsExt;
    if uid() == 0 {
        eprintln!("skipping as root: a read-only parent does not block root");
        return;
    }
    let dir = tempdir().expect("tmp");
    let (xdg, fb) = xdg_candidate(dir.path());
    let sock = fb.join("m42510-abc123def456.sock");
    bind_socket(&sock);
    // The socket verifies (parent is r-x), but the unlink fails with EACCES.
    fs::set_permissions(&fb, fs::Permissions::from_mode(0o500)).expect("chmod");
    cleanup_socket(&sock, 42510, Some(xdg), uid());
    fs::set_permissions(&fb, fs::Permissions::from_mode(0o700)).expect("restore");
    assert!(sock.exists(), "a socket that cannot be unlinked is kept");
}
