//! PostgreSQL admin execution.
//!
//! All connections go to loopback only. Passwords and SQL text never appear in
//! logs, errors or `Debug` output: errors are [`AdminError`] values, which carry
//! no engine text, and the only place a password is formatted is the statement
//! this module immediately executes.
//!
//! Each public method opens its own short-lived connection, so the struct holds
//! no live socket and stays `Send + Sync`. Admin operations are rare, so the
//! connection cost is irrelevant next to the guarantee that no stale session is
//! left behind.

use std::future::Future;
use std::time::Duration;

use tokio_postgres::Client;

use crate::consts;
use crate::secret::SecretString;
use crate::secrets::generate_password;

use super::error::{AdminError, AdminResult, SqlStateCode, TargetProblem};
use super::reset::reset_with;
use super::sql::{
    self, Dialect, Identifier, PrivilegeLevel, alter_role_password_postgres,
    create_database_postgres, create_role_postgres, database_connection_count_postgres,
    database_exists_postgres, drop_database_postgres, drop_user_postgres, grant_privileges,
    list_databases_postgres, list_users_postgres, revoke_privileges, role_exists_postgres,
};

/// Connection target for admin operations.
///
/// Immutable: [`with_timeouts`](Self::with_timeouts) returns a new value.
#[derive(Clone)]
pub struct AdminTarget {
    host: String,
    port: u16,
    superuser: String,
    password: SecretString,
    connect_timeout: Duration,
    statement_timeout: Duration,
}

impl AdminTarget {
    /// Default connect timeout.
    pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
    /// Default statement timeout.
    pub const DEFAULT_STATEMENT_TIMEOUT: Duration = Duration::from_secs(30);

    /// Creates a validated admin target.
    ///
    /// `host` must be a loopback address or the name `localhost`; anything else
    /// is [`AdminError::NonLoopbackHost`].
    pub fn new(
        host: impl Into<String>,
        port: u16,
        superuser: impl Into<String>,
        password: SecretString,
    ) -> AdminResult<Self> {
        let host = host.into();
        validate_loopback_host(&host)?;
        if port == 0 {
            return Err(AdminError::InvalidTarget(TargetProblem::ZeroPort));
        }
        let superuser = superuser.into();
        if superuser.is_empty() {
            return Err(AdminError::InvalidTarget(TargetProblem::EmptySuperuser));
        }
        Ok(Self {
            host,
            port,
            superuser,
            password,
            connect_timeout: Self::DEFAULT_CONNECT_TIMEOUT,
            statement_timeout: Self::DEFAULT_STATEMENT_TIMEOUT,
        })
    }

    /// Hostname/IP (loopback only).
    pub fn host(&self) -> &str {
        &self.host
    }

    /// TCP port.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Superuser name.
    pub fn superuser(&self) -> &str {
        &self.superuser
    }

    /// Connect timeout.
    pub fn connect_timeout(&self) -> Duration {
        self.connect_timeout
    }

    /// Statement timeout.
    pub fn statement_timeout(&self) -> Duration {
        self.statement_timeout
    }

    /// Returns a copy with custom timeouts.
    pub fn with_timeouts(mut self, connect_timeout: Duration, statement_timeout: Duration) -> Self {
        self.connect_timeout = connect_timeout;
        self.statement_timeout = statement_timeout;
        self
    }

    /// Returns a copy that authenticates with `password` (used to prove a
    /// freshly applied password actually works).
    fn with_password(&self, password: SecretString) -> Self {
        let mut next = self.clone();
        next.password = password;
        next
    }

    /// The host to dial: `localhost` is pinned to `127.0.0.1` so no resolver can
    /// redirect the connection away from loopback.
    fn dial_host(&self) -> &str {
        if self.host.eq_ignore_ascii_case("localhost") {
            "127.0.0.1"
        } else {
            &self.host
        }
    }

    fn config(&self, dbname: &str) -> tokio_postgres::Config {
        let mut cfg = tokio_postgres::Config::new();
        cfg.host(self.dial_host())
            .port(self.port)
            .dbname(dbname)
            .user(&self.superuser)
            .password(self.password.expose().as_bytes())
            .connect_timeout(self.connect_timeout)
            .application_name("dblocal-admin");
        let suppress = std::env::var_os("DBLOCAL_ADMIN_SUPPRESS_LOG_OPTIONS").as_deref()
            != Some(std::ffi::OsStr::new("0"));
        let log_opts = if suppress {
            " -c log_min_error_statement=panic -c log_statement=none \
             -c log_min_duration_statement=-1"
        } else {
            ""
        };
        cfg.options(format!(
            "-c statement_timeout={} -c lock_timeout={}{log_opts}",
            self.statement_timeout.as_millis(),
            self.statement_timeout.as_millis()
        ));
        cfg
    }
}

