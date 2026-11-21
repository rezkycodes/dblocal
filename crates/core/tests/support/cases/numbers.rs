//! Case 8 — the numbers the PRD targets are checked against.
//!
//! Prints, does not assert: create split into download and initialise, start to
//! healthy, stop, and sizes on disk (installed runtime, freshly initialised empty
//! data directory). `D-02` targets under two minutes for the first session
//! including the download, so the pieces are recorded separately rather than as
//! one flaky timing assertion.
//!
//! PostgreSQL is reported honestly: its download and extraction cannot complete
//! because of the hard-link defect (`repro_hardlink`), so its create timing is
//! measured over a runtime materialised from the cached archive and the printed
//! line says the download was skipped. Valkey does a real HTTPS download.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Instant;

use dblocal_core::model::{DeleteMode, EngineKey, InstanceStatus};

use crate::support::cache;
use crate::support::clients::wait_tcp;
use crate::support::harness::{Harness, Timings, dir_size, newest_spec, newest_version};
use crate::support::{ports, proc};

#[test]
#[ignore = "e2e: network + several hundred MB of disk; run with DBLOCAL_E2E=1 cargo xtask e2e"]
fn timings_and_sizes_are_reported() {
    super::require_e2e();

    for engine in [EngineKey::Postgresql, EngineKey::Valkey] {
        let harness = Harness::new(&format!("dblocal-e2e-numbers-{}-", engine.as_str()));
        let version = newest_version(&harness.service, engine);
        // PostgreSQL: the runtime is materialised from the cached archive because
        // the production extractor rejects its hard links (`repro_hardlink`).
        // Valkey: nothing is seeded, so the real download runs and is timed.
        let seeded = engine == EngineKey::Postgresql
            && cache::extract_archive_to_runtime(
                &harness.paths,
                engine,
                &version,
                "postgresql-18.7.0-x86_64-unknown-linux-gnu.tar.gz",
            );
        let name = format!("{}-num", super::short(engine));
        let port = ports::reserve();
        let spec = newest_spec(&harness.service, &name, engine, port);

        let mut timings = Timings::default();
        let created_at = Instant::now();
        let run = harness.create(spec);
        super::expect_ok(&run, "create");
        timings.create_total = Some(created_at.elapsed());

        let downloading = run.at_stage(dblocal_core::service::OperationStage::Downloading);
        let initializing = run.at_stage(dblocal_core::service::OperationStage::Initializing);
        let done = run.at_stage(dblocal_core::service::OperationStage::Done);
        timings.download = match (downloading, initializing) {
            (Some(start), Some(end)) => Some(end.duration_since(start)),
            _ => None,
        };
        timings.initialize = match (initializing, done) {
            (Some(start), Some(end)) => Some(end.duration_since(start)),
            _ => None,
        };
        eprintln!(
            "{}: create stages {:?}",
            engine.display_name(),
            run.stage_names()
        );

        let id = harness.list_id(&name).expect("the created instance");
        let instance_name = dblocal_core::model::InstanceName::new(&name).expect("name");
        let data_dir = harness.paths.data_dir(&instance_name);
        timings.empty_data_bytes = dir_size(&data_dir);
        timings.runtime_bytes = harness
            .paths
            .runtime_dir(engine, &version, dblocal_core::platform::Arch::current())
            .map(|path| dir_size(&path))
            .unwrap_or(0);

        let start_at = Instant::now();
        let started = harness.start(&id);
        super::expect_ok(&started, "start");
        timings.start = Some(start_at.elapsed());
        assert!(
            wait_tcp(port, std::time::Duration::from_secs(20)),
            "{}: the engine must listen",
            engine.display_name()
        );

        let stop_at = Instant::now();
        let stopped = harness.stop(&id);
        super::expect_ok(&stopped, "stop");
        timings.stop = Some(stop_at.elapsed());
        assert_eq!(
            harness.instance_by_id(&id).expect("row").status(),
            InstanceStatus::Stopped
        );

        eprintln!("{}", timings.render(engine.display_name()));
        eprintln!(
            "{}: create {} the real HTTPS download{}",
            engine.display_name(),
            if seeded { "skipped" } else { "included" },
            if seeded {
                " (runtime materialised from the cached PostgreSQL archive: the extractor \
                  rejects its hard links, see repro_hardlink)"
            } else {
                ""
            }
        );

        let delete = harness.delete(&id, DeleteMode::PurgeData);
        super::expect_ok(&delete, "cleanup delete");
        assert!(
            proc::wait_none_under(harness.home.path(), std::time::Duration::from_secs(20)),
            "{}: no engine may remain",
            engine.display_name()
        );
    }
}
