//! MySQL / MariaDB admin execution over the instance unix socket (unix) or
//! loopback TCP (Windows).
//!
//! On unix, every connection goes over the instance's **unix socket**, never
//! TCP. This is deliberate and unconditional: the socket is a secure
//! (never-transmitted) channel, so no byte — the password included — crosses the
//! loopback TCP stream, and it routes admin as `root@localhost`, the only
//! account MariaDB's bootstrap configures. (`mysql_async` would itself perform
//! the `caching_sha2_password` RSA public-key exchange over TCP, so the socket is
//! not *required* to complete the handshake — but it is the only path that keeps
//! the traffic off TCP.) The password never appears in argv, a log line, an error
//! or `Debug` output.
//!
//! On Windows there is no unix socket: `mysqld` does not create one and
//! `mysql_async`'s `Conn::new` returns `NamedPipesDisabled` for any `Opts` that
//! carries a socket path. Admin therefore dials `127.0.0.1:<instance port>`,
//! which the adapters' own `my.cnf`/`my.ini` binds (`bind-address=127.0.0.1`,
//! never `0.0.0.0`), and completes `caching_sha2_password` with the driver's RSA
//! public-key exchange. See [`Transport`] for the one decision and
//! `docs/architecture/admin.md` for the residual loopback-TCP risk.
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
use std::path::{Path, PathBuf};
use std::time::Duration;

use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Opts, OptsBuilder, ServerError};

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

#[cfg(test)]
mod tests;

/// The host an admin-managed account is created on. A client connecting with the
/// service URI does so over TCP to `127.0.0.1`, so a managed account lives at
/// `user@'127.0.0.1'` (not `localhost`, which is the socket account). The same
/// literal is the loopback TCP transport host, so the account and the transport
/// can never drift apart.
pub const MANAGED_ACCOUNT_HOST: &str = "127.0.0.1";

/// How admin reaches the engine.
///
/// A closed two-variant value, chosen once by
/// [`Transport::for_platform`]: a unix host always uses the socket, a non-unix
/// host always uses loopback TCP. There is no runtime fallback between them, so
/// a unix host can never be moved onto TCP by a missing socket and a Windows
/// host can never be moved onto a named pipe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transport {
    /// The instance's unix socket.
    Socket(PathBuf),
    /// Loopback TCP on the instance port, dialled as the literal `127.0.0.1`.
    Loopback {
        /// The instance's TCP port.
        port: u16,
    },
}

impl Transport {
    /// The transport for one platform.
    ///
    /// `unix` is the platform fact (`cfg!(unix)` in production, a parameter so
    /// both arms are provable on one host). On unix the socket wins even when a
    /// port is available; off unix the socket path is discarded.
    pub fn for_platform(unix: bool, socket: &Path, port: u16) -> Self {
        if unix {
            Self::Socket(socket.to_path_buf())
        } else {
            Self::Loopback { port }
        }
    }
}

/// Connection description for MySQL/MariaDB admin operations.
///
/// Immutable: the builders return a new value.
#[derive(Clone)]
pub struct MysqlTarget {
    /// The instance's unix socket, reported for diagnostics. It is only dialled
    /// on the [`Transport::Socket`] arm.
    socket: PathBuf,
    /// The instance's TCP port (dialled on the loopback arm only).
    port: u16,
    /// The transport this target dials: the socket on unix, loopback TCP on
    /// Windows (see [`MysqlTarget::opts`]).
    transport: Transport,
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

    /// Creates a target for this platform that dials `socket` (unix) or
    /// `127.0.0.1:<port>` (non-unix) as `superuser`.
    pub fn new(
        socket: impl Into<PathBuf>,
        port: u16,
        superuser: impl Into<String>,
        password: SecretString,
    ) -> Self {
        let socket = socket.into();
        Self {
            transport: Transport::for_platform(cfg!(unix), &socket, port),
            socket,
            port,
            superuser: superuser.into(),
            password,
            connect_timeout: Self::DEFAULT_CONNECT_TIMEOUT,
            statement_timeout: Self::DEFAULT_STATEMENT_TIMEOUT,
        }
    }

    /// The unix socket this instance configured. Only dialled when
    /// [`transport`](Self::transport) is [`Transport::Socket`].
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// The instance TCP port (the loopback transport's port off unix).
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The transport this target uses.
    pub fn transport(&self) -> &Transport {
        &self.transport
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

    /// Returns a copy whose transport is `transport`.
    ///
    /// Only unit tests use it, to prove the non-unix arm on a unix host:
    /// production never chooses a transport, [`MysqlTarget::new`] derives it
    /// from the platform alone.
    #[cfg(test)]
    fn with_transport(mut self, transport: Transport) -> Self {
        self.transport = transport;
        self
    }

    /// The `mysql_async` connection options for this target.
    ///
    /// The arm is fully determined by [`Self::transport`], and each arm pins the
    /// driver away from the other one:
    ///
    /// - Socket: the socket is set and `prefer_socket` is on (harmless there),
    ///   so `mysql_async` makes its socket connection and never looks at a TCP
    ///   address.
    /// - Loopback: no socket is set and `prefer_socket` is **off**, so the driver
    ///   does not read `@@socket` and silently reconnect over a named pipe or a
    ///   `localhost` resolver; the host is the literal `127.0.0.1`.
    fn opts(&self, db_name: Option<&str>) -> Opts {
        let builder = OptsBuilder::default()
            .user(Some(self.superuser.as_str()))
            .pass(Some(self.password.expose()))
            .stmt_cache_size(0)
            .db_name(Some(db_name.unwrap_or("")));
        let builder = match &self.transport {
            Transport::Socket(socket) => builder
                .socket(Some(socket.to_string_lossy().into_owned()))
                .prefer_socket(true),
            Transport::Loopback { port } => builder
                .ip_or_hostname(MANAGED_ACCOUNT_HOST)
                .tcp_port(*port)
                .prefer_socket(false),
        };
        Opts::from(builder)
    }
}

impl std::fmt::Debug for MysqlTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MysqlTarget")
            .field("socket", &self.socket)
            .field("transport", &self.transport)
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

    /// Runs several role statements on **one** connection (so a password change
    /// that invalidates the connect credential on some accounts does not break the
    /// rest). A statement that fails because its host account does not exist
    /// (`1396`) is skipped; any other failure is returned.
    fn exec_user_batch(&self, statements: &[String]) -> AdminResult<()> {
        let statements = statements.to_vec();
        self.with_client(self.target.opts(None), move |mut conn| async move {
            let _ = conn.query_drop("SET sql_log_off = 1").await;
            for statement in &statements {
                match conn.query_drop(statement).await {
                    Ok(()) => {}
                    Err(err) if server_error(&err).is_some_and(|s| s.code == 1396) => {}
                    Err(err) => return Err(map_user_error(err)),
                }
            }
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

    /// Opens a fresh connection and runs `SELECT 1`.
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
        // Build every loopback `ALTER USER` up front and run them on **one**
        // connection opened with the connect credential. Reconnecting per host
        // would fail the second host: after the first ALTER the engine no longer
        // accepts the connect password on the accounts already changed.
        let statements: Vec<String> = MYSQL_LOOPBACK_HOSTS
            .iter()
            .map(|host| {
                alter_password_mysql_at(&user, host, target.expose())
                    .sql()
                    .to_owned()
            })
            .collect();
        // Keep every loopback account of the superuser in sync, so the socket
        // (`localhost`) and the TCP URI (`127.0.0.1`, `::1`) all keep working.
        let connector = MysqlAdmin::new(self.target.with_password(connect_as.clone()));
        connector.exec_user_batch(&statements)
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
