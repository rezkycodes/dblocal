//! MySQL / MariaDB admin execution over the instance unix socket.
//!
//! All connections go over the instance's **unix socket**, not TCP. This is
//! deliberate: MySQL 8.4's default `caching_sha2_password` over plaintext TCP
//! demands an RSA public-key exchange whose result is still sent unencrypted, so
//! the socket (a secure channel) is the only loopback path that keeps the
//! password off the wire; MariaDB's bootstrap only configures `root@localhost`,
//! reachable solely through the socket. The password never appears in argv, a log
//! line, an error or `Debug` output.
//!
//! Statements that carry a password literal (`CREATE USER ... IDENTIFIED BY`) run
//! with `sql_log_off = 1` on the session so a failing statement never reaches the
//! general query log. That variable needs `SYSTEM_VARIABLES_ADMIN` (or super);
//! the admin connection is the superuser, so it is available. The set is
//! best-effort: if it is refused the statement still runs, and the unix socket
//! already keeps the value from a plaintext network observer.
//!
//! Each public method opens its own short-lived connection, so the struct holds
//! no live socket and stays `Send + Sync`.

use std::future::Future;
use std::time::Duration;

use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Opts, OptsBuilder, ServerError};

use crate::consts;
use crate::secret::SecretString;
use crate::secrets::generate_password;

use super::error::{AdminError, AdminResult, TargetProblem};
use super::postgres::{DatabaseInfo, UserInfo};
use super::reset::{self, CredentialSink, PasswordEngine};
use super::sql::{
    self, Identifier, MYSQL_LOOPBACK_HOSTS, PrivilegeLevel, alter_password_mysql_at,
    create_database_mysql, create_user_mysql_at, drop_database_mysql, drop_user_mysql_at,
    grant_privileges_mysql_at, is_mysql_system_schema,
};

/// The loopback host an admin-managed account is created on. The socket
/// authenticates as `localhost`, so that is where a managed account lives.
pub const MANAGED_ACCOUNT_HOST: &str = "localhost";

/// Connection description for MySQL/MariaDB admin operations.
///
/// Immutable: the builders return a new value.
#[derive(Clone)]
pub struct MysqlTarget {
    socket: std::path::PathBuf,
    superuser: String,
    password: SecretString,
    connect_timeout: Duration,
    statement_timeout: Duration,
}

impl MysqlTarget {
    /// Default connect timeout.
    pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
    /// Default statement timeout.
    pub const DEFAULT_STATEMENT_TIMEOUT: Duration = Duration::from_secs(30);

    /// Creates a target that dials `socket` as `superuser`.
    pub fn new(
        socket: impl Into<std::path::PathBuf>,
        superuser: impl Into<String>,
        password: SecretString,
    ) -> Self {
        Self {
            socket: socket.into(),
            superuser: superuser.into(),
            password,
            connect_timeout: Self::DEFAULT_CONNECT_TIMEOUT,
            statement_timeout: Self::DEFAULT_STATEMENT_TIMEOUT,
        }
    }

    /// The unix socket this target dials.
    pub fn socket(&self) -> &std::path::Path {
        &self.socket
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

    /// Returns a copy that authenticates with `password` (the current password
    /// during a reset, and the connect credential for the applied target).
    fn with_password(&self, password: SecretString) -> Self {
        let mut next = self.clone();
        next.password = password;
        next
    }

    /// The `mysql_async` connection options for this target.
    ///
    /// The socket is set explicitly so `mysql_async` never falls back to TCP
    /// (which would send the password unencrypted for `caching_sha2_password`).
    fn opts(&self, db_name: Option<&str>) -> Opts {
        let builder = OptsBuilder::default()
            .user(Some(self.superuser.as_str()))
            .pass(Some(self.password.expose()))
            .socket(Some(self.socket.to_string_lossy().into_owned()))
            .prefer_socket(true)
            .stmt_cache_size(0)
            .db_name(Some(db_name.unwrap_or("")));
        Opts::from(builder)
    }
}

impl std::fmt::Debug for MysqlTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MysqlTarget")
            .field("socket", &self.socket)
            .field("superuser", &self.superuser)
            .field("password", &"[REDACTED]")
            .field("connect_timeout", &self.connect_timeout)
            .field("statement_timeout", &self.statement_timeout)
            .finish()
    }
}

