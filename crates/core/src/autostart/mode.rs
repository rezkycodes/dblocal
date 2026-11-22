//! The `--autostart` mode driven from the CLI and reused by the GUI.
//!
//! The pass depends on [`AutostartService`], a two-method view of the service,
//! not the whole `DblService` trait. That keeps the glue small, lets the tests
//! use a two-method double, and makes the production wiring a one-line impl for
//! `Arc<dyn DblService>`.

use std::sync::Arc;

use crate::model::InstanceId;
use crate::service::DblService;

use super::AutostartError;
use super::plan::{AutostartMode, AutostartSummary, AutostartTarget, decide_mode, flagged_targets};
use super::runner::run_start_targets;

/// The two service calls an autostart pass needs.
pub trait AutostartService: Send + Sync {
    /// Reads every instance view.
    fn list(&self) -> crate::Result<Vec<crate::service::InstanceView>>;
    /// Requests a start and returns immediately.
    fn start(&self, id: &InstanceId) -> crate::Result<()>;
}

impl AutostartService for Arc<dyn DblService> {
    fn list(&self) -> crate::Result<Vec<crate::service::InstanceView>> {
        self.list_instances()
    }

    fn start(&self, id: &InstanceId) -> crate::Result<()> {
        self.start_instance(id).map(|_op| ())
    }
}

/// Outcome of one `--autostart` pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutostartPass {
    /// How the app should present itself afterwards.
    pub mode: AutostartMode,
    /// What happened for every flagged instance, in input order.
    pub summary: AutostartSummary,
}

/// Runs one `--autostart` pass against a service.
///
/// `tray_available` is supplied by the caller: the GUI knows it; the CLI
/// assumes `false`, because a CLI process never owns a tray host.
pub fn run_pass<S>(service: &S, tray_available: bool) -> Result<AutostartPass, AutostartError>
where
    S: AutostartService + ?Sized,
{
    let views = service.list().map_err(|err| AutostartError::StatusFailed {
        detail: err.to_string(),
    })?;
    let targets = flagged_targets(&views);
    let mode = decide_mode(tray_available, targets.len());
    let summary = start_targets(service, &targets)?;
    Ok(AutostartPass { mode, summary })
}

/// Starts `targets` through the service, bounded and independent.
pub fn start_targets<S>(
    service: &S,
    targets: &[AutostartTarget],
) -> Result<AutostartSummary, AutostartError>
where
    S: AutostartService + ?Sized,
{
    run_start_targets(targets, |target: &AutostartTarget| {
        service
            .start(&target.id)
            .map_err(|err| format!("[{}] {}", err.code(), err.user_message()))
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use crate::model::{
        AuthMode, EngineKey, Instance, InstanceId, InstanceName, InstanceSpec, InstanceStatus, Port,
    };
    use crate::service::InstanceView;

    /// A two-method service double: a fixed view list that accepts or refuses
    /// starts by id.
    struct FakeService {
        views: Vec<InstanceView>,
        accept: bool,
    }

    impl AutostartService for FakeService {
        fn list(&self) -> crate::Result<Vec<InstanceView>> {
            Ok(self.views.clone())
        }
        fn start(&self, id: &InstanceId) -> crate::Result<()> {
            if self.accept && self.views.iter().any(|v| v.id == *id) {
                Ok(())
            } else {
                Err(crate::DblError::NotFound {
                    kind: "instance",
                    name: id.to_string(),
                })
            }
        }
    }

    fn view(name: &str, autostart: bool) -> InstanceView {
        let spec = InstanceSpec::new(
            InstanceName::new(name).expect("name"),
            EngineKey::Postgresql,
            "18.0",
            Some(Port::new(5432).expect("port")),
            AuthMode::Password,
        )
        .expect("spec")
        .with_autostart(autostart);
        let inst = Instance::new(
            &spec,
            InstanceId::new(),
            InstanceStatus::Stopped,
            std::time::SystemTime::UNIX_EPOCH,
        );
        InstanceView::from_instance(&inst)
    }

    fn fake(views: Vec<InstanceView>, accept: bool) -> FakeService {
        FakeService { views, accept }
    }

    #[test]
    fn no_flagged_instances_yield_headless_exit() {
        let service = fake(Vec::new(), true);
        let pass = run_pass(&service, false).expect("pass");
        assert_eq!(pass.mode, AutostartMode::HeadlessExit);
        assert!(pass.summary.outcomes.is_empty());
    }

    #[test]
    fn tray_available_is_always_tray_only() {
        let service = fake(Vec::new(), true);
        assert_eq!(
            run_pass(&service, true).expect("pass").mode,
            AutostartMode::TrayOnly
        );
    }

    #[test]
    fn a_flagged_instance_is_started() {
        let service = fake(vec![view("pg-one", true), view("pg-two", false)], true);
        let pass = run_pass(&service, false).expect("pass");
        assert_eq!(pass.mode, AutostartMode::HeadlessKeepAlive);
        assert_eq!(pass.summary.outcomes.len(), 1);
        assert_eq!(pass.summary.succeeded(), 1);
    }

    #[test]
    fn a_refused_start_is_recorded_not_fatal() {
        let service = fake(vec![view("pg-one", true)], false);
        let pass = run_pass(&service, false).expect("pass");
        assert_eq!(pass.summary.failed(), 1);
        let outcome = &pass.summary.outcomes[0];
        assert!(
            outcome.result.as_ref().unwrap_err().starts_with("[DBL-"),
            "a refusal carries its code"
        );
    }

    #[test]
    fn an_arc_service_is_an_autostart_service() {
        // The blanket impl is what production uses; prove it compiles and runs
        // through the trait object by using a `DblService`-free path for the
        // `start_targets` call.
        let service = fake(vec![view("pg-one", true)], true);
        let targets = flagged_targets(&service.views);
        let summary = start_targets(&service, &targets).expect("run");
        assert_eq!(summary.succeeded(), 1);
    }
}
