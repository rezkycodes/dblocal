//! Crash-recovery data attribution: which archive directory belongs to which
//! instance, and when a second archive for a recreated name must be recorded.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]

use std::time::SystemTime;

use crate::model::{ArchivedData, EngineKey, InstanceStatus};
use crate::service::DblService;

use super::harness::{Harness, test_port};

/// Creates a stopped instance named `name` and returns its id.
fn create(h: &Harness, name: &str) -> crate::model::Instance {
    let port = test_port();
    let spec = h.spec(name, EngineKey::Postgresql, port);
    h.create(spec);
    h.instances()
        .into_iter()
        .find(|instance| instance.name().as_str() == name)
        .expect("created instance")
}

/// Moves `id` into a crashed `Deleting` state with its data directory gone, the
/// shape a delete leaves when it died after the move and before the record.
fn crashed_after_move(h: &Harness, id: &crate::model::InstanceId, name: &str) {
    // Data gone (what a purge or a completed rename leaves), row still Deleting.
    let data_dir = h
        .paths
        .data_dir(&crate::model::InstanceName::new(name).expect("name"));
    let _ = std::fs::remove_dir_all(data_dir);
    h.store()
        .transition_status(id, InstanceStatus::Stopped, InstanceStatus::Deleting)
        .expect("deleting");
}

#[test]
fn archive_recovery_never_claims_a_longer_name_directory() {
    // `pg` must not claim `pg-main`'s archive directory: the remainder after
    // `pg-` is `main-...`, which is not a stamp. Each instance recovers only
    // `<its name>-<digits>`.
    let h = Harness::new(EngineKey::Postgresql);
    let archive_root = h.paths.archive_dir();
    std::fs::create_dir_all(&archive_root).expect("archive root");

    let pg = create(&h, "pg");
    let pg_main = create(&h, "pg-main");

    let pg_dir = archive_root.join("pg-1700000000000");
    let main_dir = archive_root.join("pg-main-1700000000001");
    std::fs::create_dir_all(&pg_dir).expect("pg dir");
    std::fs::create_dir_all(&main_dir).expect("main dir");
    std::fs::write(pg_dir.join("pg.marker"), b"pg").expect("pg marker");
    std::fs::write(main_dir.join("main.marker"), b"main").expect("main marker");

    // `pg-main`'s directory is deliberately **not** recorded: only the exact
    // name-plus-digits rule can stop `pg` from claiming it. A recorded row would
    // mask the bug through the "already taken" filter instead.
    let _ = &pg_main;

    // A crashed delete of `pg`: data gone, archive directory present but not
    // recorded. Recovery must register `pg`'s own directory only.
    crashed_after_move(&h, pg.id(), "pg");

    let restarted = super::harness::restart(&h.paths, std::sync::Arc::clone(&h.procs));
    super::harness::wait_until_row_gone(&restarted, pg.id(), 1);

    let archives = restarted.service.list_archives().expect("archives");
    let for_pg: Vec<_> = archives
        .iter()
        .filter(|archive| archive.instance_name() == pg.name())
        .collect();
    assert_eq!(
        for_pg.len(),
        1,
        "pg recovers exactly one archive: {archives:?}"
    );
    assert_eq!(
        super::super::helpers::normalize(for_pg[0].path()),
        super::super::helpers::normalize(&pg_dir),
        "pg recovers the directory named for pg, never pg-main's"
    );
    assert!(
        !for_pg.iter().any(|archive| {
            super::super::helpers::normalize(archive.path())
                == super::super::helpers::normalize(&main_dir)
        }),
        "pg-main's archive directory must not be claimed by pg"
    );
    // pg-main's directory is untouched.
    assert!(
        main_dir.join("main.marker").exists(),
        "pg-main data survives"
    );
    assert!(pg_dir.join("pg.marker").exists(), "pg data survives");
}

