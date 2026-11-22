//! Engine adapter contract and stub implementations.
//!
//! Every engine implements [`EngineAdapter`]. The core and the service talk to
//! engines only through this trait, so adding a new engine never changes
//! orchestration code.

pub mod glibc;
pub mod mariadb;
mod mysql;
mod postgres;
pub mod sockdir;
pub mod uri;
mod valkey;

pub use mariadb::{
    MariaDbAdapter, log_error_path as mariadb_log_path, prepare_run as prepare_mariadb_run,
    sync_port as sync_mariadb_port,
};
pub use mysql::{
    Handshake as MysqlHandshake, MysqlAdapter, log_path as mysql_log_path, my_cnf_path,
    parse_handshake_payload as parse_mysql_handshake_payload, pidfile_path as mysql_pidfile_path,
    prepare_run as prepare_mysql_run, read_handshake as read_mysql_handshake,
    socket_path as mysql_socket_path, socket_path_from_my_cnf as mysql_socket_path_from_my_cnf,
    socket_path_limit as mysql_socket_path_limit, sync_port as sync_mysql_port,
    validate_socket_path_len as validate_mysql_socket_path_len,
};
pub use postgres::{
    PostgresAdapter, PostmasterIdentity, SocketDir as PostgresSocketDir,
    choose_socket_dir as choose_postgres_socket_dir,
    configure_after_init as configure_postgres_after_init, parse_postmaster_pid,
    prepare_run as prepare_postgres_run, read_postmaster_pid, sync_port as sync_postgres_port,
};
pub use valkey::{
    ConfigParams, ServerIdentity, ValkeyAdapter, parse_pidfile, prepare_run as prepare_valkey_run,
    read_pidfile, sync_port as sync_valkey_port,
};

/// The MySQL adapter's start preparation, as an associated-function style entry
/// used by the start dispatch (`instance/start.rs::prepare`).
pub fn mysql_prepare_run(ctx: &RunContext) -> Result<()> {
    MysqlAdapter.prepare_run(ctx)
}

use std::path::PathBuf;
use std::process::Command;

use crate::error::Result;
use crate::model::{AuthMode, EngineKey, Port, StartFailure};
use crate::secret::SecretString;
use crate::service::UriFormat;

/// Environment in which `initdb` (or its equivalent) runs.
///
/// The password is passed separately from a boolean so adapters can bootstrap
/// real credentials: PostgreSQL `initdb --pwfile`, Valkey `requirepass` and the
/// MySQL/MariaDB root password all need it. It is a [`SecretString`], so it can
/// never reach argv, logs or `Debug` output by accident.
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
    /// Authentication mode chosen at create time.
    pub auth: AuthMode,
    /// The generated superuser password.
    ///
    /// `Some` exactly when [`InitContext::auth`] is [`AuthMode::Password`],
    /// `None` exactly when it is [`AuthMode::None`]. Adapters must not write it
    /// to disk in cleartext or into the config file when the mode is `None`.
    pub password: Option<SecretString>,
}

impl InitContext {
    /// Whether a password must be configured.
    pub const fn with_password(&self) -> bool {
        matches!(self.auth, AuthMode::Password)
    }
}

/// Runtime environment passed to start, stop and health-check operations.
///
/// Authenticated engines need credentials here too: a clean `SHUTDOWN`,
/// an authenticated health probe and every [`AdminOps`] call use the same
/// value. Adapters must never put the password on a command line; use engine
/// environment variables (`PGPASSWORD`, `MYSQL_PWD`) or a stdin pipe.
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
    /// Authentication mode of this instance.
    pub auth: AuthMode,
    /// Credential for authenticated operations.
    ///
    /// `Some` exactly when [`RunContext::auth`] is [`AuthMode::Password`].
    /// [`AdminOps`] reads the credential from here; no method takes one.
    pub credential: Option<Credential>,
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

