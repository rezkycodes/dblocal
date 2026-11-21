//! Pure SQL builders for database and user administration.
//!
//! Every function here is side-effect free. Identifiers are validated against a
//! strict ASCII allowlist and quoted per dialect. Values that the engine accepts
//! as a bound parameter are returned in [`SqlStatement::params`] instead of
//! being interpolated; the only values that must be inline are DDL identifiers
//! (`CREATE DATABASE`, `CREATE ROLE`, `GRANT`) and the `CREATE ROLE PASSWORD`
//! literal, each produced by a tested quoting function.
//!
//! PostgreSQL and MySQL/MariaDB builders are both implemented and unit-tested
//! here. The MySQL ones are not executed in this task (the adapter lands later);
//! `docs/architecture/admin.md` records the exact SQL for each privilege level.

use super::error::{AdminError, AdminResult, IdentifierReject};

/// SQL dialect for quoting and privilege statements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// PostgreSQL.
    Postgres,
    /// MySQL / MariaDB.
    MySql,
}

/// Privilege level granted to a managed database user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivilegeLevel {
    /// CONNECT/USAGE + SELECT on tables and sequences, plus matching default
    /// privileges for objects created later.
    ReadOnly,
    /// ReadOnly + INSERT/UPDATE/DELETE on tables and USAGE/UPDATE on sequences.
    ReadWrite,
}

/// A validated SQL identifier.
///
/// The only constructor is [`Identifier::new`], which enforces the allowlist, so
/// an `Identifier` can only ever hold `[A-Za-z][A-Za-z0-9_]*` up to
/// [`Identifier::MAX_LEN`] bytes and never a reserved engine name. Quoting an
/// `Identifier` therefore cannot produce a second statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identifier(String);

impl Identifier {
    /// Maximum identifier length accepted by this module.
    ///
    /// PostgreSQL truncates at 63 bytes; MySQL allows 64. Sharing the smaller
    /// limit keeps one name valid on both engines.
    pub const MAX_LEN: usize = 63;

    /// Validates and wraps a raw database or object identifier.
    ///
    /// Rejects the reserved names `postgres`, `template0` and `template1` in
    /// addition to the allowlist.
    pub fn new(raw: &str) -> AdminResult<Self> {
        validate_identifier(raw)?;
        Ok(Self(raw.to_owned()))
    }

    /// Validates and wraps a raw **role** identifier.
    ///
    /// Role names use the same allowlist and length limit but skip the reserved
    /// database list: `postgres` is the default superuser role and must be
    /// addressable (`ALTER ROLE postgres ...`). Nothing else about the allowlist
    /// is relaxed, so the value is still safe to quote.
    pub fn role(raw: &str) -> AdminResult<Self> {
        validate_identifier_shape(raw)?;
        Ok(Self(raw.to_owned()))
    }

    /// Returns the validated identifier as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Identifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn validate_identifier(raw: &str) -> AdminResult<()> {
    validate_identifier_shape(raw)?;
    if is_reserved(raw) {
        return Err(AdminError::InvalidIdentifier(IdentifierReject::Reserved));
    }
    Ok(())
}

/// The allowlist and length rules, without the reserved-name check.
fn validate_identifier_shape(raw: &str) -> AdminResult<()> {
    if raw.is_empty() {
        return Err(AdminError::InvalidIdentifier(IdentifierReject::Empty));
    }
    if raw.len() > Identifier::MAX_LEN {
        return Err(AdminError::InvalidIdentifier(IdentifierReject::TooLong));
    }
    let bytes = raw.as_bytes();
    if bytes.first().is_some_and(u8::is_ascii_digit) {
        return Err(AdminError::InvalidIdentifier(
            IdentifierReject::StartsWithDigit,
        ));
    }
    if !bytes
        .iter()
        .all(|b| b.is_ascii_alphabetic() || b.is_ascii_digit() || *b == b'_')
    {
        return Err(AdminError::InvalidIdentifier(
            IdentifierReject::BadCharacter,
        ));
    }
    Ok(())
}

