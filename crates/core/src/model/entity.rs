//! Instance, credential, port and archive value types.

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::SystemTime;

use crate::DblError;
use crate::model::{AuthMode, EngineKey, InstanceId, InstanceName, InstanceStatus, Port};

/// Everything needed to create one instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceSpec {
    name: InstanceName,
    engine: EngineKey,
    version: String,
    port: Option<Port>,
    auth: AuthMode,
    autostart: bool,
}

impl InstanceSpec {
    /// Builds a spec, validating the version tag at the boundary.
    ///
    /// `port` is `None` when the caller wants the service to allocate one, as
    /// `dblocal create` does without `--port` (PRD `F-PRT-01`).
    pub fn new(
        name: InstanceName,
        engine: EngineKey,
        version: impl Into<String>,
        port: Option<Port>,
        auth: AuthMode,
    ) -> crate::Result<Self> {
        let version = version.into();
        super::validate_version_tag("version", &version)?;
        Ok(Self {
            name,
            engine,
            version,
            port,
            auth,
            autostart: false,
        })
    }

    /// Instance name.
    pub const fn name(&self) -> &InstanceName {
        &self.name
    }
    /// Engine the instance runs.
    pub const fn engine(&self) -> EngineKey {
        self.engine
    }
    /// Engine version tag, for example `18.7`.
    pub fn version(&self) -> &str {
        &self.version
    }
    /// Port to bind, or `None` to let the service allocate one.
    pub const fn port(&self) -> Option<Port> {
        self.port
    }
    /// Authentication mode chosen at create time.
    pub const fn auth(&self) -> AuthMode {
        self.auth
    }
    /// Whether the instance starts with the app.
    pub const fn autostart(&self) -> bool {
        self.autostart
    }

    /// Returns a copy with a different port, or `None` to re-allocate.
    pub fn with_port(mut self, port: Option<Port>) -> Self {
        self.port = port;
        self
    }

    /// Returns a copy with autostart toggled.
    pub fn with_autostart(mut self, autostart: bool) -> Self {
        self.autostart = autostart;
        self
    }
}

/// A created instance, as persisted in `state.db`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instance {
    id: InstanceId,
    name: InstanceName,
    engine: EngineKey,
    version: String,
    port: Port,
    status: InstanceStatus,
    auth: AuthMode,
    superuser: String,
    pid: Option<u32>,
    process_started_at: Option<SystemTime>,
    process_exe: Option<PathBuf>,
    autostart: bool,
    created_at: SystemTime,
}

impl Instance {
    /// Builds an instance, normally in `Draft` or `Provisioning`.
    pub fn new(
        spec: &InstanceSpec,
        id: InstanceId,
        status: InstanceStatus,
        created_at: SystemTime,
    ) -> Self {
        Self {
            id,
            name: spec.name.clone(),
            engine: spec.engine,
            version: spec.version.clone(),
            // A persisted instance always has a port. The real service resolves
            // an automatic allocation before it gets here; canonical ports keep
            // the no-allocation path total and deterministic.
            port: spec
                .port
                .unwrap_or_else(|| Port::assume_valid(spec.engine.canonical_port())),
            status,
            auth: spec.auth,
            superuser: spec.engine.default_superuser().to_owned(),
            pid: None,
            process_started_at: None,
            process_exe: None,
            autostart: spec.autostart,
            created_at,
        }
    }

    /// Stable identifier.
    pub const fn id(&self) -> &InstanceId {
        &self.id
    }
    /// Unique name, also the data directory name.
    pub const fn name(&self) -> &InstanceName {
        &self.name
    }
    /// Engine the instance runs.
    pub const fn engine(&self) -> EngineKey {
        self.engine
    }
    /// Engine version tag.
    pub fn version(&self) -> &str {
        &self.version
    }
    /// Port the engine binds.
    pub const fn port(&self) -> Port {
        self.port
    }
    /// Current lifecycle status.
    pub const fn status(&self) -> InstanceStatus {
        self.status
    }
    /// Authentication mode.
    pub const fn auth(&self) -> AuthMode {
        self.auth
    }
    /// Superuser name, never its password.
    pub fn superuser(&self) -> &str {
        &self.superuser
    }
    /// PID of the engine process, if known.
    pub const fn pid(&self) -> Option<u32> {
        self.pid
    }
    /// Wall clock time the recorded process started, used to defeat PID reuse.
    pub const fn process_started_at(&self) -> Option<SystemTime> {
        self.process_started_at
    }
    /// Executable name of the recorded process, used to defeat PID reuse.
    pub fn process_exe(&self) -> Option<&std::path::Path> {
        self.process_exe.as_deref()
    }
    /// Whether the instance starts with the app.
    pub const fn autostart(&self) -> bool {
        self.autostart
    }
    /// Creation time.
    pub const fn created_at(&self) -> SystemTime {
        self.created_at
    }