/// Administrative operations supported by an engine in Phase 2 (PRD `F-DBU`).
///
/// Every method authenticates with [`RunContext::credential`]: the trait never
/// takes a credential argument, so a caller cannot accidentally act as one user
/// while reporting another.
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
    ///
    /// Bootstraps the superuser with [`InitContext::auth`] and
    /// [`InitContext::password`].
    fn initialize(&self, ctx: &InitContext) -> Result<()>;

    /// Builds the detached command that starts the server.
    ///
    /// Errors are surfaced when the caller spawns or probes the process, not
    /// when building the command description. The returned command must never
    /// carry a password in argv; use [`RunContext::credential`] with an engine
    /// environment variable instead.
    fn spawn_command(&self, ctx: &RunContext) -> Command;

    /// Performs a protocol-level readiness probe.
    ///
    /// This is a real handshake, not a liveness check. An authentication
    /// challenge is proof the server is up, so Valkey `NOAUTH` and a
    /// PostgreSQL auth request must both answer `Health::Ready`.
    fn health_check(&self, ctx: &RunContext) -> Result<Health>;

    /// Requests a clean shutdown and blocks until the server is gone.
    ///
    /// The call is bounded by [`crate::consts::STOP_GRACE_SECS`]; the caller
    /// escalates to a forced kill after that, which PRD `F-INS-03` AC3 requires
    /// to be logged as a forced kill.
    fn shutdown(&self, ctx: &RunContext) -> Result<()>;

    /// Builds a connection URI or `.env` block, redacted in its `Display`
    /// form by the caller.
    fn connection_uri(
        &self,
        ctx: &RunContext,
        credential: &Credential,
        format: UriFormat,
    ) -> SecretString;

    /// Builds a connection URI or `.env` block for an instance with no password
    /// (auth mode [`AuthMode::None`], PRD `F-CRD-03`).
    ///
    /// There is no credential to pass, so the default renders through
    /// [`uri::render_no_password`]: the userinfo carries only the username and
    /// no password key appears in the `.env` block. The service only calls this
    /// when the instance's auth mode is `None`; a password instance with a
    /// missing credential stays a typed error.
    fn connection_uri_no_password(&self, ctx: &RunContext, format: UriFormat) -> SecretString {
        uri::render_no_password(self.key(), ctx.port.get(), &ctx.superuser, format)
    }

    /// Classifies raw start output so the UI can offer a fix (PRD `F-LOG-02`).
    ///
    /// `output` is the combined stdout/stderr captured during a failed start.
    /// Implementations match on engine-specific error text; the default answers
    /// [`StartFailure::Unknown`] so a stub and a not-yet-taught engine both
    /// degrade to the generic "see the log" message.
    fn diagnose_start_failure(&self, output: &str, exit_code: Option<i32>) -> StartFailure {
        let _ = (output, exit_code);
        StartFailure::Unknown
    }

    /// Optional Phase 2 admin operations.
    fn admin_ops(&self) -> Option<&dyn AdminOps> {
        None
    }

    /// Recreates engine-owned runtime state before every start.
    ///
    /// The start path calls this on every start so volatile directories that a
    /// previous run may have lost (tempfiles root, fallback unix socket dir) are
    /// rebuilt. Returns the engine error when the state cannot be rebuilt.
    fn prepare_run(&self, _ctx: &RunContext) -> Result<()> {
        Ok(())
    }
}

