//! Regression tests for D-3 (W2-F): data/run/logs/instances dirs must be 0700.
//!
//! The security invariant requires data and secrets directories to be 0700 on
//! Unix. PostgreSQL's data dir was 0700 only because initdb chose it; Valkey and
//! the service-created instance layout dirs inherited the process umask.
//!
//! Windows has no per-file mode bits, so this module is compiled only on Unix
//! (the declaration in `cases/mod.rs` carries the `#[cfg(unix)]`). It stays in
//! the crate so the regression proof exists where the invariant does.
//!
//! Run:
//!
//! ```sh
//! DBLOCAL_E2E=1 cargo test -p dblocal-core --test service_e2e -- --ignored \
//!   --nocapture repro::data_dir_mode
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::os::unix::fs::PermissionsExt;

use dblocal_core::model::{DeleteMode, EngineKey, InstanceName};

use crate::support::cache;
use crate::support::harness::{Harness, newest_spec, newest_version};
use crate::support::ports;

#[test]
#[ignore = "e2e: run with DBLOCAL_E2E=1 cargo test --test service_e2e -- --ignored repro::data_dir_mode"]
fn repro_data_directory_mode_depends_on_the_engine() {
    super::require_e2e();

    let mode = |path: &std::path::Path| {
        std::fs::symlink_metadata(path)
            .expect("stat")
            .permissions()
            .mode()
            & 0o777
    };

    for engine in [EngineKey::Postgresql, EngineKey::Valkey] {
        let harness = Harness::new(&format!("dblocal-e2e-mode-{}-", engine.as_str()));
        let version = newest_version(&harness.service, engine);
        let seeded = if engine == EngineKey::Postgresql {
            cache::extract_archive_to_runtime(
                &harness.paths,
                engine,
                &version,
                "postgresql-18.7.0-x86_64-unknown-linux-gnu.tar.gz",
            )
        } else {
            cache::seed_runtime(&harness.paths, engine, &version)
        };
        assert!(
            seeded,
            "{}: needs a cached artifact in ~/.cache/dblocal-dev",
            engine.display_name()
        );

        let name = format!("{}-mode", crate::support::cases::short(engine));
        let port = ports::reserve();
        let spec = newest_spec(&harness.service, &name, engine, port);
        let created = harness.create(spec);
        super::expect_ok(&created, "create");
        let id = harness.instance(&name).expect("row").id().clone();
        let instance_name = InstanceName::new(&name).expect("name");

        let data = harness.paths.data_dir(&instance_name);
        let logs = harness.paths.logs_dir(&instance_name);
        let run = harness.paths.run_dir(&instance_name);
        let instances = harness.paths.instances_dir();
        for (label, path) in [
            ("instances", instances.as_path()),
            ("data", data.as_path()),
            ("logs", logs.as_path()),
            ("run", run.as_path()),
        ] {
            assert_eq!(
                mode(path),
                0o700,
                "{} {label} dir must be 0700, got {:o}",
                engine.display_name(),
                mode(path)
            );
        }

        let delete = harness.delete(&id, DeleteMode::PurgeData);
        super::expect_ok(&delete, "cleanup delete");
    }
}