    /// Returns a copy in a new status. The state machine is enforced by the
    /// caller through [`InstanceStatus::apply`].
    pub fn with_status(mut self, status: InstanceStatus) -> Self {
        self.status = status;
        self
    }

    /// Returns a copy with a new port, used by the "switch port" action.
    pub fn with_port(mut self, port: Port) -> Self {
        self.port = port;
        self
    }

    /// Returns a copy with autostart toggled.
    pub fn with_autostart(mut self, autostart: bool) -> Self {
        self.autostart = autostart;
        self
    }

    /// Records the spawned process identity.
    pub fn with_process(mut self, pid: u32, started_at: SystemTime, exe: PathBuf) -> Self {
        self.pid = Some(pid);
        self.process_started_at = Some(started_at);
        self.process_exe = Some(exe);
        self
    }

    /// Forgets the process identity, for example after reconciliation.
    pub fn without_process(mut self) -> Self {
        self.pid = None;
        self.process_started_at = None;
        self.process_exe = None;
        self
    }
}

/// A port reserved by one instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortAllocation {
    port: Port,
    instance_id: InstanceId,
    reserved_at: SystemTime,
}

impl PortAllocation {
    /// Creates an allocation record.
    pub const fn new(port: Port, instance_id: InstanceId, reserved_at: SystemTime) -> Self {
        Self {
            port,
            instance_id,
            reserved_at,
        }
    }
    /// Reserved port.
    pub const fn port(&self) -> Port {
        self.port
    }
    /// Instance holding the reservation.
    pub const fn instance_id(&self) -> &InstanceId {
        &self.instance_id
    }
    /// When the port was reserved.
    pub const fn reserved_at(&self) -> SystemTime {
        self.reserved_at
    }
}

/// Where a credential is actually stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CredentialBackend {
    /// OS keychain: Secret Service, Keychain, Credential Manager.
    Keychain,
    /// Fallback file with mode `0600` inside `secrets_dir`.
    File,
}

impl CredentialBackend {
    /// Stable identifier used in SQLite.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Keychain => "keychain",
            Self::File => "file",
        }
    }
}

impl FromStr for CredentialBackend {
    type Err = DblError;

    fn from_str(value: &str) -> Result<Self, DblError> {
        match value {
            "keychain" => Ok(Self::Keychain),
            "file" => Ok(Self::File),
            other => Err(DblError::InvalidInput {
                field: "credential_backend",
                reason: format!("unknown backend: {other}"),
            }),
        }
    }
}

/// A pointer to a secret. The secret itself lives in the keychain or a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialRef {
    instance_id: InstanceId,
    backend: CredentialBackend,
    key_ref: String,
    username: String,
}

impl CredentialRef {
    /// Creates a reference without touching the secret.
    pub fn new(
        instance_id: InstanceId,
        backend: CredentialBackend,
        key_ref: impl Into<String>,
        username: impl Into<String>,
    ) -> Self {
        Self {
            instance_id,
            backend,
            key_ref: key_ref.into(),
            username: username.into(),
        }
    }

    /// Keychain key, shaped `dblocal:<instance-id>`.
    pub fn keychain_key(&self, instance_id: &InstanceId) -> String {
        format!("dblocal:{instance_id}")
    }

    /// Instance the credential belongs to.
    pub const fn instance_id(&self) -> &InstanceId {
        &self.instance_id
    }
    /// Storage backend in use.
    pub const fn backend(&self) -> CredentialBackend {
        self.backend
    }
    /// Opaque key inside the backend.
    pub fn key_ref(&self) -> &str {
        &self.key_ref
    }
    /// Database user name, not a secret.
    pub fn username(&self) -> &str {
        &self.username
    }
}

