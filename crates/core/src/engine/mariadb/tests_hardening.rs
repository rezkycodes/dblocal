//! Hardening and S1-S4 regression tests for MariaDB adapter.

use super::*;
use crate::engine::{InitContext, RunContext};
use crate::model::{AuthMode, Port};
use std::ffi::OsString;
use tempfile::tempdir;

fn init_ctx(auth: AuthMode, password: Option<&str>) -> InitContext {
    InitContext {
        instance_name: "mdb-test".into(),
        runtime_dir: PathBuf::from("/opt/mariadb"),
        data_dir: PathBuf::from("/tmp/dblocal/instances/mdb/data"),
        port: Port::new(3307).expect("port"),
        superuser: "root".into(),
        auth,
        password: password.map(SecretString::from),
    }
}

fn init_ctx_at(root: &Path, port: u16, auth: AuthMode, password: Option<&str>) -> InitContext {
    InitContext {
        instance_name: "mdb-test".into(),
        runtime_dir: root.join("rt"),
        data_dir: root.join("inst/data"),
        port: Port::new(port).expect("port"),
        superuser: "root".into(),
        auth,
        password: password.map(SecretString::from),
    }
}

#[test]
fn config_tmpdir_is_instance_private() {
    let ctx = init_ctx(AuthMode::None, None);
    let sock = socket_path(&ctx.data_dir, ctx.port.get());
    let conf = render_config(&config_params(&ctx, &sock).expect("params"));
    assert!(
        conf.expose()
            .contains("tmpdir=/tmp/dblocal/instances/mdb/run/tmp")
    );
}

#[test]
fn spawn_command_sets_tmpdir_env() {
    let ctx = RunContext {
        instance_name: "mdb-test".into(),
        runtime_dir: PathBuf::from("/opt/mariadb"),
        data_dir: PathBuf::from("/tmp/dblocal/instances/mdb/data"),
        logs_dir: PathBuf::from("/tmp/dblocal/instances/mdb/logs"),
        run_dir: PathBuf::from("/tmp/dblocal/instances/mdb/run"),
        port: Port::new(3307).expect("port"),
        superuser: "root".into(),
        auth: AuthMode::None,
        credential: None,
    };
    let cmd = MariaDbAdapter.spawn_command(&ctx);
    let mut saw = false;
    for (k, v) in cmd.get_envs() {
        if k == OsString::from("TMPDIR").as_os_str() {
            assert_eq!(
                v.expect("tmpdir value").to_string_lossy(),
                "/tmp/dblocal/instances/mdb/run/tmp"
            );
            saw = true;
        }
    }
    assert!(saw, "spawn command must set TMPDIR");
}

#[test]
fn bootstrap_command_sets_tmpdir_env() {
    let ctx = init_ctx(AuthMode::Password, Some("pw"));
    let init_file = PathBuf::from("/tmp/dblocal/instances/mdb/run/.mariadb-init.sql");
    let cmd = build_bootstrap_command(&ctx, &init_file);
    let mut saw = false;
    for (k, v) in cmd.get_envs() {
        if k == OsString::from("TMPDIR").as_os_str() {
            assert_eq!(
                v.expect("tmpdir value").to_string_lossy(),
                "/tmp/dblocal/instances/mdb/run/tmp"
            );
            saw = true;
        }
    }
    assert!(saw, "bootstrap command must set TMPDIR");
}

#[test]
fn prepare_dirs_creates_private_tmpdir() {
    let dir = tempdir().expect("tmp");
    let ctx = init_ctx_at(dir.path(), 43140, AuthMode::None, None);
    prepare_dirs(&ctx).expect("prepare");
    let tmp = dir.path().join("inst/run/tmp");
    assert!(tmp.is_dir(), "tmpdir should exist");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = fs::metadata(&tmp).expect("meta");
        assert_eq!(meta.mode() & 0o777, 0o700);
        assert_eq!(meta.uid(), nix::unistd::geteuid().as_raw());
    }
}

#[test]
fn prepare_dirs_cleans_stale_tmpdir_contents() {
    let dir = tempdir().expect("tmp");
    let ctx = init_ctx_at(dir.path(), 43141, AuthMode::None, None);
    let tmp = dir.path().join("inst/run/tmp");
    fs::create_dir_all(&tmp).expect("tmp");
    fs::write(tmp.join("#sql-temptable-stale"), b"old").expect("stale");
    fs::create_dir_all(tmp.join("nested")).expect("nested");
    fs::write(tmp.join("nested/file"), b"x").expect("nested file");
    prepare_dirs(&ctx).expect("prepare");
    let mut entries = fs::read_dir(&tmp).expect("read");
    assert!(
        entries.next().is_none(),
        "stale tmpdir contents must be cleaned"
    );
}

