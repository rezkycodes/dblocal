//! Engine adapter contract and stub implementations.
//!
//! Every engine implements [`EngineAdapter`]. The core and the service talk to
//! engines only through this trait, so adding a new engine never changes
//! orchestration code.

mod mariadb;
mod mysql;
mod postgres;
mod valkey;

use std::path::PathBuf;
use std::process::Command;

use crate::error::{DblError, Result};
use crate::model::{EngineKey, Port};
use crate::secret::SecretString;

/// Environment in which `initdb` (or its equivalent) runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitContext {
    /// Instance name, for logs and identification.
    pub instance_name: String,
    /// Path to the unpacked engine runtime.
    pub runtime_dir: PathBuf,
    /// Directory the engine will own as its data directory.
    pub data_dir: PathBuf,
    /// Port the engine will listen on.
    pub port: Port,
    /// Superuser name to bootstrap.
    pub superuser: String,
    /// Whether a password must be configured.
    pub with_password: bool,
}

/// Runtime environment passed to start, stop and health-check operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunContext {
    /// Instance name.
    pub instance_name: String,
    /// Path to the unpacked engine runtime.
    pub runtime_dir: PathBuf,
    /// Engine data directory.
    pub data_dir: PathBuf,
    /// Directory for log files.
    pub logs_dir: PathBuf,
    /// Directory for pidfile and sockets.
    pub run_dir: PathBuf,
    /// Port the engine listens on.
    pub port: Port,
    /// Superuser name.
    pub superuser: String,
}

/// Result of a protocol-level health check.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Health {
    /// The engine is ready to accept connections.
    Ready,
    /// The engine answered but is not ready yet.
    NotReady(String),
}

/// Superuser credential, handed to the adapter to build a URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credential {
    /// Database user name.
    pub username: String,
    /// The secret password.
    pub password: SecretString,
}

impl Credential {
    /// Builds a credential pair.
    pub fn new(username: impl Into<String>, password: SecretString) -> Self {
        Self {
            username: username.into(),
            password,
        }
    }
}

/// Administrative operations supported by an engine in Phase 2.
pub trait AdminOps: Send + Sync {
    /// Lists databases visible to the superuser.
    fn list_databases(&self, ctx: &RunContext) -> Result<Vec<String>>;
    /// Creates a database.
    fn create_database(&self, ctx: &RunContext, name: &str) -> Result<()>;
    /// Drops a database.
    fn drop_database(&self, ctx: &RunContext, name: &str) -> Result<()>;
    /// Creates a user with a password.
    fn create_user(&self, ctx: &RunContext, name: &str, password: &SecretString) -> Result<()>;
    /// Resets a user's password.
    fn reset_password(&self, ctx: &RunContext, name: &str, password: &SecretString) -> Result<()>;
}

/// Everything the core needs to know about one database engine.
///
/// Implementations are stateless: they translate a context into a command or a
/// probe and return control immediately.
pub trait EngineAdapter: Send + Sync {
    /// Engine identifier.
    fn key(&self) -> EngineKey;

    /// Port DBLocal proposes when no instance of this engine exists yet.
    fn canonical_port(&self) -> u16 {
        self.key().canonical_port()
    }

    /// Prepares the data directory (`initdb`, config generation).
    fn initialize(&self, ctx: &InitContext) -> Result<()>;

    /// Builds the detached command that starts the server.
    ///
    /// Errors are surfaced when the caller spawns or probes the process, not
    /// when building the command description.
    fn spawn_command(&self, ctx: &RunContext) -> Command;

    /// Performs a protocol-level readiness probe.
    fn health_check(&self, ctx: &RunContext) -> Result<Health>;

    /// Requests a clean shutdown.
    fn shutdown(&self, ctx: &RunContext) -> Result<()>;

    /// Builds a connection URI, redacted in its `Display` form by the caller.
    fn connection_uri(&self, ctx: &RunContext, credential: &Credential) -> SecretString;

    /// Optional Phase 2 admin operations.
    fn admin_ops(&self) -> Option<&dyn AdminOps> {
        None
    }
}

/// Adapter that deliberately refuses every operation.
struct StubAdapter {
    engine: EngineKey,
}

impl StubAdapter {
    fn new(engine: EngineKey) -> Self {
        Self { engine }
    }

    fn not_implemented(&self) -> DblError {
        DblError::NotImplemented(format!("{} adapter", self.engine.display_name()))
    }
}

impl EngineAdapter for StubAdapter {
    fn key(&self) -> EngineKey {
        self.engine
    }

    fn initialize(&self, _ctx: &InitContext) -> Result<()> {
        Err(self.not_implemented())
    }

    fn spawn_command(&self, _ctx: &RunContext) -> Command {
        Command::new("dblocal-stub")
    }

    fn health_check(&self, _ctx: &RunContext) -> Result<Health> {
        Err(self.not_implemented())
    }

    fn shutdown(&self, _ctx: &RunContext) -> Result<()> {
        Err(self.not_implemented())
    }

    fn connection_uri(&self, _ctx: &RunContext, _credential: &Credential) -> SecretString {
        // A URI must embed the password, so it is always a secret.
        SecretString::new(format!("{}://", self.engine.as_str()))
    }
}

/// Returns the stub adapter for an engine.
///
/// Wave 1 workers replace this with real per-engine implementations.
pub fn adapter_for(engine: EngineKey) -> Box<dyn EngineAdapter> {
    Box::new(StubAdapter::new(engine))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn ctx() -> (InitContext, RunContext) {
        let port = Port::new(5432).expect("valid");
        let init = InitContext {
            instance_name: "app".into(),
            runtime_dir: PathBuf::from("/tmp/rt"),
            data_dir: PathBuf::from("/tmp/data"),
            port,
            superuser: "postgres".into(),
            with_password: true,
        };
        let run = RunContext {
            instance_name: "app".into(),
            runtime_dir: PathBuf::from("/tmp/rt"),
            data_dir: PathBuf::from("/tmp/data"),
            logs_dir: PathBuf::from("/tmp/logs"),
            run_dir: PathBuf::from("/tmp/run"),
            port,
            superuser: "postgres".into(),
        };
        (init, run)
    }

    #[test]
    fn stubs_answer_not_implemented() {
        for engine in EngineKey::ALL {
            let adapter = adapter_for(engine);
            let (init, run) = ctx();
            assert_eq!(adapter.key(), engine);
            assert_eq!(adapter.canonical_port(), engine.canonical_port());
            assert!(matches!(
                adapter.initialize(&init),
                Err(DblError::NotImplemented(_))
            ));
            // `spawn_command` is infallible by contract; stubs surface failure
            // through the fallible probes below.
            assert_eq!(
                adapter.spawn_command(&run).get_program(),
                std::ffi::OsStr::new("dblocal-stub")
            );
            assert!(matches!(
                adapter.health_check(&run),
                Err(DblError::NotImplemented(_))
            ));
            assert!(matches!(
                adapter.shutdown(&run),
                Err(DblError::NotImplemented(_))
            ));
            assert!(adapter.admin_ops().is_none());
        }
    }

    #[test]
    fn stub_uri_is_redacted() {
        let adapter = adapter_for(EngineKey::Valkey);
        let (_, run) = ctx();
        let cred = Credential::new("default", SecretString::from("top-secret"));
        let uri = adapter.connection_uri(&run, &cred);
        assert!(!format!("{uri}").contains("top-secret"));
        assert!(!format!("{uri:?}").contains("top-secret"));
        assert!(uri.expose().starts_with("valkey://"));
        // Sanity: the type only, never logged.
        let _ = Path::new(uri.expose());
    }
}
