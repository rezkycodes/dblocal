use super::*;

fn write_conf(run_dir: &Path, port: u16, socket: &Path) {
    let params = MyCnfParams {
        port,
        data_dir: Path::new("/i/data"),
        run_dir,
        logs_dir: Path::new("/i/logs"),
        runtime_dir: Path::new("/rt/mysql"),
        socket,
    };
    write_private_config_atomic(&my_cnf_path(run_dir), &render_my_cnf(&params)).expect("conf");
}

#[test]
fn sync_port_rewrites_port_and_socket() {
    let dir = tempfile::tempdir().expect("tmp");
    let run = dir.path().join("inst/run");
    fs::create_dir_all(&run).expect("run");
    let old_socket = socket_path(&run, 42345);
    write_conf(&run, 42345, &old_socket);
    let before = fs::read_to_string(my_cnf_path(&run)).expect("read");
    assert!(before.contains("port=42345"));
    assert!(before.contains("socket="));

    sync_port(&run, 43110).expect("sync");

    let after = fs::read_to_string(my_cnf_path(&run)).expect("read after");
    assert!(after.contains("port=43110"), "port must move:\n{after}");
    assert!(!after.contains("port=42345"), "old port must be gone");
    assert!(
        after.contains(&format!("socket={}", socket_path(&run, 43110).display())),
        "socket must follow the port:\n{after}"
    );
    // Everything else is preserved.
    assert!(after.contains("datadir=/i/data"));
    assert!(after.contains("basedir=/rt/mysql"));
    assert!(after.contains("log-error=/i/logs/mysql.log"));
}

#[test]
fn sync_port_is_idempotent() {
    let dir = tempfile::tempdir().expect("tmp");
    let run = dir.path().join("inst/run");
    fs::create_dir_all(&run).expect("run");
    write_conf(&run, 42345, &socket_path(&run, 42345));
    sync_port(&run, 43110).expect("first");
    let once = fs::read_to_string(my_cnf_path(&run)).expect("read");
    sync_port(&run, 43110).expect("second");
    let twice = fs::read_to_string(my_cnf_path(&run)).expect("read");
    assert_eq!(once, twice, "a second sync writes the same bytes");
}

#[test]
fn sync_port_errors_without_port_directive() {
    let dir = tempfile::tempdir().expect("tmp");
    let run = dir.path().join("inst/run");
    fs::create_dir_all(&run).expect("run");
    write_private_config_atomic(&my_cnf_path(&run), "[mysqld]\ndatadir=/tmp\n").expect("conf");
    let err = sync_port(&run, 43111).expect_err("missing port");
    assert!(matches!(err, DblError::Engine { .. }));
}

#[test]
fn sync_port_errors_on_missing_config() {
    let dir = tempfile::tempdir().expect("tmp");
    let run = dir.path().join("inst/run");
    fs::create_dir_all(&run).expect("run");
    let err = sync_port(&run, 43111).expect_err("missing config");
    assert!(matches!(err, DblError::Engine { .. }));
    assert!(
        !my_cnf_path(&run).exists(),
        "a failed sync must not create a config"
    );
}