impl std::fmt::Debug for AdminTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminTarget")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("superuser", &self.superuser)
            .field("password", &"[REDACTED]")
            .field("connect_timeout", &self.connect_timeout)
            .field("statement_timeout", &self.statement_timeout)
            .finish()
    }
}

fn validate_loopback_host(host: &str) -> AdminResult<()> {
    if host.is_empty() {
        return Err(AdminError::InvalidTarget(TargetProblem::EmptyHost));
    }
    if host.eq_ignore_ascii_case("localhost") {
        return Ok(());
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(ip) if ip.is_loopback() => Ok(()),
        _ => Err(AdminError::NonLoopbackHost),
    }
}

/// Database information returned by [`PgAdmin::list_databases`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseInfo {
    name: String,
    owner: String,
    size_bytes: i64,
}

impl DatabaseInfo {
    /// Creates a database info record.
    pub fn new(name: impl Into<String>, owner: impl Into<String>, size_bytes: i64) -> Self {
        Self {
            name: name.into(),
            owner: owner.into(),
            size_bytes,
        }
    }

    /// Database name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Owner role name.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Database size in bytes.
    pub fn size_bytes(&self) -> i64 {
        self.size_bytes
    }
}

/// Database user information returned by [`PgAdmin::list_users`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserInfo {
    name: String,
    can_login: bool,
    is_superuser: bool,
}

impl UserInfo {
    /// Creates a user info record.
    pub fn new(name: impl Into<String>, can_login: bool, is_superuser: bool) -> Self {
        Self {
            name: name.into(),
            can_login,
            is_superuser,
        }
    }

    /// Role name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether the role can log in.
    pub fn can_login(&self) -> bool {
        self.can_login
    }

    /// Whether the role is a superuser.
    pub fn is_superuser(&self) -> bool {
        self.is_superuser
    }
}

pub use super::reset::CredentialSink;

/// Computes a PostgreSQL SCRAM-SHA-256 verifier for `password`.
///
/// The cleartext is never sent on the wire or embedded in a logged statement:
/// PostgreSQL logs failing statements by default (`log_min_error_statement =
/// error`), and that log is what `dblocal logs` / the Logs tab show.
fn scram_credential(password: &SecretString) -> String {
    postgres_protocol::password::scram_sha_256(password.expose().as_bytes())
}

/// PostgreSQL admin client over one [`AdminTarget`].
#[derive(Debug, Clone)]
pub struct PgAdmin {
    target: AdminTarget,
}

impl PgAdmin {
    /// Builds an admin client over a validated target.
    pub fn new(target: AdminTarget) -> Self {
        Self { target }
    }

    /// The target this client uses.
    pub fn target(&self) -> &AdminTarget {
        &self.target
    }

    /// Lists non-template, connectable databases with owner and size.
    pub fn list_databases(&self) -> AdminResult<Vec<DatabaseInfo>> {
        self.with_client(
            &self.target,
            consts::DEFAULT_DATABASE,
            |client, _| async move {
                let stmt = list_databases_postgres();
                let rows = client
                    .query(stmt.sql(), &[])
                    .await
                    .map_err(|err| map_pg_error(&err))?;
                let mut out = Vec::with_capacity(rows.len());
                for row in rows {
                    let name: String = row.try_get(0).map_err(|err| map_pg_error(&err))?;
                    let owner: String = row.try_get(1).map_err(|err| map_pg_error(&err))?;
                    let size: i64 = row.try_get(2).map_err(|err| map_pg_error(&err))?;
                    out.push(DatabaseInfo::new(name, owner, size));
                }
                Ok(out)
            },
        )
    }

    /// Lists login-capable roles.
    pub fn list_users(&self) -> AdminResult<Vec<UserInfo>> {
        self.with_client(
            &self.target,
            consts::DEFAULT_DATABASE,
            |client, _| async move {
                let stmt = list_users_postgres();
                let rows = client
                    .query(stmt.sql(), &[])
                    .await
                    .map_err(|err| map_pg_error(&err))?;
                let mut out = Vec::with_capacity(rows.len());
                for row in rows {
                    let name: String = row.try_get(0).map_err(|err| map_pg_error(&err))?;
                    let can_login: bool = row.try_get(1).map_err(|err| map_pg_error(&err))?;
                    let is_superuser: bool = row.try_get(2).map_err(|err| map_pg_error(&err))?;
                    out.push(UserInfo::new(name, can_login, is_superuser));
                }
                Ok(out)
            },
        )
    }

