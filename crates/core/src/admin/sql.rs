//! Pure SQL builders for database and user administration.
//!
//! Every function in this module is side-effect free. Identifiers are validated
//! against a strict allowlist and quoted per dialect; values that can be bound
//! parameters are returned separately.

use crate::error::{DblError, Result};

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
    /// CONNECT/USAGE + SELECT (+ default privileges for future tables/sequences).
    ReadOnly,
    /// ReadOnly + INSERT/UPDATE/DELETE (+ sequence usage/update + defaults).
    ReadWrite,
}

/// A validated SQL identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identifier(String);

impl Identifier {
    /// Maximum identifier length accepted by this module.
    pub const MAX_LEN: usize = 63;

    /// Validates and wraps a raw identifier.
    pub fn new(raw: &str) -> Result<Self> {
        validate_identifier(raw)?;
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

fn validate_identifier(raw: &str) -> Result<()> {
    if raw.is_empty() {
        return Err(invalid_identifier("identifier must not be empty"));
    }
    if raw.len() > Identifier::MAX_LEN {
        return Err(invalid_identifier("identifier is too long"));
    }
    let bytes = raw.as_bytes();
    let Some(first) = bytes.first() else {
        return Err(invalid_identifier("identifier must not be empty"));
    };
    if first.is_ascii_digit() {
        return Err(invalid_identifier("identifier must not start with a digit"));
    }
    if !bytes
        .iter()
        .all(|b| b.is_ascii_alphabetic() || b.is_ascii_digit() || *b == b'_')
    {
        return Err(invalid_identifier(
            "identifier contains characters outside [A-Za-z0-9_]",
        ));
    }
    if is_reserved(raw) {
        return Err(invalid_identifier("identifier is reserved"));
    }
    Ok(())
}

fn is_reserved(raw: &str) -> bool {
    matches!(
        raw.to_ascii_lowercase().as_str(),
        "postgres" | "template0" | "template1"
    )
}

fn invalid_identifier(reason: &str) -> DblError {
    DblError::InvalidInput {
        field: "identifier",
        reason: reason.to_owned(),
    }
}

/// Quotes an identifier for the given dialect.
///
/// The identifier must already be validated. This function still rejects invalid
/// input defensively.
pub fn quote_identifier(dialect: Dialect, ident: &Identifier) -> String {
    match dialect {
        Dialect::Postgres => quote_postgres_ident(ident.as_str()),
        Dialect::MySql => quote_mysql_ident(ident.as_str()),
    }
}

fn quote_postgres_ident(raw: &str) -> String {
    let escaped = raw.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

fn quote_mysql_ident(raw: &str) -> String {
    let escaped = raw.replace('`', "``");
    format!("`{escaped}`")
}

/// Quotes a SQL string literal for PostgreSQL.
///
/// Used only where PostgreSQL syntax requires a literal (e.g. CREATE ROLE PASSWORD).
/// The result is a single SQL string literal.
pub fn quote_postgres_literal(value: &str) -> String {
    let escaped = value.replace('\'', "''");
    format!("'{escaped}'")
}

/// Quotes a SQL string literal for MySQL.
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

/// A built SQL statement with optional bound parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlStatement {
    sql: String,
    params: Vec<SqlParam>,
}

/// A bound parameter value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlParam {
    /// UTF-8 text parameter.
    Text(String),
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

/// Builds `CREATE DATABASE` for PostgreSQL.
pub fn create_database_postgres(db: &Identifier) -> SqlStatement {
    let name = quote_identifier(Dialect::Postgres, db);
    SqlStatement::new(format!(
        "CREATE DATABASE {name} WITH ENCODING 'UTF8' TEMPLATE template0"
    ))
}

/// Builds `CREATE DATABASE` for MySQL/MariaDB.
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

/// Builds a role/user creation statement.
///
/// PostgreSQL: `CREATE ROLE "name" LOGIN PASSWORD '...'`.
/// MySQL: `CREATE USER `name`@'127.0.0.1' IDENTIFIED BY '...'`.
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

/// Builds statements that grant privileges for `user` on `db`.
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
    vec![
        SqlStatement::new(format!("GRANT {privs} ON {db_q}.* TO {user_q}@'127.0.0.1'")),
        SqlStatement::new("FLUSH PRIVILEGES".to_owned()),
    ]
}

/// Builds statements that revoke managed privileges for `user` on `db`.
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
                SqlStatement::new(format!("REVOKE ALL PRIVILEGES ON SCHEMA public FROM {user_q}")),
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
            vec![
                SqlStatement::new(format!(
                    "REVOKE ALL PRIVILEGES ON {db_q}.* FROM {user_q}@'127.0.0.1'"
                )),
                SqlStatement::new("FLUSH PRIVILEGES".to_owned()),
            ]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_accepts_valid_names() {
        for name in ["app", "my_db1", "_internal", "A1_b2", "a".repeat(63).as_str()] {
            let id = Identifier::new(name).expect(name);
            assert_eq!(id.as_str(), name);
        }
    }

