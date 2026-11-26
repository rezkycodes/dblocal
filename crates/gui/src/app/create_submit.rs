//! The create-instance submission core.
//!
//! Deliberately free of GPUI: it takes plain values (the engine, version, auth
//! mode, the two typed strings) plus a `&dyn DblService`, validates, builds a
//! spec and starts the operation. The dialog reads the `InputState`s and hands
//! the strings here, which is what makes the whole path testable without a
//! window.

use dblocal_core::catalog::VersionFilter;
use dblocal_core::model::{AuthMode, EngineKey, InstanceSpec, Port};
use dblocal_core::service::DblService;
use dblocal_core::strings;

use super::state::ActiveDownload;
use crate::format;

use gpui_kit::component::input::InputState;
use gpui_kit::{Entity, FocusHandle};

/// What the create dialog currently has selected.
#[derive(Clone)]
pub struct CreateForm {
    /// Engine chosen.
    pub engine: EngineKey,
    /// Version tag chosen (must exist in the catalog entry).
    pub version: String,
    /// Auth mode.
    pub auth: AuthMode,
    /// Suggested port for the engine, refreshed on open.
    pub suggested_port: Option<Port>,
    /// Name field state. `None` only in headless tests, which drive
    /// [`Workspace::submit_draft`] with explicit strings.
    pub name_input: Option<Entity<InputState>>,
    /// Port field state; see [`CreateForm::name_input`].
    pub port_input: Option<Entity<InputState>>,
    /// Per-engine-card keyboard focus handles, in catalog order. Owned by the
    /// form so the view and the keyboard tests share one handle per card rather
    /// than each reaching into window element state (`F-PLH-01` AC1).
    pub engine_focus: Vec<(EngineKey, FocusHandle)>,
    /// Per-version-row keyboard focus handles, for the currently selected
    /// engine's versions, in list order.
    pub version_focus: Vec<(String, FocusHandle)>,
    /// The "Only show latest versions" checkbox, checked by default.
    pub latest_only: bool,
    /// The footer "Create" button's focus handle.
    pub submit_focus: FocusHandle,
    /// The footer "Cancel" button's focus handle.
    pub cancel_focus: FocusHandle,
}

impl CreateForm {
    /// The picker filter the "Hanya versi terbaru" checkbox currently chooses.
    pub fn version_filter(&self) -> VersionFilter {
        if self.latest_only {
            VersionFilter::LatestOnly
        } else {
            VersionFilter::All
        }
    }
}

/// The values the form has, independent of any GPUI entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateDraft {
    /// Engine chosen.
    pub engine: EngineKey,
    /// Version tag chosen (must exist in the catalog entry).
    pub version: String,
    /// Auth mode.
    pub auth: AuthMode,
    /// Text in the name field.
    pub name: String,
    /// Text in the port field; empty means "let the service allocate".
    pub port_text: String,
}

/// Why a create submission was rejected.
#[derive(Debug)]
pub enum SubmitError {
    /// A field-level validation failure with its user message.
    Invalid(String),
    /// The service rejected the spec or the operation.
    Service(dblocal_core::DblError),
}

impl SubmitError {
    /// The message to show the user.
    pub fn message(&self) -> String {
        match self {
            Self::Invalid(message) => message.clone(),
            Self::Service(err) => err.user_message(),
        }
    }
}

/// Outcome of a successful submission.
#[derive(Debug, Clone)]
pub struct Submitted {
    /// The operation the service started.
    pub operation: dblocal_core::service::OperationId,
    /// The download row the status bar should track.
    pub download: ActiveDownload,
}

/// Validates and submits a draft against `instances` and `service`.
///
/// This is the single path the form goes through, so it is the place the form,
/// the CLI and the tests share. `instances` is only read for the port-conflict
/// check and the duplicate-name check the service also re-runs.
pub fn submit(
    service: &dyn DblService,
    draft: &CreateDraft,
    instances: &[dblocal_core::service::InstanceView],
) -> Result<Submitted, SubmitError> {
    let validation = format::validate_form(&draft.name, &draft.port_text, instances);
    if !validation.is_valid() {
        let message = validation
            .conflict
            .or(validation.name_error)
            .or(validation.port_error)
            .unwrap_or_else(|| strings::ERR_INVALID_INPUT.to_owned());
        return Err(SubmitError::Invalid(message));
    }
    let port = parse_port(&draft.port_text)?;
    let spec = build_spec(draft, port)?;
    if let Err(err) = service.validate_spec(&spec) {
        return Err(SubmitError::Service(err));
    }
    let operation = service
        .create_instance(spec)
        .map_err(SubmitError::Service)?;
    let download = ActiveDownload {
        operation: operation.clone(),
        title: format!("{} {}", draft.engine.display_name(), draft.version),
        progress: dblocal_core::model::DownloadProgress::new(0, 1, 0, None),
    };
    Ok(Submitted {
        operation,
        download,
    })
}