    /// Creates a UTF-8 database with `template0` so the encoding is honoured.
    pub fn create_database(&self, db: &Identifier) -> AdminResult<()> {
        let stmt = create_database_postgres(db);
        self.execute(&self.target, consts::DEFAULT_DATABASE, stmt.sql())
    }

    /// Drops a database.
    ///
    /// Without `force`, a live connection is [`AdminError::DatabaseInUse`] and
    /// nothing is dropped. With `force`, PostgreSQL's `WITH (FORCE)` terminates
    /// the sessions; the switch is explicit so it is never the default.
    pub fn drop_database(&self, db: &Identifier, force: bool) -> AdminResult<()> {
        let statement = drop_database_postgres(db, force);
        if !force {
            self.ensure_no_connections(db)?;
        }
        self.execute(&self.target, consts::DEFAULT_DATABASE, statement.sql())
    }

    /// Counts live connections to `db`, refusing to drop when any exists.
    fn ensure_no_connections(&self, db: &Identifier) -> AdminResult<()> {
        let stmt = database_connection_count_postgres(db);
        let db_name = db.as_str().to_owned();
        let count = self.with_client(
            &self.target,
            consts::DEFAULT_DATABASE,
            move |client, _| async move {
                let row = client
                    .query_one(stmt.sql(), &[&db_name.as_str()])
                    .await
                    .map_err(|err| map_pg_error(&err))?;
                let count: i64 = row.try_get(0).map_err(|err| map_pg_error(&err))?;
                Ok(count)
            },
        )?;
        if count > 0 {
            return Err(AdminError::DatabaseInUse);
        }
        Ok(())
    }

    /// Creates a login role with a freshly generated password.
    ///
    /// Returns the password exactly once, as a [`SecretString`]. It is never
    /// logged and never stored here; the caller hands it to a `CredentialSink`.
    pub fn create_user(&self, user: &Identifier) -> AdminResult<SecretString> {
        let password = generate_password().map_err(|_| AdminError::PasswordGenerate)?;
        self.create_user_with_password(user, &password)?;
        Ok(password)
    }

    /// Creates a login role with a caller-supplied password.
    ///
    /// `#[doc(hidden)]`: the production API generates its own password; tests use
    /// this seam so they can look for the exact cleartext in the engine log.
    /// The cleartext is still never sent on the wire — only its SCRAM verifier
    /// is.
    #[doc(hidden)]
    pub fn create_user_with_password(
        &self,
        user: &Identifier,
        password: &SecretString,
    ) -> AdminResult<()> {
        let credential = scram_credential(password);
        let stmt = create_role_postgres(user, &credential);
        self.execute(&self.target, consts::DEFAULT_DATABASE, stmt.sql())
    }

    /// Alters a role password with a caller-supplied password.
    ///
    /// `#[doc(hidden)]`: test seam for F1 (failing ALTER path). Production code
    /// uses `reset_superuser_password`.
    #[doc(hidden)]
    pub fn alter_user_password(
        &self,
        user: &Identifier,
        password: &SecretString,
    ) -> AdminResult<()> {
        let credential = scram_credential(password);
        let stmt = alter_role_password_postgres(user, &credential);
        self.execute_mapped(
            &self.target,
            consts::DEFAULT_DATABASE,
            stmt.sql(),
            map_role_error,
        )
    }

    /// Drops a role.
    pub fn drop_user(&self, user: &Identifier) -> AdminResult<()> {
        let stmt = drop_user_postgres(user);
        self.execute_mapped(
            &self.target,
            consts::DEFAULT_DATABASE,
            stmt.sql(),
            map_role_error,
        )
    }

    /// Grants `level` to `user` on `db`, running the schema-level statements
    /// against `db` itself.
    pub fn grant(
        &self,
        user: &Identifier,
        db: &Identifier,
        level: PrivilegeLevel,
    ) -> AdminResult<()> {
        let statements = grant_privileges(Dialect::Postgres, user, db, level);
        self.execute_batch_mapped(&self.target, db.as_str(), &statements, map_role_error)
    }

    /// Revokes every managed privilege from `user` on `db`.
    pub fn revoke(&self, user: &Identifier, db: &Identifier) -> AdminResult<()> {
        let statements = revoke_privileges(Dialect::Postgres, user, db);
        self.execute_batch_mapped(&self.target, db.as_str(), &statements, map_role_error)
    }