fn is_reserved(raw: &str) -> bool {
    matches!(
        raw.to_ascii_lowercase().as_str(),
        "postgres" | "template0" | "template1"
    )
}

/// Quotes an identifier for the given dialect.
pub fn quote_identifier(dialect: Dialect, ident: &Identifier) -> String {
    match dialect {
        Dialect::Postgres => quote_postgres_ident(ident.as_str()),
        Dialect::MySql => quote_mysql_ident(ident.as_str()),
    }
}

/// Quotes a raw string as a PostgreSQL identifier (`"name"`, `""` escapes).
fn quote_postgres_ident(raw: &str) -> String {
    let escaped = raw.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

/// Quotes a raw string as a MySQL identifier (`` `name` ``, ` `` ` escapes).
fn quote_mysql_ident(raw: &str) -> String {
    let escaped = raw.replace('`', "``");
    format!("`{escaped}`")
}

/// Quotes a value as a PostgreSQL string literal.
///
/// Uses the `E'...'` form so the result is unambiguous regardless of the
/// server's `standard_conforming_strings` setting: every backslash is doubled
/// and every single quote is doubled, and a NUL byte (which cannot appear in
/// the wire protocol's C string) becomes `\x00`.
pub fn quote_postgres_literal(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 3);
    out.push_str("E'");
    for ch in value.chars() {
        match ch {
            '\'' => out.push_str("''"),
            '\\' => out.push_str("\\\\"),
            '\0' => out.push_str("\\x00"),
            other => out.push(other),
        }
    }
    out.push('\'');
    out
}

/// Quotes a value as a MySQL string literal (backslash escapes).
pub fn quote_mysql_literal(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for ch in value.chars() {
        match ch {
            '\'' => out.push_str("''"),
            '\\' => out.push_str("\\\\"),
            '\0' => out.push_str("\\0"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{001a}' => out.push_str("\\Z"),
            other => out.push(other),
        }
    }
    out.push('\'');
    out
}

/// A bound parameter value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlParam {
    /// UTF-8 text parameter.
    Text(String),
}

/// A built SQL statement with optional bound parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlStatement {
    sql: String,
    params: Vec<SqlParam>,
}

impl SqlStatement {
    /// Creates a statement with no bound parameters.
    pub fn new(sql: impl Into<String>) -> Self {
        Self {
            sql: sql.into(),
            params: Vec::new(),
        }
    }

    /// Creates a statement with bound parameters.
    pub fn with_params(sql: impl Into<String>, params: Vec<SqlParam>) -> Self {
        Self {
            sql: sql.into(),
            params,
        }
    }

    /// The SQL text.
    pub fn sql(&self) -> &str {
        &self.sql
    }

    /// Bound parameters in order.
    pub fn params(&self) -> &[SqlParam] {
        &self.params
    }
}

/// Builds `CREATE DATABASE` for PostgreSQL, UTF-8 with `template0`.
pub fn create_database_postgres(db: &Identifier) -> SqlStatement {
    let name = quote_identifier(Dialect::Postgres, db);
    SqlStatement::new(format!(
        "CREATE DATABASE {name} WITH ENCODING 'UTF8' TEMPLATE template0"
    ))
}

/// Builds `CREATE DATABASE` for MySQL/MariaDB, `utf8mb4_unicode_ci`.
pub fn create_database_mysql(db: &Identifier) -> SqlStatement {
    let name = quote_identifier(Dialect::MySql, db);
    SqlStatement::new(format!(
        "CREATE DATABASE {name} CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci"
    ))
}

/// Builds `DROP DATABASE` for PostgreSQL.
pub fn drop_database_postgres(db: &Identifier) -> SqlStatement {
    let name = quote_identifier(Dialect::Postgres, db);
    SqlStatement::new(format!("DROP DATABASE {name}"))
}

/// Builds `DROP DATABASE` for MySQL/MariaDB.
pub fn drop_database_mysql(db: &Identifier) -> SqlStatement {
    let name = quote_identifier(Dialect::MySql, db);
    SqlStatement::new(format!("DROP DATABASE {name}"))
}

