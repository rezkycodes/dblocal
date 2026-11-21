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

use super::error::{AdminError, AdminResult, RollbackState, SqlStateCode, TargetProblem};
use super::sql::{
    self, Dialect, Identifier, PrivilegeLevel, alter_password, create_database_postgres,
    create_user, database_connection_count_postgres, drop_database_postgres, drop_user_postgres,
    grant_privileges, list_databases_postgres, list_users_postgres, revoke_privileges,
    role_exists_postgres,
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
            .application_name("dblocal-admin")
            .options(format!(
                "-c statement_timeout={} -c lock_timeout={}",
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

/// Sink for persisting a newly applied credential.
///
/// The real implementation is the keychain, injected by the wiring task; tests
/// inject failures. It must never log the value it receives.
pub trait CredentialSink: Send + Sync {
    /// Stores the new password. Returns `Err` if persistence fails.
    fn store(&self, password: &SecretString) -> crate::error::Result<()>;
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
        let sql = drop_database_postgres(db);
        let statement = if force {
            format!("{} WITH (FORCE)", sql.sql())
        } else {
            sql.sql().to_owned()
        };
        if !force {
            self.ensure_no_connections(db)?;
        }
        self.execute(&self.target, consts::DEFAULT_DATABASE, &statement)
    }

    /// Counts live connections to `db`, refusing to drop when any exists.
    fn ensure_no_connections(&self, db: &Identifier) -> AdminResult<()> {
        let stmt = database_connection_count_postgres(db);
        let count = self.with_client(
            &self.target,
            consts::DEFAULT_DATABASE,
            move |client, _| async move {
                let row = client
                    .query_one(stmt.sql(), &[&db.as_str()])
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
        let stmt = create_user(Dialect::Postgres, user, password.expose());
        self.execute(&self.target, consts::DEFAULT_DATABASE, stmt.sql())?;
        Ok(password)
    }

    /// Drops a role.
    pub fn drop_user(&self, user: &Identifier) -> AdminResult<()> {
        let stmt = drop_user_postgres(user);
        self.execute(&self.target, consts::DEFAULT_DATABASE, stmt.sql())
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
        self.execute_batch(&self.target, db.as_str(), &statements)
    }

    /// Revokes every managed privilege from `user` on `db`.
    pub fn revoke(&self, user: &Identifier, db: &Identifier) -> AdminResult<()> {
        let statements = revoke_privileges(Dialect::Postgres, user, db);
        self.execute_batch(&self.target, db.as_str(), &statements)
    }

    /// Whether `user` exists.
    pub fn user_exists(&self, user: &Identifier) -> AdminResult<bool> {
        let stmt = role_exists_postgres(user);
        let count = self.with_client(
            &self.target,
            consts::DEFAULT_DATABASE,
            move |client, _| async move {
                let row = client
                    .query_one(stmt.sql(), &[&user.as_str()])
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

    /// `ALTER ROLE <superuser> PASSWORD <new>`.
    fn alter_superuser_password(&self, password: &SecretString) -> AdminResult<()> {
        let role = Identifier::role(&self.target.superuser)
            .map_err(|_| AdminError::InvalidTarget(TargetProblem::EmptySuperuser))?;
        let stmt = alter_password(Dialect::Postgres, &role, password.expose());
        self.execute(&self.target, consts::DEFAULT_DATABASE, stmt.sql())
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
        self.with_client(target, dbname, move |client, _| async move {
            client
                .batch_execute(statement)
                .await
                .map_err(|err| map_pg_error(&err))
        })
    }

    /// Runs a batch of statements in order on `dbname`, stopping at the first
    /// failure.
    fn execute_batch(
        &self,
        target: &AdminTarget,
        dbname: &str,
        statements: &[sql::SqlStatement],
    ) -> AdminResult<()> {
        let sqls: Vec<String> = statements.iter().map(|s| s.sql().to_owned()).collect();
        self.with_client(target, dbname, move |client, _| async move {
            for statement in sqls {
                client
                    .batch_execute(&statement)
                    .await
                    .map_err(|err| map_pg_error(&err))?;
            }
            Ok(())
        })
    }

    /// Opens a connection to `dbname` and drives `f` to completion on a
    /// throwaway current-thread runtime.
    fn with_client<T, F, Fut>(&self, target: &AdminTarget, dbname: &str, f: F) -> AdminResult<T>
    where
        F: FnOnce(Client, String) -> Fut,
        Fut: Future<Output = AdminResult<T>>,
    {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| AdminError::Connect)?;
        let dbname_owned = dbname.to_owned();
        runtime.block_on(async move {
            let (client, connection) = target
                .config(dbname)
                .connect(tokio_postgres::NoTls)
                .await
                .map_err(|_| AdminError::Connect)?;
            // The connection task lives only as long as this `block_on`.
            tokio::spawn(async move {
                let _ = connection.await;
            });
            f(client, dbname_owned).await
        })
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

/// The engine-facing half of a password reset, abstracted so the state machine
/// can be unit-tested with injected failures instead of a live server.
pub trait PasswordEngine {
    /// Applies `password` to the superuser role on the engine.
    fn apply(&self, password: &SecretString) -> AdminResult<()>;
    /// Opens a fresh connection and confirms `password` authenticates.
    fn verify(&self, password: &SecretString) -> AdminResult<()>;
}

impl PasswordEngine for PgAdmin {
    fn apply(&self, password: &SecretString) -> AdminResult<()> {
        self.alter_superuser_password(password)
    }

    fn verify(&self, password: &SecretString) -> AdminResult<()> {
        PgAdmin::new(self.target.with_password(password.clone())).probe()
    }
}

/// The pure F-DBU-03 AC2 state machine: apply → verify → store, rolling back to
/// `old_password` on any failure after the engine changed.
///
/// Every failure point returns a typed error carrying only booleans, and the
/// rollback path itself is exercised by the injected-failure tests below.
fn reset_with(
    engine: &dyn PasswordEngine,
    sink: &dyn CredentialSink,
    old_password: &SecretString,
    new_password: SecretString,
) -> AdminResult<SecretString> {
    // 1. Apply. If this fails nothing changed yet, so nothing to roll back.
    engine.apply(&new_password)?;

    let state = RollbackState {
        engine_changed: true,
        sink_failed: false,
        rollback_failed: false,
    };

    // 2. The engine now holds the new password. Prove it before persisting it.
    if engine.verify(&new_password).is_err() {
        return Err(rollback(
            engine,
            old_password,
            state,
            AdminError::PasswordVerify,
        ));
    }

    // 3. Persist. If the sink refuses, restore the old password so the stored
    //    credential and the engine never disagree.
    if sink.store(&new_password).is_err() {
        let state = RollbackState {
            sink_failed: true,
            ..state
        };
        return Err(rollback(
            engine,
            old_password,
            state,
            AdminError::CredentialStore,
        ));
    }

    Ok(new_password)
}

/// Restores the old password and returns the error that explains the failure.
///
/// When the restore also fails, the returned error is `RollbackFailed` and says
/// the state is inconsistent. Only booleans travel, never a secret.
fn rollback(
    engine: &dyn PasswordEngine,
    old_password: &SecretString,
    state: RollbackState,
    cause: AdminError,
) -> AdminError {
    let restored = engine
        .apply(old_password)
        .and_then(|()| engine.verify(old_password));
    match restored {
        Ok(()) => cause,
        Err(_) => AdminError::RollbackFailed(RollbackState {
            rollback_failed: true,
            ..state
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn target(host: &str) -> AdminResult<AdminTarget> {
        AdminTarget::new(
            host,
            5432,
            "postgres",
            SecretString::from("secret-password"),
        )
    }

    // ---- Injected doubles for the reset state machine --------------------

    struct FakeEngine {
        apply_fails: AtomicBool,
        verify_fails: AtomicBool,
        applied: std::sync::Mutex<Vec<String>>,
    }

    impl FakeEngine {
        fn new() -> Self {
            Self {
                apply_fails: AtomicBool::new(false),
                verify_fails: AtomicBool::new(false),
                applied: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn failing_apply() -> Self {
            let engine = Self::new();
            engine.apply_fails.store(true, Ordering::SeqCst);
            engine
        }

        fn failing_verify() -> Self {
            let engine = Self::new();
            engine.verify_fails.store(true, Ordering::SeqCst);
            engine
        }

        fn applied(&self) -> Vec<String> {
            self.applied.lock().expect("lock").clone()
        }
    }

    impl PasswordEngine for FakeEngine {
        fn apply(&self, password: &SecretString) -> AdminResult<()> {
            self.applied
                .lock()
                .expect("lock")
                .push(password.expose().to_owned());
            if self.apply_fails.load(Ordering::SeqCst) {
                return Err(AdminError::Connect);
            }
            Ok(())
        }

        fn verify(&self, password: &SecretString) -> AdminResult<()> {
            // Fail only the new password's verification, so the rollback's own
            // verification of the old password still succeeds.
            if self.verify_fails.load(Ordering::SeqCst)
                && password.expose() == "new-canary-password"
            {
                return Err(AdminError::Connect);
            }
            Ok(())
        }
    }

    struct StubSink {
        fail: bool,
    }

    impl CredentialSink for StubSink {
        fn store(&self, _password: &SecretString) -> crate::error::Result<()> {
            if self.fail {
                Err(crate::error::DblError::Secret {
                    reason: "keychain locked".to_owned(),
                })
            } else {
                Ok(())
            }
        }
    }

    fn new_password() -> SecretString {
        SecretString::from("new-canary-password")
    }

    #[test]
    fn reset_succeeds_and_returns_the_password_once() {
        let engine = FakeEngine::new();
        let sink = StubSink { fail: false };
        let old = SecretString::from("old-canary");
        let returned = reset_with(&engine, &sink, &old, new_password()).expect("reset");
        assert_eq!(returned.expose(), "new-canary-password");
        // Applied exactly once (no rollback).
        assert_eq!(engine.applied(), vec!["new-canary-password".to_owned()]);
    }

    #[test]
    fn apply_failure_changes_nothing_and_does_not_roll_back() {
        let engine = FakeEngine::failing_apply();
        let sink = StubSink { fail: false };
        let old = SecretString::from("old-canary");
        let err = reset_with(&engine, &sink, &old, new_password()).expect_err("apply fails");
        assert_eq!(err, AdminError::Connect);
        // Only the (failing) new password was attempted: no rollback write.
        assert_eq!(engine.applied(), vec!["new-canary-password".to_owned()]);
    }

    #[test]
    fn verify_failure_rolls_back_to_the_old_password() {
        let engine = FakeEngine::failing_verify();
        let sink = StubSink { fail: false };
        let old = SecretString::from("old-canary");
        let err = reset_with(&engine, &sink, &old, new_password()).expect_err("verify fails");
        assert_eq!(err, AdminError::PasswordVerify);
        // Applied new, then the old password was restored.
        assert_eq!(
            engine.applied(),
            vec!["new-canary-password".to_owned(), "old-canary".to_owned()]
        );
    }

    #[test]
    fn sink_failure_rolls_back_to_the_old_password() {
        let engine = FakeEngine::new();
        let sink = StubSink { fail: true };
        let old = SecretString::from("old-canary");
        let err = reset_with(&engine, &sink, &old, new_password()).expect_err("sink fails");
        assert_eq!(err, AdminError::CredentialStore);
        assert_eq!(
            engine.applied(),
            vec!["new-canary-password".to_owned(), "old-canary".to_owned()]
        );
    }

    #[test]
    fn a_rollback_that_also_fails_reports_inconsistent_state_without_secrets() {
        struct FailsOnRollback {
            applied: std::sync::Mutex<Vec<String>>,
        }
        impl PasswordEngine for FailsOnRollback {
            fn apply(&self, password: &SecretString) -> AdminResult<()> {
                self.applied
                    .lock()
                    .expect("lock")
                    .push(password.expose().to_owned());
                if password.expose() == "old-canary" {
                    Err(AdminError::Connect)
                } else {
                    Ok(())
                }
            }
            fn verify(&self, _password: &SecretString) -> AdminResult<()> {
                Ok(())
            }
        }
        let engine = FailsOnRollback {
            applied: std::sync::Mutex::new(Vec::new()),
        };
        let sink = StubSink { fail: true };
        let old = SecretString::from("old-canary");
        let err = reset_with(&engine, &sink, &old, new_password()).expect_err("rollback fails");
        match &err {
            AdminError::RollbackFailed(state) => {
                assert!(state.engine_changed);
                assert!(state.sink_failed);
                assert!(state.rollback_failed);
            }
            other => panic!("expected RollbackFailed, got {other:?}"),
        }
        let surfaces = format!("{err:?} {err} {}", err.user_message());
        assert!(!surfaces.contains("new-canary-password"));
        assert!(!surfaces.contains("old-canary"));
    }

    #[test]
    fn loopback_hosts_are_accepted() {
        for host in ["127.0.0.1", "::1", "localhost", "LOCALHOST"] {
            let target = target(host).expect(host);
            assert_eq!(target.host(), host);
        }
    }

    #[test]
    fn remote_hosts_are_rejected() {
        for host in [
            "0.0.0.0",
            "192.168.1.5",
            "10.0.0.1",
            "example.com",
            "::",
            "",
        ] {
            let err = target(host).expect_err(host);
            assert!(
                matches!(
                    err,
                    AdminError::NonLoopbackHost
                        | AdminError::InvalidTarget(TargetProblem::EmptyHost)
                ),
                "{host}: {err:?}"
            );
        }
    }

    #[test]
    fn target_validation_rejects_empty_superuser_and_zero_port() {
        let err = AdminTarget::new("127.0.0.1", 5432, "", SecretString::from("x"))
            .expect_err("empty superuser");
        assert_eq!(
            err,
            AdminError::InvalidTarget(TargetProblem::EmptySuperuser)
        );

        let err = AdminTarget::new("127.0.0.1", 0, "postgres", SecretString::from("x"))
            .expect_err("zero port");
        assert_eq!(err, AdminError::InvalidTarget(TargetProblem::ZeroPort));
    }

    #[test]
    fn dial_host_pins_localhost_to_loopback() {
        assert_eq!(target("localhost").expect("valid").dial_host(), "127.0.0.1");
        assert_eq!(target("127.0.0.1").expect("valid").dial_host(), "127.0.0.1");
        assert_eq!(target("::1").expect("valid").dial_host(), "::1");
    }

    #[test]
    fn config_sets_timeouts_and_never_debug_leaks_the_password() {
        let target = target("127.0.0.1")
            .expect("valid")
            .with_timeouts(Duration::from_secs(2), Duration::from_secs(7));
        assert_eq!(target.connect_timeout(), Duration::from_secs(2));
        assert_eq!(target.statement_timeout(), Duration::from_secs(7));
        let debug = format!("{target:?}");
        assert!(!debug.contains("secret-password"));
        assert!(debug.contains("[REDACTED]"));
        let cfg = target.config("postgres");
        assert_eq!(cfg.get_dbname(), Some("postgres"));
        assert_eq!(cfg.get_ports(), &[5432]);
    }

    #[test]
    fn sql_states_map_to_typed_errors() {
        assert_eq!(classify_sql_state("3D000"), AdminError::DatabaseNotFound);
        assert_eq!(classify_sql_state("42P04"), AdminError::DatabaseExists);
        assert_eq!(classify_sql_state("55006"), AdminError::DatabaseInUse);
        assert_eq!(classify_sql_state("42710"), AdminError::UserExists);
        assert_eq!(
            classify_sql_state("42501"),
            AdminError::InsufficientPrivilege
        );
        assert_eq!(
            classify_sql_state("XX000"),
            AdminError::QueryFailed(SqlStateCode::parse("XX000"))
        );
        // A malformed code degrades to the payload-free variant.
        assert_eq!(classify_sql_state("bad!"), AdminError::QueryFailed(None));
    }
}