#[test]
fn archive_recovery_records_nothing_when_no_own_directory_exists() {
    // A crashed delete of `pg` whose own archive directory never appeared (a
    // purge that finished, or a rename that never ran) must not invent an
    // archive, and must certainly not adopt a neighbour's directory.
    let h = Harness::new(EngineKey::Postgresql);
    let archive_root = h.paths.archive_dir();
    std::fs::create_dir_all(&archive_root).expect("archive root");
    // A neighbour's directory, which `pg` must ignore.
    let neighbour = archive_root.join("pg-main-1700000000000");
    std::fs::create_dir_all(&neighbour).expect("neighbour dir");

    let pg = create(&h, "pg");
    crashed_after_move(&h, pg.id(), "pg");

    let restarted = super::harness::restart(&h.paths, std::sync::Arc::clone(&h.procs));
    super::harness::wait_until_row_gone(&restarted, pg.id(), 2);

    let archives = restarted.service.list_archives().expect("archives");
    assert!(
        archives.is_empty(),
        "no archive is invented when no own directory exists, saw {archives:?}"
    );
    assert!(neighbour.exists(), "the neighbour directory is untouched");
}

#[test]
fn a_recreated_name_records_its_own_second_archive() {
    // Delete `xx` (archived, recorded), create a new `xx`, and crash its delete
    // after the rename. `recorded` must be decided by the stable archive id, not
    // by the name, or the second archive would sit in archive/ with no row.
    let h = Harness::new(EngineKey::Postgresql);
    let archive_root = h.paths.archive_dir();
    std::fs::create_dir_all(&archive_root).expect("archive root");

    let first = create(&h, "xx");
    // A previous archived `xx`: its row exists and points at its own directory.
    let first_dir = archive_root.join("xx-1600000000000");
    std::fs::create_dir_all(&first_dir).expect("first dir");
    std::fs::write(first_dir.join("first.marker"), b"first").expect("first marker");
    h.store()
        .add_archive(&ArchivedData::new(
            "arch-first-xx",
            first.name().clone(),
            EngineKey::Postgresql,
            "18.7",
            first_dir.clone(),
            1,
            SystemTime::UNIX_EPOCH,
        ))
        .expect("record first xx");

    // Delete the first instance fully so the name is free.
    let (_op, run) = h.run_call(|| {
        h.service
            .delete_instance(first.id(), crate::model::DeleteMode::PurgeData)
    });
    assert!(
        matches!(run.result(), Some(Ok(()))),
        "delete first: {:?}",
        run.result()
    );
    // Remove the archived row's *directory* record so only the row remains? No:
    // keep the record, that is the "same name earlier" case. Recreate `xx`.
    let second = create(&h, "xx");
    assert_ne!(second.id(), first.id());

    // The second instance's own archive directory exists (the rename ran), but
    // the row was never written because the delete crashed.
    let second_dir = archive_root.join("xx-1700000000002");
    std::fs::create_dir_all(&second_dir).expect("second dir");
    std::fs::write(second_dir.join("second.marker"), b"second").expect("second marker");
    crashed_after_move(&h, second.id(), "xx");

    let restarted = super::harness::restart(&h.paths, std::sync::Arc::clone(&h.procs));
    super::harness::wait_until_row_gone(&restarted, second.id(), 3);

    let archives = restarted.service.list_archives().expect("archives");
    // Both rows now exist: the first `x` archive, and the newly recovered one
    // for the recreated `x`.
    assert_eq!(
        archives.len(),
        2,
        "the recreated xx records its own archive too: {archives:?}"
    );
    let recovered = archives
        .iter()
        .find(|archive| {
            super::super::helpers::normalize(archive.path())
                == super::super::helpers::normalize(&second_dir)
        })
        .expect("the second xx archive is recorded");
    assert_eq!(
        recovered.id(),
        format!(
            "arch-{}",
            second
                .id()
                .as_str()
                .get(..8)
                .unwrap_or(second.id().as_str())
        ),
        "the recovered row uses the stable id of the recreated instance"
    );
    assert!(
        archives
            .iter()
            .any(|archive| archive.id() == "arch-first-xx"),
        "the first xx archive row is untouched"
    );
}