/// Query listing `(datname, pg_get_userbyid(datdba), pg_database_size)` for
/// non-template databases, bound to no parameters.
pub fn list_databases_postgres() -> SqlStatement {
    SqlStatement::new(
        "SELECT d.datname::text, pg_get_userbyid(d.datdba)::text, \
                pg_database_size(d.datname)::bigint \
         FROM pg_database d \
         WHERE d.datistemplate = false AND d.datallowconn = true \
         ORDER BY d.datname",
    )
}

/// Query counting live connections to a database, with the name bound as `$1`.
///
/// This is the value the engine accepts as a parameter, so it is bound rather
/// than interpolated even though it is also validated.
pub fn database_connection_count_postgres(db: &Identifier) -> SqlStatement {
    SqlStatement::with_params(
        "SELECT count(*)::bigint FROM pg_stat_activity WHERE datname = $1",
        vec![SqlParam::Text(db.as_str().to_owned())],
    )
}

/// Query listing login-capable roles, bound to no parameters.
pub fn list_users_postgres() -> SqlStatement {
    SqlStatement::new(
        "SELECT rolname::text, rolcanlogin, rolsuper FROM pg_roles \
         WHERE rolcanlogin ORDER BY rolname",
    )
}

/// Query checking whether a role exists, with the name bound as `$1`.
pub fn role_exists_postgres(user: &Identifier) -> SqlStatement {
    SqlStatement::with_params(
        "SELECT count(*)::bigint FROM pg_roles WHERE rolname = $1",
        vec![SqlParam::Text(user.as_str().to_owned())],
    )
}

/// Builds a role/user creation statement.
///
/// PostgreSQL: `CREATE ROLE "name" LOGIN PASSWORD E'...'`.
/// MySQL: ``CREATE USER `name`@'127.0.0.1' IDENTIFIED BY '...'``.
pub fn create_user(dialect: Dialect, user: &Identifier, password: &str) -> SqlStatement {
    match dialect {
        Dialect::Postgres => {
            let name = quote_identifier(Dialect::Postgres, user);
            let pw = quote_postgres_literal(password);
            SqlStatement::new(format!("CREATE ROLE {name} LOGIN PASSWORD {pw}"))
        }
        Dialect::MySql => {
            let name = quote_identifier(Dialect::MySql, user);
            let pw = quote_mysql_literal(password);
            SqlStatement::new(format!("CREATE USER {name}@'127.0.0.1' IDENTIFIED BY {pw}"))
        }
    }
}

/// Builds the statements that grant privileges for `user` on `db`.
pub fn grant_privileges(
    dialect: Dialect,
    user: &Identifier,
    db: &Identifier,
    level: PrivilegeLevel,
) -> Vec<SqlStatement> {
    match dialect {
        Dialect::Postgres => grant_postgres(user, db, level),
        Dialect::MySql => grant_mysql(user, db, level),
    }
}

/// The exact PostgreSQL grants per level, documented in
/// `docs/architecture/admin.md`.
fn grant_postgres(user: &Identifier, db: &Identifier, level: PrivilegeLevel) -> Vec<SqlStatement> {
    let user_q = quote_identifier(Dialect::Postgres, user);
    let db_q = quote_identifier(Dialect::Postgres, db);
    let mut stmts = vec![
        SqlStatement::new(format!("GRANT CONNECT ON DATABASE {db_q} TO {user_q}")),
        SqlStatement::new(format!("GRANT USAGE ON SCHEMA public TO {user_q}")),
        SqlStatement::new(format!(
            "GRANT SELECT ON ALL TABLES IN SCHEMA public TO {user_q}"
        )),
        SqlStatement::new(format!(
            "GRANT SELECT ON ALL SEQUENCES IN SCHEMA public TO {user_q}"
        )),
        SqlStatement::new(format!(
            "ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT ON TABLES TO {user_q}"
        )),
        SqlStatement::new(format!(
            "ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT ON SEQUENCES TO {user_q}"
        )),
    ];
    if level == PrivilegeLevel::ReadWrite {
        stmts.push(SqlStatement::new(format!(
            "GRANT INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO {user_q}"
        )));
        stmts.push(SqlStatement::new(format!(
            "GRANT USAGE, UPDATE ON ALL SEQUENCES IN SCHEMA public TO {user_q}"
        )));
        stmts.push(SqlStatement::new(format!(
            "ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT INSERT, UPDATE, DELETE ON TABLES TO {user_q}"
        )));
        stmts.push(SqlStatement::new(format!(
            "ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT USAGE, UPDATE ON SEQUENCES TO {user_q}"
        )));
    }
    stmts
}