/// MySQL/MariaDB admin client over one [`MysqlTarget`].
#[derive(Debug, Clone)]
pub struct MysqlAdmin {
    target: MysqlTarget,
}

impl MysqlAdmin {
    /// Builds an admin client over a validated target.
    pub fn new(target: MysqlTarget) -> Self {
        Self { target }
    }

    /// The target this client uses.
    pub fn target(&self) -> &MysqlTarget {
        &self.target
    }

    /// Lists non-system databases with their size in bytes.
    pub fn list_databases(&self) -> AdminResult<Vec<DatabaseInfo>> {
        self.with_client(self.target.opts(None), |mut conn| async move {
            let rows: Vec<(String, Option<i64>)> = conn
                .query(sql::list_databases_mysql().sql())
                .await
                .map_err(map_query_error)?;
            Ok(rows
                .into_iter()
                .map(|(name, size)| DatabaseInfo::new(name, String::new(), size.unwrap_or(0)))
                .collect())
        })
    }

    /// Lists login-capable accounts (`User`+`Host`), deduplicated to the account
    /// name. A managed account is never a superuser.
    pub fn list_users(&self) -> AdminResult<Vec<UserInfo>> {
        self.with_client(self.target.opts(None), |mut conn| async move {
            let rows: Vec<(String, String)> = conn
                .query(sql::list_users_mysql().sql())
                .await
                .map_err(map_query_error)?;
            let mut out = Vec::new();
            for (user, host) in rows {
                // Collapse `user@localhost` and `user@127.0.0.1` into one row.
                if out.iter().any(|u: &UserInfo| u.name() == user) {
                    continue;
                }
                let _ = host;
                out.push(UserInfo::new(user, true, false));
            }
            Ok(out)
        })
    }

    /// Creates a database with `utf8mb4` / `utf8mb4_unicode_ci` (PRD F-DBU-01 AC2).
    pub fn create_database(&self, db: &Identifier) -> AdminResult<()> {
        reject_system_schema(db)?;
        self.exec_literal(create_database_mysql(db).sql())
    }

    /// Drops a database, refusing a system schema and (without `force`) a
    /// database with a live session.
    pub fn drop_database(&self, db: &Identifier, force: bool) -> AdminResult<()> {
        reject_system_schema(db)?;
        if !force {
            self.ensure_no_connections(db)?;
        }
        self.exec_literal(drop_database_mysql(db).sql())
    }

    /// Whether `db` exists.
    pub fn database_exists(&self, db: &Identifier) -> AdminResult<bool> {
        let name = db.as_str().to_owned();
        self.with_count(sql::database_exists_mysql(db).sql(), name)
    }

    /// Refuses to drop `db` while a session is connected to it.
    fn ensure_no_connections(&self, db: &Identifier) -> AdminResult<()> {
        let name = db.as_str().to_owned();
        if self.with_count(sql::database_connection_count_mysql(db).sql(), name)? {
            return Err(AdminError::DatabaseInUse);
        }
        Ok(())
    }

    /// Runs a `COUNT(*)` query bound to one name parameter.
    fn with_count(&self, statement: &str, name: String) -> AdminResult<bool> {
        let statement = statement.to_owned();
        self.with_client(self.target.opts(None), move |mut conn| async move {
            let count: Option<i64> = conn
                .exec_first(&statement, (name.as_str(),))
                .await
                .map_err(map_query_error)?;
            Ok(count.unwrap_or(0) > 0)
        })
    }

    /// Creates a login account `user@'<host>'` with a freshly generated password.
    /// Returns the password once.
    pub fn create_user_at(&self, user: &Identifier, host: &str) -> AdminResult<SecretString> {
        let password = generate_password().map_err(|_| AdminError::PasswordGenerate)?;
        let stmt = create_user_mysql_at(user, host, password.expose());
        self.exec_user(stmt.sql())?;
        Ok(password)
    }