#[test]
fn recovery_skips_a_recorded_directory_that_sorts_last() {
    // The `!taken.contains(..)` filter must be load-bearing, not just covered by
    // ordering. Here the directory an existing row already points at sorts
    // AFTER the crashed delete's own directory, so `sort(); pop()` would pick the
    // recorded one unless the filter removes it first. Recovery must record the
    // crashed delete's own (earlier-stamped) directory and must not create a
    // second row for the recorded one.
    //
    // Mutation check (R10): delete the `.filter(|path| !taken.contains(..))`
    // line in `reconcile.rs::unrecorded_archive` and this test fails with
    //   thread '...recovery_skips_a_recorded_directory_that_sorts_last'
    //   panicked at crates/core/src/instance/tests/archive_recovery.rs:269:10:
    //   the crashed delete's own directory is recorded
    // because `sort(); pop()` then picks the recorded `xx-1700000000009`.
    let h = Harness::new(EngineKey::Postgresql);
    let archive_root = h.paths.archive_dir();
    std::fs::create_dir_all(&archive_root).expect("archive root");

    let xx = create(&h, "xx");

    // A directory an existing row already points at, whose stamp sorts LAST.
    let recorded_dir = archive_root.join("xx-1700000000009");
    std::fs::create_dir_all(&recorded_dir).expect("recorded dir");
    std::fs::write(recorded_dir.join("recorded.marker"), b"recorded").expect("marker");
    h.store()
        .add_archive(&ArchivedData::new(
            "arch-already-there",
            xx.name().clone(),
            EngineKey::Postgresql,
            "18.7",
            recorded_dir.clone(),
            1,
            SystemTime::UNIX_EPOCH,
        ))
        .expect("record existing archive");

    // The crashed delete's own archive directory, stamped BEFORE the recorded
    // one, so plain `sort(); pop()` would wrongly choose the recorded one.
    let own_dir = archive_root.join("xx-1600000000001");
    std::fs::create_dir_all(&own_dir).expect("own dir");
    std::fs::write(own_dir.join("own.marker"), b"own").expect("marker");

    // Crash the delete of `xx` after the rename: data gone, row still Deleting,
    // own archive directory unrecorded.
    crashed_after_move(&h, xx.id(), "xx");

    let restarted = super::harness::restart(&h.paths, std::sync::Arc::clone(&h.procs));
    super::harness::wait_until_row_gone(&restarted, xx.id(), 4);

    let archives = restarted.service.list_archives().expect("archives");
    // Exactly two rows: the pre-existing one and the recovered own directory.
    assert_eq!(
        archives.len(),
        2,
        "recovery adds its own directory and no duplicate: {archives:?}"
    );
    let recovered = archives
        .iter()
        .find(|archive| archive.id().starts_with("arch-"))
        .filter(|archive| {
            super::super::helpers::normalize(archive.path())
                == super::super::helpers::normalize(&own_dir)
        })
        .expect("the crashed delete's own directory is recorded");
    assert_eq!(
        super::super::helpers::normalize(recovered.path()),
        super::super::helpers::normalize(&own_dir),
        "recovery recorded the already-recorded directory instead of its own"
    );
    // No second row for the recorded directory.
    let for_recorded = archives
        .iter()
        .filter(|archive| {
            super::super::helpers::normalize(archive.path())
                == super::super::helpers::normalize(&recorded_dir)
        })
        .count();
    assert_eq!(
        for_recorded, 1,
        "the already-recorded directory keeps exactly one row"
    );
    assert!(own_dir.join("own.marker").exists(), "own data survives");
    assert!(
        recorded_dir.join("recorded.marker").exists(),
        "recorded data survives"
    );
}
