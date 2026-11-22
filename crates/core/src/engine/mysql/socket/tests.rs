//! MySQL-specific socket-path tests.
//!
//! The behaviour (candidates, verification, cleanup rules, `prepare_run`) is
//! tested once in `engine::sockdir`; here we only prove this adapter maps the
//! MySQL layout onto it correctly.

use super::*;
use crate::engine::sockdir;
use crate::engine::{AuthMode, RunContext};
use crate::error::DblError;
use crate::model::Port;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

fn uid() -> u32 {
    sockdir::current_uid()
}

fn bind_socket(path: &Path) {
    let _ = fs::remove_file(path);
    let listener = std::os::unix::net::UnixListener::bind(path).expect("bind socket");
    drop(listener);
}

fn private(dir: &Path) -> PathBuf {
    crate::fsutil::ensure_private_dir(dir).expect("private dir");
    dir.to_path_buf()
}

#[test]
fn natural_socket_path_is_mysqld_port_under_run_dir() {
    let p = socket_path(Path::new("/home/u/.dblocal/instances/app/run"), 3307);
    assert_eq!(
        p,
        PathBuf::from("/home/u/.dblocal/instances/app/run/mysqld-3307.sock")
    );
}

#[test]
fn resolve_uses_the_run_dir_when_short_and_falls_back_when_deep() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let instance = dir.path().join("inst");

    let short =
        resolve_socket_path_with_env(&run_dir, 42000, &instance, None, uid()).expect("short");
    assert_eq!(short, socket_path(&run_dir, 42000));

    let deep_run = dir.path().join("r".repeat(140)).join("run");
    fs::create_dir_all(&deep_run).expect("deep run");
    let chosen =
        resolve_socket_path_with_env(&deep_run, 42000, &instance, None, uid()).expect("fallback");
    assert_ne!(chosen, socket_path(&deep_run, 42000));
    assert!(sockdir::socket_path_fits(&chosen));
    let name = chosen.file_name().unwrap().to_string_lossy();
    assert!(name.starts_with("m42000-"), "{name}");
    assert_eq!(name, sockdir::fallback_socket_name(42000, &instance));
}

#[test]
fn validate_socket_path_len_accepts_at_limit_and_rejects_one_over() {
    let limit = socket_path_limit();
    let base = "/tmp/s/";
    let suffix = ".sock";
    let fixed = base.len() + suffix.len();
    let at_limit = format!("{base}{}{suffix}", "a".repeat(limit - fixed));
    validate_socket_path_len(Path::new(&at_limit)).expect("at limit");
    let over = format!("{base}{}{suffix}", "a".repeat(limit - fixed + 1));
    let err = validate_socket_path_len(Path::new(&over)).unwrap_err();
    assert!(matches!(
        err,
        DblError::InvalidInput {
            field: "socket",
            ..
        }
    ));
}

#[test]
fn socket_path_from_my_cnf_reads_the_directive() {
    let dir = tempfile::tempdir().expect("tmp");
    let cnf = dir.path().join("my.cnf");
    fs::write(&cnf, "[mysqld]\n socket = /tmp/dblocal-1/m42000-abcd.sock \n").expect("write");
    assert_eq!(
        socket_path_from_my_cnf(&cnf),
        Some(PathBuf::from("/tmp/dblocal-1/m42000-abcd.sock"))
    );
    fs::write(&cnf, "[mysqld]\nsocket=\nport=42000\n").expect("write");
    assert_eq!(socket_path_from_my_cnf(&cnf), None);
}

#[test]
fn cleanup_from_my_cnf_skips_foreign_socket_and_removes_a_verified_one() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let xdg = dir.path().join("xdg");
    fs::create_dir_all(&xdg).expect("xdg");
    let fb = private(&xdg.join("dblocal"));
    let cnf = run_dir.join("my.cnf");

    let foreign = dir.path().join("notes.txt");
    fs::write(&foreign, b"notes").expect("notes");
    fs::write(&cnf, format!("[mysqld]\nsocket={}\n", foreign.display())).expect("cnf");
    cleanup_fallback_socket_from_my_cnf_with_env(
        &cnf,
        42215,
        Some(xdg.as_os_str().to_os_string()),
        uid(),
    );
    assert!(foreign.exists(), "a regular non-fallback file must survive");

    let sock = fb.join("m42215-abc123def456.sock");
    bind_socket(&sock);
    fs::write(&cnf, format!("[mysqld]\nsocket={}\n", sock.display())).expect("cnf2");
    cleanup_fallback_socket_from_my_cnf_with_env(
        &cnf,
        42215,
        Some(xdg.as_os_str().to_os_string()),
        uid(),
    );
    assert!(!sock.exists(), "the verified fallback socket must be removed");
}

