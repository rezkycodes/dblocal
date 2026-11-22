//! `change_port` wiring for the two MySQL-family engines (W4-E findings 6 and 7).
//!
//! The adapters' own `sync_port` tests prove the rewrite; these prove that the
//! service's dispatch actually calls it for `EngineKey::Mysql`, which is the arm
//! that stayed a no-op while no MySQL artifact existed in the catalog. They live
//! here because `instance::helpers` is in this crate and they need no engine.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::Path;
use std::time::SystemTime;

use crate::model::{
    AuthMode, EngineKey, Instance, InstanceId, InstanceName, InstanceSpec, InstanceStatus, Port,
};
use crate::paths::DblPaths;

fn paths() -> (tempfile::TempDir, DblPaths) {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = DblPaths::with_root(dir.path());
    (dir, paths)
}

fn instance_at(engine: EngineKey, name: &str, port: u16) -> Instance {
    let spec = InstanceSpec::new(
        InstanceName::new(name).expect("name"),
        engine,
        "8.4.11",
        Some(Port::new(port).expect("port")),
        AuthMode::None,
    )
    .expect("spec");
    Instance::new(
        &spec,
        InstanceId::new(),
        InstanceStatus::Stopped,
        SystemTime::UNIX_EPOCH,
    )
}

fn write_my_cnf(dir: &Path, port: u16, socket: &Path) {
    fs::create_dir_all(dir).expect("dir");
    let body = format!(
        "[mysqld]\nbind-address=127.0.0.1\nport={port}\n\
         datadir=/i/data\nsocket={}\nlog-error=/i/logs/mysql.log\n",
        socket.display()
    );
    fs::write(dir.join("my.cnf"), body).expect("write my.cnf");
}

#[test]
fn sync_instance_port_rewrites_mysql_my_cnf() {
    // Failing-first for finding 6: on main `EngineKey::Mysql => {}` left
    // my.cnf on the old port, so the start after change_port bound the old one.
    let (_tmp, paths) = paths();
    let name = InstanceName::new("mysql-sync").expect("name");
    let run = paths.run_dir(&name);
    let old = paths.run_dir(&name).join("mysqld-42345.sock");
    write_my_cnf(&run, 42345, &old);

    let instance = instance_at(EngineKey::Mysql, "mysql-sync", 43110);
    crate::instance::helpers::sync_instance_port(&paths, &instance).expect("sync");

    let after = fs::read_to_string(run.join("my.cnf")).expect("read");
    assert!(
        after.contains("port=43110"),
        "new port must be written:\n{after}"
    );
    assert!(
        !after.contains("port=42345"),
        "old port must be gone:\n{after}"
    );
    assert!(
        after.contains(&format!(
            "socket={}",
            paths.run_dir(&name).join("mysqld-43110.sock").display()
        )),
        "socket must follow the port:\n{after}"
    );
}

#[test]
fn sync_instance_port_rewrites_mariadb_my_cnf() {
    // The same dispatch for MariaDB was equally untested; this keeps the two
    // arms from drifting.
    let (_tmp, paths) = paths();
    let name = InstanceName::new("mariadb-sync").expect("name");
    let data = paths.data_dir(&name);
    fs::create_dir_all(&data).expect("data");
    let body = "[mysqld]\nbind-address=127.0.0.1\nport=42345\n\
                socket=/i/run/mariadb-42345.sock\nlog-error=/i/logs/mariadb.log\n";
    fs::write(data.join("my.cnf"), body).expect("write my.cnf");

    let instance = instance_at(EngineKey::Mariadb, "mariadb-sync", 43110);
    crate::instance::helpers::sync_instance_port(&paths, &instance).expect("sync");

    let after = fs::read_to_string(data.join("my.cnf")).expect("read");
    assert!(
        after.contains("port=43110"),
        "new port must be written:\n{after}"
    );
    assert!(
        !after.contains("port=42345"),
        "old port must be gone:\n{after}"
    );
}

#[test]
fn sync_instance_port_is_a_no_op_when_the_config_is_missing() {
    // A create that never finished has no my.cnf; the sync must not invent one.
    let (_tmp, paths) = paths();
    let instance = instance_at(EngineKey::Mysql, "not-created", 42345);
    crate::instance::helpers::sync_instance_port(&paths, &instance).expect("no-op");
    assert!(!paths.run_dir(instance.name()).join("my.cnf").exists());
}

#[test]
fn mysql_family_adapters_write_the_log_the_service_reads() {
    // Finding 7: `log_file_for` returned `logs/mysql.log` / `logs/mariadb.log`
    // while the adapters wrote `error.log` / `mariadb.err`, so the failure view
    // and the log viewer read a nearly empty file. One path is now the truth.
    let (_tmp, paths) = paths();
    let mysql = instance_at(EngineKey::Mysql, "log-mysql", 42345);
    assert_eq!(
        crate::engine::mysql_log_path(&paths.logs_dir(mysql.name())),
        crate::instance::helpers::log_file_for(&paths, &mysql),
    );
    let mariadb = instance_at(EngineKey::Mariadb, "log-mariadb", 42345);
    assert_eq!(
        crate::engine::mariadb_log_path(&paths.data_dir(mariadb.name())),
        crate::instance::helpers::log_file_for(&paths, &mariadb),
    );
}
