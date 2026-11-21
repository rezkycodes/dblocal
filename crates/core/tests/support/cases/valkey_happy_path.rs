//! Case 2 — Valkey happy path over the real catalog, HTTPS downloader and
//! engine: auth is required, `SET`/`GET` works, data survives a stop/start, and
//! a purge delete leaves nothing.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use dblocal_core::model::{DeleteMode, EngineKey, InstanceStatus};
use dblocal_core::service::DblService;

use crate::support::cache;
use crate::support::clients::{RedisClient, wait_tcp, wait_tcp_gone};
use crate::support::harness::{Harness, newest_spec, newest_version};
use crate::support::{ports, proc};

#[test]
#[ignore = "e2e: network + several hundred MB of disk; run with DBLOCAL_E2E=1 cargo xtask e2e"]
fn valkey_happy_path_requires_auth_and_persists() {
    super::require_e2e();

    let harness = Harness::new("dblocal-e2e-vk-");
    let version = newest_version(&harness.service, EngineKey::Valkey);
    eprintln!("valkey e2e: newest catalog version {version}");
    let port = ports::reserve();
    let spec = newest_spec(&harness.service, "vk-e2e", EngineKey::Valkey, port);

    let mut events = harness.events();
    let op = harness
        .service
        .create_instance(spec)
        .expect("create_instance must accept a valid spec");
    let run = events.until_finished(&op, crate::CREATE_TIMEOUT);

    if let Err(err) = &run.result {
        panic!(
            "create failed: [{}] {}; stages {:?}",
            err.code,
            err.message,
            run.stage_names()
        );
    }
    let stages = run.stage_names();
    eprintln!("valkey e2e create stages: {stages:?}");
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
    assert_eq!(run.finished_count(), 1, "exactly one OperationFinished");
    assert!(
        !run.downloads().is_empty(),
        "the download must report real progress samples"
    );
    let instance = harness.instance("vk-e2e").expect("row after create");
    let id = instance.id().clone();
    assert_eq!(instance.status(), InstanceStatus::Stopped);
    cache::seed_cache_from(&harness.paths, EngineKey::Valkey, &version);

    // Start to healthy.
    let start_run = harness.start(&id);
    super::expect_ok(&start_run, "start");
    harness.wait_status(&id, InstanceStatus::Running, Duration::from_secs(5));
    assert!(
        wait_tcp(port, Duration::from_secs(20)),
        "valkey must listen on {port}"
    );

    // Auth is required: a connection without the password must be refused.
    let bare = format!("redis://127.0.0.1:{port}/0");
    match RedisClient::connect(&bare) {
        Err(_) => {}
        Ok(mut connection) => {
            // Some clients defer auth; a command must still fail.
            let refused = connection.ping().is_err()
                || connection.set("probe", "1").is_err()
                || connection.get("probe").is_err();
            assert!(
                refused,
                "a connection without the password must be refused by requirepass"
            );
        }
    }
    eprintln!("valkey e2e: an unauthenticated connection was refused");

    // The service URI carries the generated password.
    let uri = harness.uri(&id).expose().to_owned();
    assert!(uri.starts_with("redis://"), "unexpected URI shape: {uri}");
    let mut client = RedisClient::connect(&uri).expect("connect with the service URI");
    assert_eq!(client.ping().expect("PING"), "PONG");
    client.set("e2e:key", "value-1").expect("SET");
    assert_eq!(
        client.get("e2e:key").expect("GET"),
        Some("value-1".to_owned())
    );

    // Stop/restart: AOF must carry the value across the cycle.
    let stop_run = harness.stop(&id);
    super::expect_ok(&stop_run, "stop");
    assert!(
        wait_tcp_gone(port, Duration::from_secs(20)),
        "port {port} must be released after stop"
    );
    let start_run = harness.start(&id);
    super::expect_ok(&start_run, "restart");
    harness.wait_status(&id, InstanceStatus::Running, Duration::from_secs(5));
    assert!(
        wait_tcp(port, Duration::from_secs(20)),
        "valkey must listen again"
    );
    let mut client = RedisClient::connect(harness.uri(&id).expose()).expect("reconnect");
    assert_eq!(
        client.get("e2e:key").expect("GET after restart"),
        Some("value-1".to_owned()),
        "AOF persistence must carry the value across a stop/start"
    );

    // Delete with PurgeData: the instance directory is gone and removed is emitted.
    let delete_run = harness.delete(&id, DeleteMode::PurgeData);
    super::expect_ok(&delete_run, "delete with PurgeData");
    assert!(
        delete_run.saw_removed(&id),
        "InstanceRemoved must be emitted for the deleted instance"
    );
    harness.wait_row_gone(&id, Duration::from_secs(20));
    assert!(
        !harness
            .paths
            .instance_dir(&dblocal_core::model::InstanceName::new("vk-e2e").expect("name"))
            .exists(),
        "PurgeData must remove the whole instance directory"
    );
    assert!(
        harness
            .service
            .list_archives()
            .expect("list_archives")
            .is_empty(),
        "PurgeData must not leave an archive"
    );
    assert!(
        wait_tcp_gone(port, Duration::from_secs(20)),
        "the engine must be stopped after the delete"
    );
    assert!(
        proc::engines_under(harness.home.path()).is_empty(),
        "no engine process may be left: {:?}",
        proc::engines_under(harness.home.path())
    );
}
