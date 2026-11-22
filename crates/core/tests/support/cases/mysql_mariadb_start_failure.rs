//! Finding 7 — a start failure on a *spawned* MySQL/MariaDB must show the real
//! diagnosis and a non-empty log tail.
//!
//! Both adapters write `log-error=` into `my.cnf`, so the server logs there and
//! not to stderr. Before the fix, `log_file_for` read `logs/mysql.log` /
//! `logs/mariadb.log` while the adapters wrote `error.log` / `mariadb.err`, so a
//! bind failure surfaced as `StartFailure::Unknown` with no tail. This drives a
//! real `LocalService` into exactly that failure: a foreign listener takes the
//! port *after* the service's own bind probe (a probe made to say "free"), the
//! engine spawns, fails to bind, and the failure view must still explain it.
//!
//! ```sh
//! flock -w 3600 "$HOME/.cache/dblocal-e2e.lock" \
//!   env DBLOCAL_E2E=1 CARGO_TARGET_DIR="$HOME/.cache/dblocal-target" \
//!   cargo test -p dblocal-core --test service_e2e -- --ignored --nocapture \
//!   start_failure
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use dblocal_core::model::{DeleteMode, EngineKey, InstanceStatus};
use dblocal_core::paths::DblPaths;
use dblocal_core::service::{DblService, LocalService, ServiceOptions};

use crate::support::cache;
use crate::support::clients::wait_tcp_gone;
use crate::support::env::TempHome;
use crate::support::harness::{self, newest_spec};
use crate::support::{ports, proc};

/// The version each engine publishes on this platform.
fn engine_version(engine: EngineKey) -> &'static str {
    match engine {
        EngineKey::Mysql => "8.4.11",
        EngineKey::Mariadb => "11.8.10",
        _ => unreachable!("only the MySQL family is proved here"),
    }
}

fn cached_archive(engine: EngineKey) -> &'static str {
    match engine {
        EngineKey::Mysql => "mysql-8.4.11-linux-glibc2.28-x86_64-minimal.tar.xz",
        EngineKey::Mariadb => "mariadb-11.8.10-linux-systemd-x86_64.tar.gz",
        _ => unreachable!(),
    }
}

#[test]
#[ignore = "e2e: needs the cached MySQL archive; run with DBLOCAL_E2E=1 under flock"]
fn mysql_start_failure_reports_port_in_use_with_a_log_tail() {
    super::require_e2e();
    run_start_failure_case(EngineKey::Mysql);
}

#[test]
#[ignore = "e2e: needs the cached MariaDB archive and libcrypt.so.1; run with DBLOCAL_E2E=1 under flock"]
fn mariadb_start_failure_reports_port_in_use_with_a_log_tail() {
    super::require_e2e();
    if !crate::support::mariadb_shim::ensure_shim_for_runtime() {
        eprintln!("skip mariadb start-failure e2e: no libcrypt.so.1 is reachable here");
        return;
    }
    run_start_failure_case(EngineKey::Mariadb);
}

fn run_start_failure_case(engine: EngineKey) {
    let version = engine_version(engine);
    let home = TempHome::new("dblocal-e2e-startfail-");
    let paths = home.paths();
    assert!(
        cache::extract_archive_to_runtime(&paths, engine, version, cached_archive(engine)),
        "this case needs {} cached in ~/.cache/dblocal-dev",
        cached_archive(engine)
    );

    // The runtime is already extracted, so create takes the "installed" path and
    // the bundled catalog supplies the version + size; no catalog override is
    // needed. The service's own bind probe must pass, because the port is taken
    // *after* it by the foreign listener held below, so the engine really spawns
    // and fails on bind instead of the service refusing before it spawns.
    let options = ServiceOptions {
        port_free: Some(Arc::new(|_port: u16| true)),
        ..harness::real_options(&paths)
    };
    let service = LocalService::new(paths.clone(), options).expect("service");

    let port = ports::reserve();
    let spec = newest_spec(&service, "startfail", engine, port);
    assert_eq!(spec.version(), version);
    service.create_instance(spec).expect("create_instance");
    // Wait for the create to settle into Stopped.
    let id = wait_for_row(&paths, "startfail");
    wait_status(&service, &id, InstanceStatus::Stopped);

    // Take the port now, after the service's recorded probe said it was free.
    let foreign = ports::hold(port);
    let mut events = crate::support::events::Events::new(service.subscribe());
    let op = service.start_instance(&id).expect("start");
    let run = events.until_finished(&op, crate::START_TIMEOUT);
    let err = super::expect_err(&run, "start on a port taken after create");
    eprintln!(
        "{} start failure: [{}] {}; stages {:?}",
        engine.as_str(),
        err.code,
        err.message,
        run.stage_names()
    );

    // The typed error and the failure view must both name the real cause.
    let view = service
        .list_instances()
        .expect("list")
        .into_iter()
        .find(|v| v.id == id)
        .expect("the failed row is listed");
    assert_eq!(view.status, InstanceStatus::Failed);
    let failure = view.failure.expect("a Failed view must explain itself");
    assert_eq!(
        failure.diagnosis,
        dblocal_core::model::StartFailure::PortInUse,
        "the diagnosis must come from the real {} log, not be Unknown: {}",
        engine.as_str(),
        failure.message
    );
    assert!(
        !failure.log_tail.is_empty(),
        "the failure view must carry a log tail from {}; the unified log path is what makes this non-empty",
        engine.as_str()
    );
    let tail = failure.log_tail.join("\n").to_ascii_lowercase();
    assert!(
        tail.contains("address already in use") || tail.contains("bind"),
        "the tail must contain the bind error, got: {tail}"
    );

    drop(foreign);
    assert!(wait_tcp_gone(port, Duration::from_secs(20)));
    // Cleanup: delete the instance and prove nothing leaked.
    service
        .delete_instance(&id, DeleteMode::PurgeData)
        .expect("delete");
    assert!(
        proc::wait_none_under(home.path(), Duration::from_secs(30)),
        "no {} engine may remain: {:?}",
        engine.as_str(),
        proc::engines_under(home.path())
    );
}

fn wait_for_row(paths: &DblPaths, name: &str) -> dblocal_core::model::InstanceId {
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    loop {
        let rows = dblocal_core::store::Store::open(paths)
            .expect("store")
            .list_instances()
            .expect("list");
        if let Some(instance) = rows.iter().find(|i| i.name().as_str() == name) {
            return instance.id().clone();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "row {name} never appeared"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn wait_status(
    service: &LocalService,
    id: &dblocal_core::model::InstanceId,
    status: InstanceStatus,
) {
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    loop {
        let row = service
            .list_instances()
            .expect("list")
            .into_iter()
            .find(|v| v.id == *id);
        if row.as_ref().is_some_and(|v| v.status == status) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "instance never reached {status:?}: {:?}",
            row.map(|v| v.status)
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}