#[test]
fn prepare_run_creates_the_private_tmpdir_and_recreates_a_lost_fallback_dir() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let xdg = dir.path().join("xdg");
    fs::create_dir_all(&xdg).expect("xdg");
    let fb = xdg.join("dblocal");
    let sock = fb.join("m42120-abc123def456.sock");
    fs::write(
        run_dir.join("my.cnf"),
        format!("[mysqld]\nsocket={}\nport=42120\n", sock.display()),
    )
    .expect("cnf");

    prepare_runtime_dirs_for_start_with_env(
        &run_dir,
        42120,
        Some(xdg.as_os_str().to_os_string()),
        uid(),
    )
    .expect("prepare");

    let tmp = run_dir.join("tmp");
    let meta = fs::symlink_metadata(&tmp).expect("tmp meta");
    assert!(meta.is_dir());
    assert_eq!(meta.permissions().mode() & 0o777, 0o700);
    assert_eq!(meta.uid(), uid());
    let fb_meta = fs::symlink_metadata(&fb).expect("fb meta");
    assert_eq!(fb_meta.permissions().mode() & 0o777, 0o700);
    assert_eq!(fb_meta.uid(), uid());
}

#[test]
fn prepare_run_removes_a_stale_socket_and_refuses_a_regular_file() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let xdg = dir.path().join("xdg");
    fs::create_dir_all(&xdg).expect("xdg");
    let fb = private(&xdg.join("dblocal"));
    let sock = fb.join("m42121-abc123def456.sock");
    bind_socket(&sock);
    fs::write(run_dir.join("my.cnf"), format!("[mysqld]\nsocket={}\n", sock.display()))
        .expect("cnf");

    prepare_runtime_dirs_for_start_with_env(
        &run_dir,
        42121,
        Some(xdg.as_os_str().to_os_string()),
        uid(),
    )
    .expect("prepare");
    assert!(!sock.exists(), "a stale socket is removed");

    fs::write(&sock, b"not socket").expect("write");
    let err = prepare_runtime_dirs_for_start_with_env(
        &run_dir,
        42121,
        Some(xdg.as_os_str().to_os_string()),
        uid(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("not a socket"), "{err}");
    let _ = fs::remove_file(&sock);
}

#[test]
fn prepare_run_refuses_a_symlinked_or_loose_fallback_dir() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let xdg = dir.path().join("xdg");
    fs::create_dir_all(&xdg).expect("xdg");
    let target = private(&dir.path().join("target"));
    symlink(&target, xdg.join("dblocal")).expect("symlink");
    let sock = xdg.join("dblocal").join("m42122-abc123def456.sock");
    fs::write(run_dir.join("my.cnf"), format!("[mysqld]\nsocket={}\n", sock.display()))
        .expect("cnf");
    let err = prepare_runtime_dirs_for_start_with_env(
        &run_dir,
        42122,
        Some(xdg.as_os_str().to_os_string()),
        uid(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("not a real directory"), "{err}");

    let loose_xdg = dir.path().join("loose-xdg");
    let loose = loose_xdg.join("dblocal");
    fs::create_dir_all(&loose).expect("loose");
    fs::set_permissions(&loose, fs::Permissions::from_mode(0o755)).expect("chmod");
    let sock = loose.join("m42122-abc123def456.sock");
    fs::write(run_dir.join("my.cnf"), format!("[mysqld]\nsocket={}\n", sock.display()))
        .expect("cnf");
    let err = prepare_runtime_dirs_for_start_with_env(
        &run_dir,
        42122,
        Some(loose_xdg.as_os_str().to_os_string()),
        uid(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("0700"), "{err}");
    assert_eq!(
        fs::symlink_metadata(&loose).unwrap().permissions().mode() & 0o777,
        0o755,
        "prepare must not chmod a pre-existing fallback dir"
    );
}

#[test]
fn prepare_run_rejects_a_socket_outside_the_fallback_dirs() {
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    let sock = dir.path().join("stray.sock");
    fs::write(run_dir.join("my.cnf"), format!("[mysqld]\nsocket={}\n", sock.display()))
        .expect("cnf");
    let err =
        prepare_runtime_dirs_for_start_with_env(&run_dir, 42221, None, uid()).unwrap_err();
    assert!(
        err.to_string().contains("not an allowed fallback socket path"),
        "{err}"
    );
}

#[test]
fn prepare_run_errors_when_the_tmpdir_cannot_be_created() {
    if uid() == 0 {
        eprintln!("skipping as root: a read-only parent does not block root");
        return;
    }
    let dir = tempfile::tempdir().expect("tmp");
    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).expect("run");
    fs::write(run_dir.join("tmp"), b"x").expect("a file where the tmpdir must be");
    let err =
        prepare_runtime_dirs_for_start_with_env(&run_dir, 42214, None, uid()).unwrap_err();
    assert!(
        err.to_string().contains("cannot prepare private tmpdir"),
        "{err}"
    );
}

#[test]
fn public_prepare_run_prepares_the_tmpdir_before_any_config_exists() {
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
    crate::engine::mysql::prepare_run(&ctx).expect("prepare_run");
    let meta = fs::symlink_metadata(run_dir.join("tmp")).expect("tmp meta");
    assert_eq!(meta.permissions().mode() & 0o777, 0o700);
}
