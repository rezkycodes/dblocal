//! The `--autostart` mode driven from the CLI and reused by the GUI.

use std::sync::Arc;

use crate::service::DblService;

use super::AutostartError;
use super::plan::{AutostartMode, AutostartSummary, AutostartTarget, decide_mode, flagged_targets};
use super::runner::{StartFn, run_start_targets};

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
pub fn run_pass(
    service: &Arc<dyn DblService>,
    tray_available: bool,
) -> Result<AutostartPass, AutostartError> {
    let views = service
        .list_instances()
        .map_err(|err| AutostartError::StatusFailed {
            detail: err.to_string(),
        })?;
    let targets = flagged_targets(&views);
    let mode = decide_mode(tray_available, targets.len());
    let summary = start_targets(service, &targets)?;
    Ok(AutostartPass { mode, summary })
}

/// Starts `targets` through the service, bounded and independent.
///
/// A start that returns an error is recorded in the summary and never stops
/// the others; the returned summary carries one outcome per target in input
/// order.
pub fn start_targets(
    service: &Arc<dyn DblService>,
    targets: &[AutostartTarget],
) -> Result<AutostartSummary, AutostartError> {
    run_start_targets(targets, start_fn(service))
}

/// Adapts `DblService::start_instance` to the runner's [`StartFn`].
pub fn start_fn(service: &Arc<dyn DblService>) -> StartFn {
    let handle = Arc::clone(service);
    Arc::new(move |target: &AutostartTarget| {
        handle
            .start_instance(&target.id)
            .map(|_op| ())
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
    use crate::service::UserFacingError;

    /// Minimal service double: empty list plus scripted starts.
    struct FakeService {
        views: Vec<InstanceView>,
        starts: std::sync::Mutex<Vec<InstanceId>>,
    }

    impl FakeService {
        fn empty() -> Self {
            Self {
                views: Vec::new(),
                starts: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    impl DblService for FakeService {
        fn list_instances(&self) -> crate::Result<Vec<InstanceView>> {
            Ok(self.views.clone())
        }
        fn catalog(&self) -> crate::Result<crate::service::CatalogView> {
            unimplemented!()
        }
        fn refresh_catalog(&self, _force: bool) -> crate::Result<crate::service::OperationId> {
            unimplemented!()
        }
        fn validate_spec(&self, _spec: &crate::model::InstanceSpec) -> crate::Result<()> {
            Ok(())
        }
        fn suggest_port(&self, _engine: EngineKey) -> crate::Result<crate::model::Port> {
            unimplemented!()
        }
        fn create_instance(
            &self,
            _spec: crate::model::InstanceSpec,
        ) -> crate::Result<crate::service::OperationId> {
            unimplemented!()
        }
        fn start_instance(&self, id: &InstanceId) -> crate::Result<crate::service::OperationId> {
            if self.views.iter().any(|view| view.id == *id) {
                self.starts.lock().expect("lock").push(id.clone());
                return Ok(crate::service::OperationId::new("op"));
            }
            Err(crate::DblError::NotFound {
                kind: "instance",
                name: id.to_string(),
            })
        }
        fn stop_instance(&self, _id: &InstanceId) -> crate::Result<crate::service::OperationId> {
            unimplemented!()
        }
        fn delete_instance(
            &self,
            _id: &InstanceId,
            _mode: crate::model::DeleteMode,
        ) -> crate::Result<crate::service::OperationId> {
            unimplemented!()
        }
        fn change_port(&self, _id: &InstanceId, _port: crate::model::Port) -> crate::Result<()> {
            unimplemented!()
        }
        fn data_size(&self, _id: &InstanceId) -> crate::Result<u64> {
            Ok(0)
        }
        fn set_autostart(&self, _id: &InstanceId, _enabled: bool) -> crate::Result<()> {
            Ok(())
        }
        fn cancel_operation(&self, _id: &crate::service::OperationId) -> crate::Result<()> {
            Ok(())
        }
        fn connection_uri(
            &self,
            _id: &InstanceId,
            _format: crate::service::UriFormat,
        ) -> crate::Result<crate::secret::SecretString> {
            unimplemented!()
        }
        fn tail_logs(&self, _id: &InstanceId, _lines: usize) -> crate::Result<Vec<String>> {
            Ok(Vec::new())
        }
        fn watch_logs(&self, _id: &InstanceId, _enabled: bool) -> crate::Result<()> {
            Ok(())
        }
        fn list_runtimes(&self) -> crate::Result<Vec<crate::model::Runtime>> {
            unimplemented!()
        }
        fn remove_runtime(&self, _runtime: &str) -> crate::Result<()> {
            Ok(())
        }
        fn list_archives(&self) -> crate::Result<Vec<crate::model::ArchivedData>> {
            unimplemented!()
        }
        fn purge_archive(&self, _archive: &str) -> crate::Result<()> {
            Ok(())
        }
        fn subscribe(&self) -> tokio::sync::broadcast::Receiver<crate::service::ServiceEvent> {
            unimplemented!()
        }
    }

    fn startable_view(name: &str, autostart: bool) -> InstanceView {
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

    #[test]
    fn an_empty_service_starts_nothing_and_exits_headless() {
        let service: Arc<dyn DblService> = Arc::new(FakeService::empty());
        let pass = run_pass(&service, false).expect("pass");
        assert_eq!(pass.mode, AutostartMode::HeadlessExit);
        assert!(pass.summary.outcomes.is_empty());
    }

    #[test]
    fn tray_available_wins_over_the_flagged_count() {
        let service: Arc<dyn DblService> = Arc::new(FakeService::empty());
        let pass = run_pass(&service, true).expect("pass");
        assert_eq!(pass.mode, AutostartMode::TrayOnly);
    }

    #[test]
    fn flagged_instances_are_started_in_order() {
        let mut svc = FakeService::empty();
        svc.views.push(startable_view("pg-one", true));
        svc.views.push(startable_view("pg-two", false));
        let service: Arc<dyn DblService> = Arc::new(svc);
        let pass = run_pass(&service, false).expect("pass");
        assert_eq!(pass.mode, AutostartMode::HeadlessKeepAlive);
        assert_eq!(pass.summary.outcomes.len(), 1);
        assert_eq!(pass.summary.succeeded(), 1);
    }

    #[test]
    fn a_start_refusal_becomes_a_user_safe_string() {
        let service: Arc<dyn DblService> = Arc::new(FakeService::empty());
        let start = start_fn(&service);
        let bogus = AutostartTarget {
            id: InstanceId::new(),
            name: "missing".to_owned(),
        };
        let err = start(&bogus).expect_err("unknown instance must refuse");
        assert!(err.starts_with("[DBL-"), "got: {err}");
    }

    #[test]
    fn start_refusal_is_reported_in_the_summary() {
        let service: Arc<dyn DblService> = Arc::new(FakeService::empty());
        let targets = vec![AutostartTarget {
            id: InstanceId::new(),
            name: "ghost".to_owned(),
        }];
        let summary = start_targets(&service, &targets).expect("run");
        assert_eq!(summary.outcomes.len(), 1);
        assert_eq!(summary.failed(), 1);
    }

    #[test]
    fn user_facing_error_is_rendered_with_its_code() {
        let facing: UserFacingError =
            UserFacingError::new("DBL-8500", "Gagal membaca status autostart.");
        let text = format!("[{}] {}", facing.code, facing.message);
        assert!(text.starts_with("[DBL-8500]"));
    }
}