fn grant_mysql(user: &Identifier, db: &Identifier, level: PrivilegeLevel) -> Vec<SqlStatement> {
    let user_q = quote_identifier(Dialect::MySql, user);
    let db_q = quote_identifier(Dialect::MySql, db);
    let privs = match level {
        PrivilegeLevel::ReadOnly => "SELECT",
        PrivilegeLevel::ReadWrite => "SELECT, INSERT, UPDATE, DELETE",
    };
    vec![SqlStatement::new(format!(
        "GRANT {privs} ON {db_q}.* TO {user_q}@'127.0.0.1'"
    ))]
}

/// Builds the statements that revoke every managed privilege for `user` on `db`.
pub fn revoke_privileges(
    dialect: Dialect,
    user: &Identifier,
    db: &Identifier,
) -> Vec<SqlStatement> {
    match dialect {
        Dialect::Postgres => {
            let user_q = quote_identifier(Dialect::Postgres, user);
            let db_q = quote_identifier(Dialect::Postgres, db);
            vec![
                SqlStatement::new(format!(
                    "REVOKE ALL PRIVILEGES ON DATABASE {db_q} FROM {user_q}"
                )),
                SqlStatement::new(format!(
                    "REVOKE ALL PRIVILEGES ON SCHEMA public FROM {user_q}"
                )),
                SqlStatement::new(format!(
                    "REVOKE ALL PRIVILEGES ON ALL TABLES IN SCHEMA public FROM {user_q}"
                )),
                SqlStatement::new(format!(
                    "REVOKE ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA public FROM {user_q}"
                )),
                SqlStatement::new(format!(
                    "ALTER DEFAULT PRIVILEGES IN SCHEMA public REVOKE ALL ON TABLES FROM {user_q}"
                )),
                SqlStatement::new(format!(
                    "ALTER DEFAULT PRIVILEGES IN SCHEMA public REVOKE ALL ON SEQUENCES FROM {user_q}"
                )),
            ]
        }
        Dialect::MySql => {
            let user_q = quote_identifier(Dialect::MySql, user);
            let db_q = quote_identifier(Dialect::MySql, db);
            vec![SqlStatement::new(format!(
                "REVOKE ALL PRIVILEGES ON {db_q}.* FROM {user_q}@'127.0.0.1'"
            ))]
        }
    }
}

/// Builds an `ALTER ROLE/USER ... PASSWORD` statement.
pub fn alter_password(dialect: Dialect, user: &Identifier, password: &str) -> SqlStatement {
    match dialect {
        Dialect::Postgres => {
            let user_q = quote_identifier(Dialect::Postgres, user);
            let pw = quote_postgres_literal(password);
            SqlStatement::new(format!("ALTER ROLE {user_q} PASSWORD {pw}"))
        }
        Dialect::MySql => {
            let user_q = quote_identifier(Dialect::MySql, user);
            let pw = quote_mysql_literal(password);
            SqlStatement::new(format!(
                "ALTER USER {user_q}@'127.0.0.1' IDENTIFIED BY {pw}"
            ))
        }
    }
}

/// Builds `DROP ROLE` for PostgreSQL.
pub fn drop_user_postgres(user: &Identifier) -> SqlStatement {
    let name = quote_identifier(Dialect::Postgres, user);
    SqlStatement::new(format!("DROP ROLE {name}"))
}

#[cfg(test)]
#[path = "sql/tests.rs"]
mod tests;