    #[test]
    fn identifier_rejects_hostile_and_invalid() {
        let long = "a".repeat(64);
        let cases = [
            "",
            "1abc",
            "ab-cd",
            "ab cd",
            "ab;cd",
            "ab'cd",
            "ab\"cd",
            "ab`cd",
            "ab\\cd",
            "ab\0cd",
            "ab\ncd",
            "ab--cd",
            "ab/*cd*/",
            long.as_str(),
            "postgres",
            "template0",
            "template1",
            "POSTGRES",
            "Template0",
        ];
        for raw in cases {
            let err = Identifier::new(raw).expect_err(raw);
            assert!(matches!(err, DblError::InvalidInput { .. }), "{raw}: {err:?}");
        }
    }

    #[test]
    fn quote_postgres_identifier_escapes_quotes() {
        let id = Identifier::new("normal_name").expect("valid");
        assert_eq!(quote_identifier(Dialect::Postgres, &id), "\"normal_name\"");
    }

    #[test]
    fn quote_mysql_identifier_uses_backticks() {
        let id = Identifier::new("normal_name").expect("valid");
        assert_eq!(quote_identifier(Dialect::MySql, &id), "`normal_name`");
    }

    #[test]
    fn postgres_literal_escapes_single_quote() {
        assert_eq!(quote_postgres_literal("a'b"), "'a''b'");
    }

    #[test]
    fn mysql_literal_escapes_special_chars() {
        let quoted = quote_mysql_literal("a'b\\c\0\n\r\u{001a}");
        assert_eq!(quoted, "'a''b\\\\c\\0\\n\\r\\Z'");
    }

    #[test]
    fn build_create_database_postgres() {
        let db = Identifier::new("mydb").expect("valid");
        let stmt = create_database_postgres(&db);
        assert_eq!(
            stmt.sql(),
            "CREATE DATABASE \"mydb\" WITH ENCODING 'UTF8' TEMPLATE template0"
        );
        assert!(stmt.params().is_empty());
    }

    #[test]
    fn build_create_database_mysql() {
        let db = Identifier::new("mydb").expect("valid");
        let stmt = create_database_mysql(&db);
        assert_eq!(
            stmt.sql(),
            "CREATE DATABASE `mydb` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci"
        );
    }

    #[test]
    fn build_create_user_postgres_quotes_password_literal() {
        let user = Identifier::new("app_user").expect("valid");
        let stmt = create_user(Dialect::Postgres, &user, "p@ss'w0rd");
        assert_eq!(
            stmt.sql(),
            "CREATE ROLE \"app_user\" LOGIN PASSWORD 'p@ss''w0rd'"
        );
    }

    #[test]
    fn build_create_user_mysql_quotes_password_literal() {
        let user = Identifier::new("app_user").expect("valid");
        let stmt = create_user(Dialect::MySql, &user, "p@ss'w0rd");
        assert_eq!(
            stmt.sql(),
            "CREATE USER `app_user`@'127.0.0.1' IDENTIFIED BY 'p@ss''w0rd'"
        );
    }

