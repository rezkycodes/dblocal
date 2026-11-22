//! MariaDB 11.8.10 (Linux x86_64) end to end through the real service, the same
//! proof as [`super::mysql_download`] for the second MySQL-family engine.
//!
//! This host lacks `libcrypt.so.1`, which `mariadbd` links, so a shim is placed
//! on `LD_LIBRARY_PATH` for this binary (see `support::mariadb_shim`); without
//! it the engine cannot even load and the test says so instead of failing.
//!
//! ```sh
//! flock -w 3600 "$HOME/.cache/dblocal-e2e.lock" \
//!   env DBLOCAL_E2E=1 CARGO_TARGET_DIR="$HOME/.cache/dblocal-target" \
//!   cargo test -p dblocal-core --test service_e2e -- --ignored --nocapture \
//!   mariadb_download
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use dblocal_core::model::{DeleteMode, EngineKey, InstanceStatus};
use dblocal_core::service::DblService;

use crate::support::clients::{MyClient, wait_tcp, wait_tcp_gone};
use crate::support::harness::{Harness, newest_spec, newest_version};
use crate::support::{mariadb_shim, ports, proc};

#[test]
#[ignore = "e2e: network + ~400 MB of disk and libcrypt.so.1; run with DBLOCAL_E2E=1 under flock"]
fn mariadb_downloads_from_the_catalog_and_runs() {
    super::require_e2e();
    if !mariadb_shim::ensure_shim_for_runtime() {
        eprintln!(
            "skip mariadb download e2e: this host cannot load libcrypt.so.1 and no \
             compatible library was found to shim it"
        );
        return;
    }

    let harness = Harness::new("dblocal-e2e-mariadb-dl-");
    let version = newest_version(&harness.service, EngineKey::Mariadb);
    assert_eq!(version, "11.8.10", "the newest published MariaDB line");
    let port = ports::reserve();
    let spec = newest_spec(&harness.service, "mariadb-dl", EngineKey::Mariadb, port);

    let mut events = harness.events();
    let op = harness
        .service
        .create_instance(spec)
        .expect("create_instance accepts a MariaDB spec");
    let run = events.until_finished(&op, crate::CREATE_TIMEOUT);
    let stages = run.stage_names();
    eprintln!("mariadb download e2e create stages: {stages:?}");
    for expected in [
        "queued",
        "downloading",
        "verifying",
        "extracting",
        "initializing",
        "done",
    ] {
        assert!(
            stages.contains(&expected),
            "stage {expected} missing from {stages:?}"
        );
    }
    assert!(
        run.downloads().last().is_some_and(|p| p.is_complete()),
        "the whole archive must have arrived: {:?}",
        run.downloads().last()
    );
    if let Err(err) = &run.result {
        panic!(
            "create failed: [{}] {}; stages {stages:?}",
            err.code, err.message
        );
    }

    let runtime = harness
        .paths
        .runtime_dir(
            EngineKey::Mariadb,
            &version,
            dblocal_core::platform::Arch::current(),
        )
        .expect("runtime dir");
    assert!(
        runtime.join("bin/mariadbd").is_file(),
        "extraction must put bin/mariadbd under the runtime root ({})",
        runtime.display()
    );

    let id = harness
        .instance("mariadb-dl")
        .expect("row after create")
        .id()
        .clone();
    let start = harness.start(&id);
    super::expect_ok(&start, "start");
    harness.wait_status(&id, InstanceStatus::Running, Duration::from_secs(5));
    assert!(
        wait_tcp(port, Duration::from_secs(30)),
        "mariadbd must listen on {port}"
    );
    let uri = harness.uri(&id).expose().to_owned();
    let mut client = MyClient::connect(&uri).expect("connect with the service URI");
    assert_eq!(client.query_i64("SELECT 1").expect("SELECT 1"), 1);

    let stop = harness.stop(&id);
    super::expect_ok(&stop, "stop");
    assert!(
        wait_tcp_gone(port, Duration::from_secs(20)),
        "port {port} must be released"
    );

    let delete = harness.delete(&id, DeleteMode::PurgeData);
    super::expect_ok(&delete, "delete with PurgeData");
    harness.wait_row_gone(&id, Duration::from_secs(20));
    assert!(
        proc::wait_none_under(harness.home.path(), Duration::from_secs(30)),
        "no MariaDB engine may remain: {:?}",
        proc::engines_under(harness.home.path())
    );
    if let Ok(output) = std::process::Command::new("ss").arg("-ltnp").output() {
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(
            !text.contains(&format!(":{port} ")),
            "ss still lists port {port} after delete:\n{text}"
        );
    }
}
