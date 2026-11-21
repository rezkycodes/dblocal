//! Two `LocalService` instances over one home, racing to start the same stopped
//! instance: the store's compare-and-set must let exactly one claim it, so the
//! CLI's "sudah berjalan" path is not merely a local check (F-CLI-02 AC3).
//!
//! No engine runs here: the assertion is on which service accepted the start,
//! not on a healthy process.

use std::sync::{Arc, Barrier};

use dblocal_core::model::{AuthMode, EngineKey, InstanceName, InstanceSpec, InstanceStatus, Port};
use dblocal_core::paths::DblPaths;
use dblocal_core::service::{DblService, LocalService, ServiceOptions};

const ROUNDS: usize = 20;

#[test]
fn two_services_on_one_home_accept_exactly_one_start() {
    // Many independent rounds, because a single barrier release can get lucky
    // (the first 40-run sweep found the `starting -> starting` wording only
    // once). Each round gets its own name and port so rows cannot interfere.
    for round in 0..ROUNDS {
        race_one_round(round);
    }
}

fn race_one_round(round: usize) {
    let dir = tempfile::TempDir::new().expect("temp home");
    let paths = DblPaths::with_root(dir.path());

    let first = LocalService::new(paths.clone(), ServiceOptions::default()).expect("first service");
    let second =
        LocalService::new(paths.clone(), ServiceOptions::default()).expect("second service");

    let spec = InstanceSpec::new(
        InstanceName::new(format!("shared-{round:02}")).expect("name"),
        EngineKey::Postgresql,
        "17.4",
        Some(Port::new(20451 + round as u16).expect("port")),
        AuthMode::None,
    )
    .expect("spec");
    // Insert the row directly as `Stopped`, bypassing create so the test needs no
    // runtime, adapter or network. This is the row shape a real create leaves.
    let instance = dblocal_core::model::Instance::new(
        &spec,
        dblocal_core::model::InstanceId::new(),
        InstanceStatus::Stopped,
        std::time::SystemTime::now(),
    );
    dblocal_core::store::Store::open(&paths)
        .expect("store")
        .insert_instance(&instance)
        .expect("insert row");

    let barrier = Arc::new(Barrier::new(2));
    let a = Arc::new(first);
    let b = Arc::new(second);
    let id = instance.id().clone();

    let run =
        |service: Arc<LocalService>, barrier: Arc<Barrier>, id: dblocal_core::model::InstanceId| {
            std::thread::spawn(move || {
                barrier.wait();
                service.start_instance(&id).map(|op| op.as_str().to_owned())
            })
        };
    let a_handle = run(Arc::clone(&a), Arc::clone(&barrier), id.clone());
    let b_handle = run(Arc::clone(&b), Arc::clone(&barrier), id.clone());

    let a_result = a_handle.join().expect("thread a");
    let b_result = b_handle.join().expect("thread b");

    let accepted = [&a_result, &b_result].iter().filter(|r| r.is_ok()).count();
    assert_eq!(
        accepted, 1,
        "exactly one start may claim the row: a={a_result:?} b={b_result:?}"
    );
    let loser = [&a_result, &b_result]
        .into_iter()
        .find(|r| r.is_err())
        .expect("a loser");
    let err = loser.as_ref().expect_err("loser error");
    assert_eq!(err.code(), "DBL-1004");
    // `busy` is what a loser sees when the row moved between its read and the
    // write; a loser that read the row just after the winner persisted
    // `Starting` sees `starting -> starting`. Both are refusals, and the CLI
    // treats both as "sudah berjalan".
    let message = err.to_string();
    assert!(
        message.contains("busy") || message.contains("starting"),
        "the loser must report a refusal: {message}"
    );
}
