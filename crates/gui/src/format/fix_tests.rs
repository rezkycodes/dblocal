//! W4-H fix-round `offers_restore` tests (`M5`, `M6`): the Overview banner's
//! visibility, split out of `format/tests.rs` to keep both under the size cap.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use dblocal_core::model::{
    AuthMode, EngineKey, InstanceId, InstanceName, InstanceSpec, InstanceStatus, Port, StartFailure,
};
use dblocal_core::service::{FailureInfo, InstanceView};
use dblocal_core::strings;

use super::*;

fn view(name: &str, engine: EngineKey, status: InstanceStatus, port: u16) -> InstanceView {
    let spec = InstanceSpec::new(
        InstanceName::new(name).expect("valid name"),
        engine,
        "17.4",
        Some(Port::new(port).expect("valid port")),
        AuthMode::Password,
    )
    .expect("spec");
    let now = std::time::SystemTime::UNIX_EPOCH;
    let instance = dblocal_core::model::Instance::new(&spec, InstanceId::new(), status, now);
    InstanceView::from_instance(&instance)
}

/// `M5`: only a `Failed` instance offers the restore, even if it still carries
/// a failure object with `restore_available` after it was restarted.
#[test]
fn only_a_failed_instance_offers_the_restore_banner() {
    let failure = || FailureInfo {
        code: "DBL-8601".to_owned(),
        message: "config changed".to_owned(),
        diagnosis: StartFailure::Unknown,
        log_tail: vec![],
        restore_available: true,
    };
    for status in [InstanceStatus::Running, InstanceStatus::Stopped] {
        let view = view("pg", EngineKey::Postgresql, status, 5432).with_failure(failure());
        assert!(
            !offers_restore(&view),
            "M5: a {status:?} instance must not offer the restore banner"
        );
    }
    let failed =
        view("pg", EngineKey::Postgresql, InstanceStatus::Failed, 5432).with_failure(failure());
    assert!(offers_restore(&failed), "M5: a Failed one does");
}

/// `M6`: the restore banner honours `failure.restore_available`. A port conflict
/// is a failure with no backup to restore, so no banner.
#[test]
fn a_failure_without_a_restore_offers_no_banner() {
    let port_conflict = view("shop", EngineKey::Postgresql, InstanceStatus::Failed, 5434)
        .with_failure(FailureInfo {
            code: "DBL-1005".to_owned(),
            message: strings::ERR_PORT_IN_USE.to_owned(),
            diagnosis: StartFailure::PortInUse,
            log_tail: vec![],
            restore_available: false,
        });
    assert!(
        !offers_restore(&port_conflict),
        "M6: a failure with restore_available == false must not offer the restore"
    );
    let with_backup =
        view("pg", EngineKey::Postgresql, InstanceStatus::Failed, 5432).with_failure(FailureInfo {
            code: "DBL-4001".to_owned(),
            message: strings::ERR_ENGINE.to_owned(),
            diagnosis: StartFailure::Unknown,
            log_tail: vec![],
            restore_available: true,
        });
    assert!(offers_restore(&with_backup));
}