    /// Grants `level` on `db` to `user@'<host>'`.
    pub fn grant(
        &self,
        user: &Identifier,
        db: &Identifier,
        level: PrivilegeLevel,
        host: &str,
    ) -> AdminResult<()> {
        for stmt in grant_privileges_mysql_at(user, db, level, host) {
            self.exec_user(stmt.sql())?;
        }
        Ok(())
    }

    /// Drops the account `user@'<host>'` (used to undo a half-created user).
    pub fn drop_user_at(&self, user: &Identifier, host: &str) -> AdminResult<()> {
        self.exec_user(drop_user_mysql_at(user, host).sql())
    }

    /// Resets the superuser password with the F-DBU-03 AC2 guarantee.
    pub fn reset_superuser_password(&self, sink: &dyn CredentialSink) -> AdminResult<SecretString> {
        let new_password = generate_password().map_err(|_| AdminError::PasswordGenerate)?;
        reset::reset_with(self, sink, &self.target.password, new_password)
    }

    /// Runs one statement with no password literal, mapping generic query errors.
    fn exec_literal(&self, statement: &str) -> AdminResult<()> {
        let statement = statement.to_owned();
        self.with_client(self.target.opts(None), move |mut conn| async move {
            conn.query_drop(&statement).await.map_err(map_query_error)?;
            Ok(())
        })
    }

    /// Runs one role statement with `sql_log_off` set first so a password literal
    /// never reaches the query log; maps role errors.
    fn exec_user(&self, statement: &str) -> AdminResult<()> {
        let statement = statement.to_owned();
        self.with_client(self.target.opts(None), move |mut conn| async move {
            let _ = conn.query_drop("SET sql_log_off = 1").await;
            conn.query_drop(&statement).await.map_err(map_user_error)?;
            Ok(())
        })
    }

    /// Opens a connection with `opts`, runs `f`, and maps a connection failure.
    fn with_client<T, F, Fut>(&self, opts: Opts, f: F) -> AdminResult<T>
    where
        T: Send + 'static,
        F: FnOnce(Conn) -> Fut + Send + 'static,
        Fut: Future<Output = AdminResult<T>> + 'static,
    {
        // Build a throwaway current-thread runtime on a dedicated OS thread so the
        // call is safe from any caller runtime (same contract as `PgAdmin`).
        let run = move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| AdminError::Connect)?;
            runtime.block_on(async move {
                let conn = Conn::new(opts).await.map_err(map_connect_error)?;
                f(conn).await
            })
        };
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
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

    /// Opens a fresh socket connection and runs `SELECT 1`.
    fn probe(&self) -> AdminResult<()> {
        self.with_client(self.target.opts(None), |mut conn| async move {
            conn.query_drop("SELECT 1")
                .await
                .map_err(map_connect_error)?;
            Ok(())
        })
    }
}

/// Rejects a system schema before a statement can reach it.
fn reject_system_schema(db: &Identifier) -> AdminResult<()> {
    if is_mysql_system_schema(db.as_str()) {
        return Err(AdminError::SystemSchema);
    }
    Ok(())
}

/// The server error inside `err`, if it is a `Server` error.
fn server_error(err: &mysql_async::Error) -> Option<&ServerError> {
    match err {
        mysql_async::Error::Server(server) => Some(server),
        _ => None,
    }
}

/// Maps a connection failure to [`AdminError`], dropping the driver text.
fn map_connect_error(err: mysql_async::Error) -> AdminError {
    if let Some(server) = server_error(&err) {
        match server.code {
            1045 => return AdminError::AuthFailed, // ER_ACCESS_DENIED_ERROR
            1049 => return AdminError::DatabaseNotFound, // ER_BAD_DB_ERROR
            _ => {}
        }
    }
    AdminError::Connect
}