/// What to do with the data directory when an instance is deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum DeleteMode {
    /// Delete the instance and its data directory.
    PurgeData,
    /// Move the data directory to `archive/` first.
    #[default]
    ArchiveData,
}

impl DeleteMode {
    /// Stable identifier used by the CLI.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PurgeData => "purge",
            Self::ArchiveData => "archive",
        }
    }
}

impl FromStr for DeleteMode {
    type Err = DblError;

    fn from_str(value: &str) -> Result<Self, DblError> {
        match value {
            "purge" => Ok(Self::PurgeData),
            "archive" => Ok(Self::ArchiveData),
            other => Err(DblError::InvalidInput {
                field: "delete_mode",
                reason: format!("unknown delete mode: {other}"),
            }),
        }
    }
}

/// A data directory that was archived instead of deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivedData {
    id: String,
    instance_name: InstanceName,
    engine: EngineKey,
    version: String,
    path: PathBuf,
    size_bytes: u64,
    archived_at: SystemTime,
}

impl ArchivedData {
    /// Creates an archive record.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        instance_name: InstanceName,
        engine: EngineKey,
        version: impl Into<String>,
        path: PathBuf,
        size_bytes: u64,
        archived_at: SystemTime,
    ) -> Self {
        Self {
            id: id.into(),
            instance_name,
            engine,
            version: version.into(),
            path,
            size_bytes,
            archived_at,
        }
    }

    /// Archive identifier.
    pub fn id(&self) -> &str {
        &self.id
    }
    /// Name of the instance the data came from.
    pub const fn instance_name(&self) -> &InstanceName {
        &self.instance_name
    }
    /// Engine the data belongs to.
    pub const fn engine(&self) -> EngineKey {
        self.engine
    }
    /// Engine version tag.
    pub fn version(&self) -> &str {
        &self.version
    }
    /// Archived directory.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
    /// Total size on disk.
    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }
    /// When the archive was created.
    pub const fn archived_at(&self) -> SystemTime {
        self.archived_at
    }
}

