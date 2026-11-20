//! Read models the GUI and CLI render directly.

use crate::model::{
    AuthMode, CatalogStatus, CredentialBackend, EngineKey, Instance, InstanceId, InstanceStatus,
    Port, StartFailure,
};

/// Everything a UI needs to explain a failed start (PRD `F-INS-03` AC4, `F-LOG-02`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureInfo {
    /// Stable `DBL-NNNN` code of the error that caused the failure.
    pub code: String,
    /// Friendly, non-secret message.
    pub message: String,
    /// Engine-specific diagnosis so the UI can offer a concrete fix.
    pub diagnosis: StartFailure,
    /// Last 20 log lines, rendered inline without asking the user to open logs.
    pub log_tail: Vec<String>,
}

/// What the GUI and CLI need to render one row in the instance list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceView {
    /// Stable identifier.
    pub id: InstanceId,
    /// User-visible name.
    pub name: String,
    /// Engine.
    pub engine: EngineKey,
    /// Engine version tag.
    pub version: String,
    /// Bound port.
    pub port: Port,
    /// Lifecycle status.
    pub status: InstanceStatus,
    /// Authentication mode, shown as the "No auth" badge when [`AuthMode::None`].
    pub auth: AuthMode,
    /// Whether the instance starts with the app.
    pub autostart: bool,
    /// Where credentials live, surfaced for the fallback-file note (F-CRD-01 AC2).
    pub credential_backend: Option<CredentialBackend>,
    /// Details of the most recent failure, present while `status` is `failed`.
    pub failure: Option<FailureInfo>,
}

impl InstanceView {
    /// Builds a view from a domain instance, without failure or credential info.
    pub fn from_instance(instance: &Instance) -> Self {
        Self {
            id: instance.id().clone(),
            name: instance.name().to_string(),
            engine: instance.engine(),
            version: instance.version().to_owned(),
            port: instance.port(),
            status: instance.status(),
            auth: instance.auth(),
            autostart: instance.autostart(),
            credential_backend: None,
            failure: None,
        }
    }

    /// Returns a copy with a credential backend recorded.
    pub fn with_credential_backend(mut self, backend: CredentialBackend) -> Self {
        self.credential_backend = Some(backend);
        self
    }

    /// Returns a copy carrying failure details.
    pub fn with_failure(mut self, failure: FailureInfo) -> Self {
        self.failure = Some(failure);
        self
    }
}

/// One engine in the platform-filtered catalog read model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineEntry {
    /// Engine.
    pub engine: EngineKey,
    /// Whether at least one version can be installed on this platform.
    pub available: bool,
    /// Why the engine is unavailable, shown in the disabled state (F-CAT-02 AC2).
    pub unavailable_reason: Option<String>,
    /// Versions that run on this platform, newest first.
    pub versions: Vec<VersionEntry>,
}

/// One version of one engine, as the picker renders it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionEntry {
    /// Version tag, for example `18.7.0`.
    pub version: String,
    /// Whether upstream no longer supports this version.
    pub eol: bool,
    /// Whether a runtime for this version is already installed locally.
    pub installed: bool,
    /// Download size in bytes, so the UI can show "260 MB" before a click.
    pub size_bytes: u64,
}

/// Platform-filtered catalog read model (PRD `F-CAT-02` AC1–AC3).
///
/// The service does the platform filtering once, so the GUI and the CLI never
/// re-implement it and never show an installable version that cannot run here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogView {
    /// Freshness of the snapshot in use.
    pub status: CatalogStatus,
    /// Banner to show, for example "Katalog mungkin tidak terbaru".
    pub banner: Option<String>,
    /// Engines in display order, including unavailable ones.
    pub engines: Vec<EngineEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AuthMode, EngineKey, InstanceName, InstanceSpec};
    use std::time::SystemTime;

    fn spec() -> InstanceSpec {
        InstanceSpec::new(
            InstanceName::new("app-dev").expect("valid"),
            EngineKey::Postgresql,
            "18.7",
            Some(Port::new(5432).expect("valid")),
            AuthMode::Password,
        )
        .expect("valid spec")
    }

    #[test]
    fn instance_view_exposes_auth_and_badges() {
        let instance = Instance::new(
            &spec(),
            InstanceId::new(),
            InstanceStatus::Stopped,
            SystemTime::UNIX_EPOCH,
        );
        let view = InstanceView::from_instance(&instance)
            .with_credential_backend(CredentialBackend::File)
            .with_failure(FailureInfo {
                code: "DBL-4001".to_owned(),
                message: "engine failed".to_owned(),
                diagnosis: StartFailure::PortInUse,
                log_tail: vec!["line 1".into(), "line 2".into()],
            });
        assert_eq!(view.auth, AuthMode::Password);
        assert!(!view.autostart);
        assert_eq!(view.credential_backend, Some(CredentialBackend::File));
        assert_eq!(
            view.failure.as_ref().expect("failure").diagnosis,
            StartFailure::PortInUse
        );
        assert_eq!(view.failure.as_ref().expect("failure").log_tail.len(), 2);
    }

    #[test]
    fn catalog_view_carries_availability() {
        let view = CatalogView {
            status: CatalogStatus::Stale,
            banner: Some("stale".to_owned()),
            engines: vec![EngineEntry {
                engine: EngineKey::Valkey,
                available: false,
                unavailable_reason: Some("not on windows".to_owned()),
                versions: vec![VersionEntry {
                    version: "8.0.0".to_owned(),
                    eol: false,
                    installed: false,
                    size_bytes: 12,
                }],
            }],
        };
        assert_eq!(view.status, CatalogStatus::Stale);
        assert_eq!(view.engines[0].versions[0].size_bytes, 12);
        assert!(!view.engines[0].available);
    }
}