/// Returns the adapter for an engine.
pub fn adapter_for(engine: EngineKey) -> Box<dyn EngineAdapter> {
    match engine {
        EngineKey::Postgresql => Box::new(postgres::PostgresAdapter),
        EngineKey::Valkey => Box::new(valkey::ValkeyAdapter),
        EngineKey::Mariadb => Box::new(mariadb::MariaDbAdapter),
        EngineKey::Mysql => Box::new(mysql::MysqlAdapter),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AuthMode;
    use std::path::Path;

    fn ctx() -> (InitContext, RunContext) {
        let port = Port::new(5432).expect("valid");
        let init = InitContext {
            instance_name: "app".into(),
            runtime_dir: PathBuf::from("/tmp/rt"),
            data_dir: PathBuf::from("/tmp/data"),
            port,
            superuser: "postgres".into(),
            auth: AuthMode::Password,
            password: Some(SecretString::from("pw")),
        };
        let run = RunContext {
            instance_name: "app".into(),
            runtime_dir: PathBuf::from("/tmp/rt"),
            data_dir: PathBuf::from("/tmp/data"),
            logs_dir: PathBuf::from("/tmp/logs"),
            run_dir: PathBuf::from("/tmp/run"),
            port,
            superuser: "postgres".into(),
            auth: AuthMode::Password,
            credential: Some(Credential::new("postgres", SecretString::from("pw"))),
        };
        (init, run)
    }

    #[test]
    fn mysql_and_mariadb_are_real_adapters() {
        let (_init, run) = ctx();
        let mysql = adapter_for(EngineKey::Mysql);
        assert_eq!(mysql.key(), EngineKey::Mysql);
        assert_eq!(mysql.canonical_port(), 3306);
        assert!(mysql.admin_ops().is_none());
        assert_ne!(
            mysql.spawn_command(&run).get_program(),
            std::ffi::OsStr::new("dblocal-stub")
        );

        let mariadb = adapter_for(EngineKey::Mariadb);
        assert_eq!(mariadb.key(), EngineKey::Mariadb);
        assert_eq!(mariadb.canonical_port(), 3307);
        assert!(mariadb.admin_ops().is_none());
        assert_ne!(
            mariadb.spawn_command(&run).get_program(),
            std::ffi::OsStr::new("dblocal-stub")
        );
    }

    #[test]
    fn postgres_valkey_mysql_are_real_adapters() {
        let pg = adapter_for(EngineKey::Postgresql);
        assert_eq!(pg.key(), EngineKey::Postgresql);
        assert!(pg.admin_ops().is_none());
        let vk = adapter_for(EngineKey::Valkey);
        assert_eq!(vk.key(), EngineKey::Valkey);
        assert!(vk.admin_ops().is_none());
        let my = adapter_for(EngineKey::Mysql);
        assert_eq!(my.key(), EngineKey::Mysql);
    }

    #[test]
    fn auth_is_consistent_in_contexts() {
        let password = SecretString::from("s3cret");
        let init = InitContext {
            instance_name: "app".into(),
            runtime_dir: PathBuf::from("/tmp/rt"),
            data_dir: PathBuf::from("/tmp/data"),
            port: Port::new(5432).expect("valid"),
            superuser: "postgres".into(),
            auth: AuthMode::Password,
            password: Some(password.clone()),
        };
        assert!(init.with_password());
        assert!(init.password.is_some());
        assert!(!format!("{init:?}").contains("s3cret"));

        let init_none = InitContext {
            auth: AuthMode::None,
            password: None,
            ..init.clone()
        };
        assert!(!init_none.with_password());
        assert!(init_none.password.is_none());

        let run = RunContext {
            instance_name: "app".into(),
            runtime_dir: PathBuf::from("/tmp/rt"),
            data_dir: PathBuf::from("/tmp/data"),
            logs_dir: PathBuf::from("/tmp/logs"),
            run_dir: PathBuf::from("/tmp/run"),
            port: Port::new(5432).expect("valid"),
            superuser: "postgres".into(),
            auth: AuthMode::Password,
            credential: Some(Credential::new("postgres", password)),
        };
        assert_eq!(run.auth, AuthMode::Password);
        assert!(run.credential.is_some());
        assert!(!format!("{run:?}").contains("s3cret"));
        assert!(!format!("{:?}", run.credential).contains("s3cret"));

        let run_none = RunContext {
            auth: AuthMode::None,
            credential: None,
            ..run
        };
        assert!(run_none.credential.is_none());
    }

    #[test]
    fn stub_uri_is_redacted() {
        let adapter = adapter_for(EngineKey::Mysql);
        let (_, run) = ctx();
        let cred = Credential::new("root", SecretString::from("top-secret"));
        let uri = adapter.connection_uri(&run, &cred, UriFormat::Uri);
        assert!(!format!("{uri}").contains("top-secret"));
        assert!(!format!("{uri:?}").contains("top-secret"));
        assert!(uri.expose().starts_with("mysql://"));
        // Sanity: the type only, never logged.
        let _ = Path::new(uri.expose());
    }

    #[test]
    fn real_adapters_render_uris_and_dotenv() {
        let (_, run) = ctx();
        let cred = Credential::new("postgres", SecretString::from("s3cr:et"));
        let pg = adapter_for(EngineKey::Postgresql);
        let uri = pg.connection_uri(&run, &cred, UriFormat::Uri);
        assert!(uri.expose().starts_with("postgresql://postgres:"));
        let env = pg.connection_uri(&run, &cred, UriFormat::DotEnv);
        assert!(env.expose().contains("DATABASE_URL="));
        assert!(env.expose().contains("DB_USERNAME=postgres"));

        let run_vk = RunContext {
            port: Port::new(6379).expect("valid"),
            ..run
        };
        let vk = adapter_for(EngineKey::Valkey);
        let cred_vk = Credential::new("default", SecretString::from("pw"));
        let uri = vk.connection_uri(&run_vk, &cred_vk, UriFormat::Uri);
        assert!(uri.expose().starts_with("redis://default:"));
        let env = vk.connection_uri(&run_vk, &cred_vk, UriFormat::DotEnv);
        assert!(env.expose().contains("REDIS_URL="));
    }
}