    /// Whether `user` exists.
    pub fn user_exists(&self, user: &Identifier) -> AdminResult<bool> {
        let stmt = role_exists_postgres(user);
        let user_name = user.as_str().to_owned();
        let count = self.with_client(
            &self.target,
            consts::DEFAULT_DATABASE,
            move |client, _| async move {
                let row = client
                    .query_one(stmt.sql(), &[&user_name.as_str()])
                    .await
                    .map_err(|err| map_pg_error(&err))?;
                let count: i64 = row.try_get(0).map_err(|err| map_pg_error(&err))?;
                Ok(count)
            },
        )?;
        Ok(count > 0)
    }

    /// Whether `db` exists.
    ///
    /// Checked before creating a role in it: the grant step connects to `db`, so
    /// a missing database would otherwise surface as a connect failure
    /// (`DBL-8100`, "server unreachable") even though the server is up.
    pub fn database_exists(&self, db: &Identifier) -> AdminResult<bool> {
        let stmt = database_exists_postgres(db);
        let db_name = db.as_str().to_owned();
        let count = self.with_client(
            &self.target,
            consts::DEFAULT_DATABASE,
            move |client, _| async move {
                let row = client
                    .query_one(stmt.sql(), &[&db_name.as_str()])
                    .await
                    .map_err(|err| map_pg_error(&err))?;
                let count: i64 = row.try_get(0).map_err(|err| map_pg_error(&err))?;
                Ok(count)
            },
        )?;
        Ok(count > 0)
    }

    /// Resets the superuser password with the F-DBU-03 AC2 guarantee.
    ///
    /// Steps: generate, `ALTER ROLE`, verify a **new** connection with the new
    /// password, then persist through `sink`. If verification or persistence
    /// fails, the engine is rolled back to the old password; if that rollback
    /// also fails the returned error says the state is inconsistent and carries
    /// only booleans. The old and new secrets never appear in an error.
    pub fn reset_superuser_password(&self, sink: &dyn CredentialSink) -> AdminResult<SecretString> {
        let new_password = generate_password().map_err(|_| AdminError::PasswordGenerate)?;
        reset_with(self, sink, &self.target.password, new_password)
    }

    /// Opens a fresh connection and runs `SELECT 1`.
    fn probe(&self) -> AdminResult<()> {
        self.with_client(
            &self.target,
            consts::DEFAULT_DATABASE,
            |client, _| async move {
                client
                    .simple_query("SELECT 1")
                    .await
                    .map_err(|err| map_pg_error(&err))?;
                Ok(())
            },
        )
    }

    /// Runs one statement on `dbname`.
    fn execute(&self, target: &AdminTarget, dbname: &str, statement: &str) -> AdminResult<()> {
        self.execute_mapped(target, dbname, statement, map_pg_error)
    }

    fn execute_mapped(
        &self,
        target: &AdminTarget,
        dbname: &str,
        statement: &str,
        map: fn(&tokio_postgres::Error) -> AdminError,
    ) -> AdminResult<()> {
        let statement = statement.to_owned();
        self.with_client(target, dbname, move |client, _| async move {
            client
                .batch_execute(&statement)
                .await
                .map_err(|err| map(&err))
        })
    }

    fn execute_batch_mapped(
        &self,
        target: &AdminTarget,
        dbname: &str,
        statements: &[sql::SqlStatement],
        map: fn(&tokio_postgres::Error) -> AdminError,
    ) -> AdminResult<()> {
        let sqls: Vec<String> = statements.iter().map(|s| s.sql().to_owned()).collect();
        self.with_client(target, dbname, move |client, _| async move {
            for statement in sqls {
                client
                    .batch_execute(&statement)
                    .await
                    .map_err(|err| map(&err))?;
            }
            Ok(())
        })
    }

    /// Opens a connection to `dbname` and drives `f` to completion on a
    /// throwaway current-thread runtime, on a dedicated OS thread so this is
    /// safe to call from inside an existing tokio runtime (F3).
    fn with_client<T, F, Fut>(&self, target: &AdminTarget, dbname: &str, f: F) -> AdminResult<T>
    where
        T: Send + 'static,
        F: FnOnce(Client, String) -> Fut + Send + 'static,
        Fut: Future<Output = AdminResult<T>> + 'static,
    {
        let target = target.clone();
        let dbname_owned = dbname.to_owned();
        let run = move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| AdminError::Connect)?;
            runtime.block_on(async move {
                let (client, connection) = target
                    .config(&dbname_owned)
                    .connect(tokio_postgres::NoTls)
                    .await
                    .map_err(|err| map_connect_error(&err))?;
                tokio::spawn(async move {
                    let _ = connection.await;
                });
                f(client, dbname_owned).await
            })
        };
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            // Inside a runtime: move the blocking + block_on work to a fresh
            // OS thread so we never call block_on on a worker thread.
            match handle.runtime_flavor() {
                tokio::runtime::RuntimeFlavor::MultiThread => tokio::task::block_in_place(run),
                _ => std::thread::scope(|scope| {
                    scope.spawn(run).join().unwrap_or(Err(AdminError::Connect))
                }),
            }
        } else {
            run()
        }
    }
}