/// Maps a statement error, keeping only known MySQL error codes and dropping the
/// engine text. Unrecognised codes are payload-free.
fn map_query_error(err: mysql_async::Error) -> AdminError {
    let Some(server) = server_error(&err) else {
        return AdminError::QueryFailed(None);
    };
    match server.code {
        1007 => AdminError::DatabaseExists,   // ER_DB_CREATE_EXISTS
        1008 => AdminError::DatabaseNotFound, // ER_DB_DROP_EXISTS
        1044 | 1142 => AdminError::InsufficientPrivilege, // access/table denied
        1049 => AdminError::DatabaseNotFound, // ER_BAD_DB_ERROR
        _ => AdminError::QueryFailed(None),
    }
}

/// Maps a role statement error. MySQL reports `1396` (`ER_CANNOT_USER`) for both
/// a duplicate create and a missing drop; the text distinguishes them, and it is
/// safe to inspect because it never contains the password.
fn map_user_error(err: mysql_async::Error) -> AdminError {
    if let Some(server) = server_error(&err)
        && server.code == 1396
    {
        if server.message.contains("doesn't exist") || server.message.contains("does not exist") {
            return AdminError::UserNotFound;
        }
        return AdminError::UserExists;
    }
    map_query_error(err)
}

impl PasswordEngine for MysqlAdmin {
    fn apply(&self, connect_as: &SecretString, target: &SecretString) -> AdminResult<()> {
        let user = Identifier::role(&self.target.superuser)
            .map_err(|_| AdminError::InvalidTarget(TargetProblem::EmptySuperuser))?;
        // Keep every loopback account of the superuser in sync, so the socket
        // (`localhost`) and the TCP URI (`127.0.0.1`, `::1`) all keep working.
        for host in MYSQL_LOOPBACK_HOSTS {
            let stmt = alter_password_mysql_at(&user, host, target.expose());
            let connector = MysqlAdmin::new(self.target.with_password(connect_as.clone()));
            match connector.exec_user(stmt.sql()) {
                // A host account that does not exist answers 1396/unknown user;
                // only the accounts that do exist need to move to the new value.
                Ok(()) | Err(AdminError::UserExists) | Err(AdminError::UserNotFound) => {}
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }

    fn verify(&self, password: &SecretString) -> AdminResult<()> {
        MysqlAdmin::new(self.target.with_password(password.clone())).probe()
    }
}

impl super::executor::AdminExecutor for MysqlAdmin {
    fn list_databases(&self) -> AdminResult<Vec<DatabaseInfo>> {
        MysqlAdmin::list_databases(self)
    }
    fn list_users(&self) -> AdminResult<Vec<UserInfo>> {
        MysqlAdmin::list_users(self)
    }
    fn create_database(&self, db: &Identifier) -> AdminResult<()> {
        MysqlAdmin::create_database(self, db)
    }
    fn drop_database(&self, db: &Identifier, force: bool) -> AdminResult<()> {
        MysqlAdmin::drop_database(self, db, force)
    }
    fn create_user(&self, user: &Identifier) -> AdminResult<SecretString> {
        MysqlAdmin::create_user_at(self, user, MANAGED_ACCOUNT_HOST)
    }
    fn database_exists(&self, db: &Identifier) -> AdminResult<bool> {
        MysqlAdmin::database_exists(self, db)
    }
    fn grant(&self, user: &Identifier, db: &Identifier, level: PrivilegeLevel) -> AdminResult<()> {
        MysqlAdmin::grant(self, user, db, level, MANAGED_ACCOUNT_HOST)
    }
    fn drop_user(&self, user: &Identifier) -> AdminResult<()> {
        MysqlAdmin::drop_user_at(self, user, MANAGED_ACCOUNT_HOST)
    }
    fn reset_superuser_password(&self, sink: &dyn CredentialSink) -> AdminResult<SecretString> {
        MysqlAdmin::reset_superuser_password(self, sink)
    }
}

/// `database_exists` and `with_count` return a bool from a count; this lets the
/// same helper serve the "exists" and "in use" queries. Kept private.
#[allow(dead_code)]
fn _conn_timeout_defaults() -> Duration {
    Duration::from_secs(consts::ADMIN_CONNECT_TIMEOUT_SECS)
}
