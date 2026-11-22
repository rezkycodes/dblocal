//! Autostart of chosen instances (`F-AUT-02`), on a real PostgreSQL.
//!
//! Two instances are flagged `autostart`; one of them has its port taken by a
//! foreign listener before the pass runs. The independent pass must start the
//! free one and record the blocked one as failed with a `DBL-1005` diagnosis,
//! without the failure stopping the other (AC2) and without any modal dialog
//! (the failure is the ordinary `failed` status the tray and window render).
//!
//! Run with:
//!
//! ```sh
//! export CARGO_TARGET_DIR="$HOME/.cache/dblocal-target"
//! flock -w 3600 "$HOME/.cache/dblocal-e2e.lock" \
//!   cargo test -p dblocal-core --test service_e2e -- --ignored --nocapture \
//!   autostart_two_flagged_one_blocked
//! ```
//!
//! Ports come from the shared 20000-29999 allocator in `support::ports`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use dblocal_core::autostart::{flagged_targets, start_targets};
use dblocal_core::error::CODE_PORT_IN_USE;
use dblocal_core::model::{DeleteMode, EngineKey, InstanceStatus};
use dblocal_core::service::DblService;

use crate::support::cache;
use crate::support::harness::{Harness, newest_spec, newest_version};
use crate::support::ports;

#[test]
#[ignore = "e2e: real PostgreSQL, needs the cached archive and free ports in 20000-29999"]
fn autostart_two_flagged_one_blocked() {
    super::require_e2e();

    let harness = Harness::new("dblocal-e2e-autostart-");
    let version = newest_version(&harness.service, EngineKey::Postgresql);
    assert!(
        cache::extract_archive_to_runtime(
            &harness.paths,
            EngineKey::Postgresql,
            &version,
            "postgresql-18.7.0-x86_64-unknown-linux-gnu.tar.gz",
        ),
        "needs the PostgreSQL archive cached in ~/.cache/dblocal-dev"
    );

    let free_port = ports::reserve();
    let blocked_port = ports::reserve();
    let spec_a = newest_spec(
        &harness.service,
        "auto-ok",
        EngineKey::Postgresql,
        free_port,
    );
    let spec_b = newest_spec(
        &harness.service,
        "auto-blocked",
        EngineKey::Postgresql,
        blocked_port,
    );
    let created_a = harness.create(spec_a);
    super::expect_ok(&created_a, "create auto-ok");
    let created_b = harness.create(spec_b);
    super::expect_ok(&created_b, "create auto-blocked");

    let id_a = harness.instance("auto-ok").expect("row a").id().clone();
    let id_b = harness
        .instance("auto-blocked")
        .expect("row b")
        .id()
        .clone();
    harness.service.set_autostart(&id_a, true).expect("flag a");
    harness.service.set_autostart(&id_b, true).expect("flag b");

    let foreign = ports::hold(blocked_port);
    eprintln!("a foreign listener holds auto-blocked's port {blocked_port}");

    // The same code the app runs at login.
    let views = harness.service.list_instances().expect("list");
    let targets = flagged_targets(&views);
    let names: Vec<&str> = targets.iter().map(|t| t.name.as_str()).collect();
    assert!(
        names.contains(&"auto-ok") && names.contains(&"auto-blocked"),
        "both flagged instances must be selected: {names:?}"
    );
    let service: Arc<dyn DblService> = Arc::new(harness.service.clone());
    let summary = start_targets(&service, &targets).expect("start pass");
    assert_eq!(
        summary.outcomes.len(),
        targets.len(),
        "one outcome per target"
    );

    // The free instance reaches Running; the blocked one ends Failed with the
    // port diagnosis. Neither failure blocked the other (AC2).
    harness.wait_status(&id_a, InstanceStatus::Running, Duration::from_secs(120));
    harness.wait_status(&id_b, InstanceStatus::Failed, Duration::from_secs(120));
    let view_b = harness.view("auto-blocked").expect("auto-blocked view");
    let info = view_b
        .failure
        .as_ref()
        .expect("a failed autostart must record a failure");
    eprintln!(
        "auto-blocked failed with [{}] {} ({:?})",
        info.code, info.message, info.diagnosis
    );
    assert_eq!(
        info.code, CODE_PORT_IN_USE,
        "the blocked start must report DBL-1005"
    );

    let blocked_outcome = summary
        .outcomes
        .iter()
        .find(|o| o.target.name == "auto-blocked")
        .expect("blocked outcome");
    assert!(
        blocked_outcome.is_err(),
        "the blocked start must be recorded as an error"
    );
    let ok_outcome = summary
        .outcomes
        .iter()
        .find(|o| o.target.name == "auto-ok")
        .expect("ok outcome");
    assert!(ok_outcome.result.is_ok(), "the free start must succeed");

    // Cleanup.
    drop(foreign);
    let stopped = harness.stop(&id_a);
    super::expect_ok(&stopped, "stop auto-ok");
    for id in [&id_a, &id_b] {
        let deleted = harness.delete(id, DeleteMode::PurgeData);
        super::expect_ok(&deleted, "delete");
    }
    assert!(
        crate::support::proc::wait_none_under(harness.home.path(), Duration::from_secs(20)),
        "no engine may remain"
    );
}