/// Maps a connection failure to [`AdminError`]. Wrong/stale credentials are
/// distinct from an unreachable engine (F2), and a server that answers with
/// `3D000` (`invalid_catalog_name`, unknown database) is a *reachable* server
/// naming a missing database, not an unreachable one. The driver text is
/// dropped.
fn map_connect_error(err: &tokio_postgres::Error) -> AdminError {
    match err.code().map(|code| code.code()) {
        Some("28P01" | "28000") => AdminError::AuthFailed,
        Some("3D000") => AdminError::DatabaseNotFound,
        _ => AdminError::Connect,
    }
}

/// Maps a `tokio-postgres` error to an [`AdminError`], keeping only the
/// five-character SQLSTATE and dropping the engine's message text.
fn map_pg_error(err: &tokio_postgres::Error) -> AdminError {
    let Some(state) = err.code() else {
        return AdminError::QueryFailed(None);
    };
    classify_sql_state(state.code())
}

fn classify_sql_state(code: &str) -> AdminError {
    match code {
        "3D000" => AdminError::DatabaseNotFound,
        "42P04" => AdminError::DatabaseExists,
        "55006" => AdminError::DatabaseInUse,
        "42710" => AdminError::UserExists,
        "42501" => AdminError::InsufficientPrivilege,
        other => AdminError::QueryFailed(SqlStateCode::parse(other)),
    }
}

/// Role DDL/DCL statements report `42704` (undefined_object) when the role is
/// missing. Map only at those call sites so other `42704`s (missing schema,
/// missing extension) remain `QueryFailed`.
fn map_role_error(err: &tokio_postgres::Error) -> AdminError {
    match err.code().map(|code| code.code()) {
        Some("42704") => AdminError::UserNotFound,
        _ => map_pg_error(err),
    }
}

pub use super::reset::PasswordEngine;

impl super::executor::AdminExecutor for PgAdmin {
    fn list_databases(&self) -> AdminResult<Vec<DatabaseInfo>> {
        PgAdmin::list_databases(self)
    }
    fn list_users(&self) -> AdminResult<Vec<UserInfo>> {
        PgAdmin::list_users(self)
    }
    fn create_database(&self, db: &Identifier) -> AdminResult<()> {
        PgAdmin::create_database(self, db)
    }
    fn drop_database(&self, db: &Identifier, force: bool) -> AdminResult<()> {
        PgAdmin::drop_database(self, db, force)
    }
    fn create_user(&self, user: &Identifier) -> AdminResult<SecretString> {
        PgAdmin::create_user(self, user)
    }
    fn database_exists(&self, db: &Identifier) -> AdminResult<bool> {
        PgAdmin::database_exists(self, db)
    }
    fn grant(&self, user: &Identifier, db: &Identifier, level: PrivilegeLevel) -> AdminResult<()> {
        PgAdmin::grant(self, user, db, level)
    }
    fn drop_user(&self, user: &Identifier) -> AdminResult<()> {
        PgAdmin::drop_user(self, user)
    }
    fn reset_superuser_password(&self, sink: &dyn CredentialSink) -> AdminResult<SecretString> {
        PgAdmin::reset_superuser_password(self, sink)
    }
}

impl PasswordEngine for PgAdmin {
    fn apply(&self, connect_as: &SecretString, target: &SecretString) -> AdminResult<()> {
        let role = Identifier::role(&self.target.superuser)
            .map_err(|_| AdminError::InvalidTarget(TargetProblem::EmptySuperuser))?;
        let credential = scram_credential(target);
        let stmt = alter_role_password_postgres(&role, &credential);
        let connector = PgAdmin::new(self.target.with_password(connect_as.clone()));
        connector.execute_mapped(
            &connector.target,
            consts::DEFAULT_DATABASE,
            stmt.sql(),
            map_role_error,
        )
    }

    fn verify(&self, password: &SecretString) -> AdminResult<()> {
        PgAdmin::new(self.target.with_password(password.clone())).probe()
    }
}
#[cfg(test)]
#[path = "postgres/tests.rs"]
mod tests;