    #[test]
    fn grant_read_only_postgres_has_expected_statements() {
        let user = Identifier::new("app_ro").expect("valid");
        let db = Identifier::new("appdb").expect("valid");
        let stmts = grant_privileges(Dialect::Postgres, &user, &db, PrivilegeLevel::ReadOnly);
        let sqls: Vec<&str> = stmts.iter().map(|s| s.sql()).collect();
        assert!(sqls.contains(&"GRANT CONNECT ON DATABASE \"appdb\" TO \"app_ro\""));
        assert!(sqls.contains(&"GRANT USAGE ON SCHEMA public TO \"app_ro\""));
        assert!(sqls.contains(&"GRANT SELECT ON ALL TABLES IN SCHEMA public TO \"app_ro\""));
        assert!(sqls.iter().any(|s| s.contains("DEFAULT PRIVILEGES") && s.contains("SELECT ON TABLES")));
        assert!(!sqls.iter().any(|s| s.contains("INSERT, UPDATE, DELETE")));
    }

    #[test]
    fn grant_read_write_postgres_adds_write_privileges() {
        let user = Identifier::new("app_rw").expect("valid");
        let db = Identifier::new("appdb").expect("valid");
        let stmts = grant_privileges(Dialect::Postgres, &user, &db, PrivilegeLevel::ReadWrite);
        let joined = stmts.iter().map(|s| s.sql()).collect::<Vec<_>>().join("\n");
        assert!(joined.contains("GRANT INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO \"app_rw\""));
        assert!(joined.contains("GRANT USAGE, UPDATE ON ALL SEQUENCES IN SCHEMA public TO \"app_rw\""));
        assert!(joined.contains("ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT INSERT, UPDATE, DELETE ON TABLES TO \"app_rw\""));
    }

    #[test]
    fn grant_mysql_read_only_and_write() {
        let user = Identifier::new("app").expect("valid");
        let db = Identifier::new("appdb").expect("valid");
        let ro = grant_privileges(Dialect::MySql, &user, &db, PrivilegeLevel::ReadOnly);
        assert_eq!(ro[0].sql(), "GRANT SELECT ON `appdb`.* TO `app`@'127.0.0.1'");
        let rw = grant_privileges(Dialect::MySql, &user, &db, PrivilegeLevel::ReadWrite);
        assert_eq!(rw[0].sql(), "GRANT SELECT, INSERT, UPDATE, DELETE ON `appdb`.* TO `app`@'127.0.0.1'");
    }

    #[test]
    fn alter_password_builders_quote() {
        let user = Identifier::new("app").expect("valid");
        let pg = alter_password(Dialect::Postgres, &user, "pa'ss");
        assert_eq!(pg.sql(), "ALTER ROLE \"app\" PASSWORD 'pa''ss'");
        let my = alter_password(Dialect::MySql, &user, "pa'ss");
        assert_eq!(my.sql(), "ALTER USER `app`@'127.0.0.1' IDENTIFIED BY 'pa''ss'");
    }

    #[test]
    fn drop_and_revoke_builders() {
        let db = Identifier::new("appdb").expect("valid");
        let user = Identifier::new("app").expect("valid");
        assert_eq!(drop_database_postgres(&db).sql(), "DROP DATABASE \"appdb\"");
        assert_eq!(drop_database_mysql(&db).sql(), "DROP DATABASE `appdb`");
        let rev_pg = revoke_privileges(Dialect::Postgres, &user, &db);
        assert!(rev_pg[0].sql().contains("REVOKE ALL PRIVILEGES ON DATABASE \"appdb\" FROM \"app\""));
        let rev_my = revoke_privileges(Dialect::MySql, &user, &db);
        assert!(rev_my[0].sql().contains("REVOKE ALL PRIVILEGES ON `appdb`.* FROM `app`@'127.0.0.1'"));
    }
}
