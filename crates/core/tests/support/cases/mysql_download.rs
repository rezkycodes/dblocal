//! MySQL 8.4.11 (Linux x86_64) end to end through the real service: the real
//! catalog entry, the real HTTPS downloader, SHA-256 verification, extraction
//! with the catalog's `strip_prefix`, then create/start/connect/stop/delete.
//!
//! This is the W4-E deliverable-4 proof. It uses the production `LocalService`
//! over a temp `DBLOCAL_HOME`; nothing about the download or extraction is faked.
//! The ~80 MB download is written under the temp home and removed when the guard
//! drops.
//!
//! ```sh
//! flock -w 3600 "$HOME/.cache/dblocal-e2e.lock" \
//!   env DBLOCAL_E2E=1 CARGO_TARGET_DIR="$HOME/.cache/dblocal-target" \
//!   cargo test -p dblocal-core --test service_e2e -- --ignored --nocapture \
//!   mysql_download
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use dblocal_core::model::{DeleteMode, EngineKey, InstanceStatus};
use dblocal_core::service::DblService;

use crate::support::clients::{MyClient, wait_tcp, wait_tcp_gone};
use crate::support::harness::{Harness, newest_spec, newest_version};
use crate::support::{ports, proc};

#[test]
#[ignore = "e2e: network + ~80 MB of disk; run with DBLOCAL_E2E=1 under flock"]
fn mysql_downloads_from_the_catalog_and_runs() {
    super::require_e2e();

    let harness = Harness::new("dblocal-e2e-mysql-dl-");
    let version = newest_version(&harness.service, EngineKey::Mysql);
    assert_eq!(version, "8.4.11", "the catalog publishes the 8.4 LTS line");
    let port = ports::reserve();
    let spec = newest_spec(&harness.service, "mysql-dl", EngineKey::Mysql, port);

    // Real download -> verify -> extract -> initialize through `create_instance`.
    let mut events = harness.events();
    let op = harness
        .service
        .create_instance(spec)
        .expect("create_instance accepts a MySQL spec");
    let run = events.until_finished(&op, crate::CREATE_TIMEOUT);
    let stages = run.stage_names();
    eprintln!("mysql download e2e create stages: {stages:?}");
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
    let downloads = run.downloads();
    assert!(
        !downloads.is_empty(),
        "the HTTPS download must report progress"
    );
    assert!(
        downloads.last().is_some_and(|p| p.is_complete()),
        "the full archive must have arrived: {:?}",
        downloads.last()
    );
    if let Err(err) = &run.result {
        panic!(
            "create failed: [{}] {}; stages {stages:?}",
            err.code, err.message
        );
    }

    // The extracted layout is the one the adapter expects: `bin/` at the root
    // after `strip_prefix`, which was the builder defect this case guards.
    let runtime = harness
        .paths
        .runtime_dir(
            EngineKey::Mysql,
            &version,
            dblocal_core::platform::Arch::current(),
        )
        .expect("runtime dir");
    assert!(
        runtime.join("bin/mysqld").is_file(),
        "extraction must put bin/mysqld under the runtime root ({})",
        runtime.display()
    );

    let id = harness
        .instance("mysql-dl")
        .expect("row after create")
        .id()
        .clone();
    assert_eq!(
        harness.instance_by_id(&id).expect("row").status(),
        InstanceStatus::Stopped
    );

    // Start, run SQL, then prove stop releases the port.
    let start = harness.start(&id);
    super::expect_ok(&start, "start");
    harness.wait_status(&id, InstanceStatus::Running, Duration::from_secs(5));
    assert!(
        wait_tcp(port, Duration::from_secs(30)),
        "mysqld must listen on {port}"
    );
    let uri = harness.uri(&id).expose().to_owned();
    let mut client = MyClient::connect(&uri).expect("connect with the service URI");
    assert_eq!(client.query_i64("SELECT 1").expect("SELECT 1"), 1);
    client.batch("CREATE DATABASE e2e_dl; CREATE TABLE e2e_dl.t (v INT); INSERT INTO e2e_dl.t VALUES (42);")
        .expect("seed");
    assert_eq!(
        client.query_i64("SELECT v FROM e2e_dl.t").expect("read"),
        42
    );

    let stop = harness.stop(&id);
    super::expect_ok(&stop, "stop");
    assert!(
        wait_tcp_gone(port, Duration::from_secs(20)),
        "port {port} must be released"
    );
    assert_eq!(
        harness.instance_by_id(&id).expect("row").status(),
        InstanceStatus::Stopped
    );

    // Delete: the instance tree and the engine are gone; `ss` lists no listener.
    let delete = harness.delete(&id, DeleteMode::PurgeData);
    super::expect_ok(&delete, "delete with PurgeData");
    harness.wait_row_gone(&id, Duration::from_secs(20));
    assert!(
        proc::wait_none_under(harness.home.path(), Duration::from_secs(30)),
        "no MySQL engine may remain: {:?}",
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
