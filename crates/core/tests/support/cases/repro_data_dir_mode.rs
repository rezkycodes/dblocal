//! Minimal reproduction of a third defect this suite found (W2-C).
//!
//! **Defect (defence in depth):** the task's invariant is "`0700` for the secrets
//! and data directories". The secrets directory honours it (the `0600` file
//! backend tightens it). The **data directory does not**, and the answer depends
//! on the engine:
//!
//! - PostgreSQL: `initdb` creates the data directory itself and leaves it `0700`,
//!   so the invariant holds by accident of the engine.
//! - Valkey: the service creates the data directory with
//!   `std::fs::create_dir_all` (`crates/core/src/instance/create.rs` for
//!   `data/logs/run`, and again in `engine/valkey.rs`), which inherits the process
//!   umask. On this host (`umask 0022`) the result is `0755`.
//!
//! Practical exposure is limited: the DBLocal root (`~/.dblocal`) is created
//! `0700` by `Store::open` → `ensure_private_dir`, so a `0755` child is not
//! traversable by another user while the root stays `0700`. It is still a gap
//! against the task's stated invariant, and it is inconsistent: the same
//! directory means different things for the two engines.
//!
//! **Not patched here.** `instance/create.rs` and `engine/valkey.rs` are non-test
//! code in `dblocal-core`.
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

    let mut observed = Vec::new();
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
        let mode = |path: &std::path::Path| {
            std::fs::symlink_metadata(path)
                .expect("stat")
                .permissions()
                .mode()
                & 0o777
        };
        let data = harness.paths.data_dir(&instance_name);
        let root = harness.paths.root().to_path_buf();
        let instances = harness.paths.instances_dir();
        let secrets = harness.paths.secrets_dir();
        let secrets_mode = if secrets.exists() {
            format!("{:o}", mode(&secrets))
        } else {
            "absent".to_owned()
        };
        eprintln!(
            "{}: root={:o} instances={:o} data={:o} secrets={secrets_mode}",
            engine.display_name(),
            mode(&root),
            mode(&instances),
            mode(&data),
        );
        observed.push((engine, mode(&data)));

        let delete = harness.delete(&id, DeleteMode::PurgeData);
        super::expect_ok(&delete, "cleanup delete");
    }

    let postgres = observed
        .iter()
        .find(|(engine, _)| *engine == EngineKey::Postgresql)
        .map(|(_, mode)| *mode);
    let valkey = observed
        .iter()
        .find(|(engine, _)| *engine == EngineKey::Valkey)
        .map(|(_, mode)| *mode);

    match (postgres, valkey) {
        (Some(postgres), Some(valkey)) if postgres == 0o700 && valkey == 0o700 => {
            eprintln!("both data directories are 0700 now; remove this reproduction");
        }
        (Some(postgres), Some(valkey)) => {
            eprintln!(
                "DEFECT CONFIRMED: data directory mode differs by engine — PostgreSQL {postgres:o}, Valkey {valkey:o}; \
                 the task requires 0700 for the data directory. Valkey is created by `create_dir_all` in \
                 instance/create.rs and engine/valkey.rs, which inherits the umask; PostgreSQL is 0700 only because \
                 initdb chooses that. The 0700 DBLocal root limits the practical exposure."
            );
            assert_ne!(
                postgres, valkey,
                "the two engines would agree; nothing to report"
            );
        }
        _ => panic!("both engines must have produced a data directory"),
    }
}