#[cfg(unix)]
#[test]
fn prepare_dirs_rejects_symlink_tmpdir() {
    use std::os::unix::fs::symlink;
    let dir = tempdir().expect("tmp");
    let ctx = init_ctx_at(dir.path(), 43142, AuthMode::None, None);
    let run = dir.path().join("inst/run");
    fs::create_dir_all(&run).expect("run");
    let target = dir.path().join("outside");
    fs::create_dir_all(&target).expect("target");
    symlink(&target, run.join("tmp")).expect("symlink");
    let err = prepare_dirs(&ctx).expect_err("symlink tmpdir must fail");
    assert!(matches!(err, DblError::Engine { .. }));
}

#[test]
fn ps_command_output_matches_exe_path_with_spaces() {
    let expected = ExpectedIdentity {
        exe: PathBuf::from(
            "/Users/x/Library/Application Support/DBLocal/runtimes/mariadb/11.8/bin/mariadbd",
        ),
        data_dir: PathBuf::from("/Users/x/Library/Application Support/DBLocal/instances/a/data"),
        config_path: PathBuf::from(
            "/Users/x/Library/Application Support/DBLocal/instances/a/data/my.cnf",
        ),
    };
    let cmd = "/Users/x/Library/Application Support/DBLocal/runtimes/mariadb/11.8/bin/mariadbd --defaults-file=/Users/x/Library/Application Support/DBLocal/instances/a/data/my.cnf --datadir=/Users/x/Library/Application Support/DBLocal/instances/a/data";
    let id = verify_ps_command_output(cmd, &expected);
    assert_eq!(id, PidIdentity::Match);
}

#[test]
fn ps_command_output_rejects_other_instance_with_spaces() {
    let expected = ExpectedIdentity {
        exe: PathBuf::from(
            "/Users/x/Library/Application Support/DBLocal/runtimes/mariadb/11.8/bin/mariadbd",
        ),
        data_dir: PathBuf::from("/Users/x/Library/Application Support/DBLocal/instances/a/data"),
        config_path: PathBuf::from(
            "/Users/x/Library/Application Support/DBLocal/instances/a/data/my.cnf",
        ),
    };
    let cmd = "/Users/x/Library/Application Support/DBLocal/runtimes/mariadb/11.8/bin/mariadbd --defaults-file=/Users/x/Library/Application Support/DBLocal/instances/b/data/my.cnf --datadir=/Users/x/Library/Application Support/DBLocal/instances/b/data";
    let id = verify_ps_command_output(cmd, &expected);
    assert!(matches!(id, PidIdentity::NotOurs(_)), "{id:?}");
}

#[test]
fn ps_command_output_rejects_data_dir_prefix_collision() {
    let expected = ExpectedIdentity {
        exe: PathBuf::from("/opt/mariadb/bin/mariadbd"),
        data_dir: PathBuf::from("/Users/x/Library/Application Support/DBLocal/instances/a/data"),
        config_path: PathBuf::from(
            "/Users/x/Library/Application Support/DBLocal/instances/a/data/my.cnf",
        ),
    };
    let cmd = "/opt/mariadb/bin/mariadbd --defaults-file=/Users/x/Library/Application Support/DBLocal/instances/a/data2/my.cnf --datadir=/Users/x/Library/Application Support/DBLocal/instances/a/data2";
    let id = verify_ps_command_output(cmd, &expected);
    assert!(matches!(id, PidIdentity::NotOurs(_)), "{id:?}");
}

#[test]
fn ps_command_output_rejects_different_binary() {
    let expected = ExpectedIdentity {
        exe: PathBuf::from("/opt/mariadb/bin/mariadbd"),
        data_dir: PathBuf::from("/tmp/inst/data"),
        config_path: PathBuf::from("/tmp/inst/data/my.cnf"),
    };
    let cmd = "/usr/bin/sleep --defaults-file=/tmp/inst/data/my.cnf --datadir=/tmp/inst/data";
    let id = verify_ps_command_output(cmd, &expected);
    assert!(matches!(id, PidIdentity::NotOurs(_)), "{id:?}");
}

#[test]
fn shutdown_unverifiable_identity_is_error() {
    let err = shutdown_verified_pid(
        4242,
        PidIdentity::Unverifiable("cannot read exe: Permission denied".to_owned()),
        Duration::from_millis(10),
    )
    .expect_err("unverifiable must be error");
    match err {
        DblError::Engine { reason } => {
            assert!(reason.contains("4242"));
            assert!(reason.contains("unverifiable"));
            assert!(reason.contains("Permission denied"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn shutdown_no_such_process_is_ok() {
    shutdown_verified_pid(
        999_999,
        PidIdentity::NoSuchProcess,
        Duration::from_millis(10),
    )
    .expect("no such process is ok");
}

#[test]
fn shutdown_not_ours_is_ok() {
    shutdown_verified_pid(
        999_998,
        PidIdentity::NotOurs("exe mismatch".to_owned()),
        Duration::from_millis(10),
    )
    .expect("not ours is ok");
}