impl fmt::Display for DeleteMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> InstanceSpec {
        InstanceSpec::new(
            InstanceName::new("app-dev").expect("valid name"),
            EngineKey::Postgresql,
            "18.7",
            Some(Port::new(5432).expect("valid port")),
            AuthMode::Password,
        )
        .expect("valid spec")
    }

    #[test]
    fn spec_rejects_bad_version() {
        let err = InstanceSpec::new(
            InstanceName::new("app-dev").expect("valid name"),
            EngineKey::Valkey,
            "../etc",
            Some(Port::new(6379).expect("valid port")),
            AuthMode::None,
        );
        assert!(err.is_err());
    }

    #[test]
    fn builders_return_new_values() {
        let base = spec();
        let moved = base
            .clone()
            .with_port(Some(Port::new(5433).expect("valid port")));
        assert_eq!(base.port().map(Port::get), Some(5432));
        assert_eq!(moved.port().map(Port::get), Some(5433));
        assert!(!base.autostart());
        assert!(base.clone().with_autostart(true).autostart());
    }

    #[test]
    fn instance_records_process_identity() {
        let instance = Instance::new(
            &spec(),
            InstanceId::new(),
            InstanceStatus::Running,
            SystemTime::UNIX_EPOCH,
        )
        .with_process(
            4242,
            SystemTime::UNIX_EPOCH,
            PathBuf::from("/opt/pg/bin/postgres"),
        );
        assert_eq!(instance.pid(), Some(4242));
        assert_eq!(instance.superuser(), "postgres");
        assert!(instance.without_process().pid().is_none());
    }

    #[test]
    fn credential_ref_never_holds_the_secret() {
        let id = InstanceId::new();
        let ref_ = CredentialRef::new(id.clone(), CredentialBackend::Keychain, "key", "postgres");
        assert_eq!(ref_.keychain_key(&id), format!("dblocal:{id}"));
        assert_eq!(ref_.backend().as_str(), "keychain");
        assert_eq!(
            "file".parse::<CredentialBackend>().ok(),
            Some(CredentialBackend::File)
        );
    }

    #[test]
    fn archived_data_exposes_every_field() {
        let archived = ArchivedData::new(
            "arch-1",
            InstanceName::new("app-dev").expect("valid name"),
            EngineKey::Postgresql,
            "18.7",
            PathBuf::from("/tmp/archive/app-dev-20260101-1200"),
            2048,
            SystemTime::UNIX_EPOCH,
        );
        assert_eq!(archived.id(), "arch-1");
        assert_eq!(archived.instance_name().as_str(), "app-dev");
        assert_eq!(archived.engine(), EngineKey::Postgresql);
        assert_eq!(archived.version(), "18.7");
        assert_eq!(
            archived.path(),
            std::path::Path::new("/tmp/archive/app-dev-20260101-1200")
        );
        assert_eq!(archived.size_bytes(), 2048);
        assert_eq!(archived.archived_at(), SystemTime::UNIX_EPOCH);
    }

    #[test]
    fn spec_and_instance_accessors_are_consistent() {
        let spec = spec();
        assert_eq!(spec.name().as_str(), "app-dev");
        assert_eq!(spec.engine(), EngineKey::Postgresql);
        assert_eq!(spec.version(), "18.7");
        assert_eq!(spec.port().map(Port::get), Some(5432));
        assert_eq!(spec.auth(), AuthMode::Password);

        let id = InstanceId::new();
        let instance = Instance::new(
            &spec,
            id.clone(),
            InstanceStatus::Stopped,
            SystemTime::UNIX_EPOCH,
        );
        assert_eq!(instance.id(), &id);
        assert_eq!(instance.name().as_str(), "app-dev");
        assert_eq!(instance.engine(), EngineKey::Postgresql);
        assert_eq!(instance.version(), "18.7");
        assert_eq!(instance.port().get(), 5432);
        assert_eq!(instance.status(), InstanceStatus::Stopped);
        assert_eq!(instance.auth(), AuthMode::Password);
        assert_eq!(instance.created_at(), SystemTime::UNIX_EPOCH);
        assert!(instance.process_started_at().is_none());
        assert!(instance.process_exe().is_none());
    }

    #[test]
    fn instance_updates_return_new_values() {
        let base = Instance::new(
            &spec(),
            InstanceId::new(),
            InstanceStatus::Running,
            SystemTime::UNIX_EPOCH,
        );
        let stopped = base.clone().with_status(InstanceStatus::Stopped);
        assert_eq!(base.status(), InstanceStatus::Running);
        assert_eq!(stopped.status(), InstanceStatus::Stopped);

        let moved = base.clone().with_port(Port::new(5433).expect("valid port"));
        assert_eq!(moved.port().get(), 5433);
        assert!(base.clone().with_autostart(true).autostart());

        let recorded = base.clone().with_process(
            99,
            SystemTime::UNIX_EPOCH,
            PathBuf::from("/opt/pg/bin/postgres"),
        );
        assert_eq!(recorded.pid(), Some(99));
        assert_eq!(recorded.process_started_at(), Some(SystemTime::UNIX_EPOCH));
        assert_eq!(
            recorded.process_exe(),
            Some(std::path::Path::new("/opt/pg/bin/postgres"))
        );
    }

    #[test]
    fn credential_ref_backend_round_trips() {
        for backend in [CredentialBackend::Keychain, CredentialBackend::File] {
            assert_eq!(
                backend.as_str().parse::<CredentialBackend>().ok(),
                Some(backend)
            );
        }
        assert!("vault".parse::<CredentialBackend>().is_err());
        assert_eq!(DeleteMode::PurgeData.as_str(), "purge");
        assert_eq!(DeleteMode::ArchiveData.to_string(), "archive");
    }

    #[test]
    fn delete_mode_defaults_to_archive() {
        assert_eq!(DeleteMode::default(), DeleteMode::ArchiveData);
        assert_eq!(
            "purge".parse::<DeleteMode>().ok(),
            Some(DeleteMode::PurgeData)
        );
        assert!("nuke".parse::<DeleteMode>().is_err());
    }
}