/// Parses the port field. An empty field asks the service to allocate one.
fn parse_port(text: &str) -> Result<Option<Port>, SubmitError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    match trimmed
        .parse::<u16>()
        .ok()
        .and_then(|value| Port::new(value).ok())
    {
        Some(port) => Ok(Some(port)),
        None => Err(SubmitError::Invalid(
            strings::UI_VALIDATE_PORT_LOW.to_owned(),
        )),
    }
}

fn build_spec(draft: &CreateDraft, port: Option<Port>) -> Result<InstanceSpec, SubmitError> {
    let name =
        dblocal_core::model::InstanceName::new(draft.name.clone()).map_err(SubmitError::Service)?;
    InstanceSpec::new(name, draft.engine, draft.version.clone(), port, draft.auth)
        .map_err(SubmitError::Service)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use dblocal_core::service::{DblService, mock::MockService};

    use super::*;

    fn service() -> Arc<MockService> {
        Arc::new(MockService::deterministic())
    }

    fn draft(name: &str) -> CreateDraft {
        CreateDraft {
            engine: EngineKey::Postgresql,
            version: "17.4".to_owned(),
            auth: AuthMode::Password,
            name: name.to_owned(),
            port_text: String::new(),
        }
    }

    #[test]
    fn rejects_an_invalid_name_with_the_validation_message() {
        let service = service();
        let err = submit(&*service, &draft("A"), &[]).expect_err("short name");
        assert!(matches!(err, SubmitError::Invalid(_)));
        assert_eq!(err.message(), strings::UI_VALIDATE_NAME);
    }

    #[test]
    fn rejects_a_port_below_the_minimum() {
        let service = service();
        let mut draft = draft("fine-name");
        draft.port_text = "80".to_owned();
        let err = submit(&*service, &draft, &[]).expect_err("low port");
        assert_eq!(err.message(), strings::UI_VALIDATE_PORT_LOW);
    }

    #[test]
    fn rejects_a_port_owned_by_another_instance() {
        let service = service();
        service.seed_instance(
            InstanceSpec::new(
                dblocal_core::model::InstanceName::new("taken").expect("name"),
                EngineKey::Postgresql,
                "17.4",
                Some(Port::new(5432).expect("port")),
                AuthMode::Password,
            )
            .expect("spec"),
            dblocal_core::model::InstanceStatus::Stopped,
        );
        let instances = service.list_instances().expect("list");
        let mut draft = draft("fresh-name");
        draft.port_text = "5432".to_owned();
        let err = submit(&*service, &draft, &instances).expect_err("owned port");
        assert!(err.message().contains("5432"));
    }

    #[test]
    fn accepts_a_valid_draft_and_returns_the_download_row() {
        let service = service();
        let submitted = submit(&*service, &draft("brand-new"), &[]).expect("valid");
        assert!(submitted.download.title.starts_with("PostgreSQL"));
        assert!(!submitted.operation.as_str().is_empty());
    }

    #[test]
    fn accepts_an_empty_port_as_auto_allocate() {
        let service = service();
        let mut draft = draft("auto-port");
        draft.port_text = "   ".to_owned();
        assert!(submit(&*service, &draft, &[]).is_ok());
    }

    #[test]
    fn surfaces_a_service_level_rejection() {
        let service = service();
        let _id = service.seed_instance(
            InstanceSpec::new(
                dblocal_core::model::InstanceName::new("dup-name").expect("name"),
                EngineKey::Valkey,
                "8.1.0",
                Some(Port::new(6379).expect("port")),
                AuthMode::None,
            )
            .expect("spec"),
            dblocal_core::model::InstanceStatus::Stopped,
        );
        // A stale instance list omits the duplicate, so the pre-check passes and
        // `validate_spec` is the one that rejects it.
        let err = submit(&*service, &draft("dup-name"), &[]).expect_err("duplicate");
        assert!(matches!(err, SubmitError::Service(_)));
    }
}
