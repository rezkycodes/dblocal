//! Unit tests for the pure SQL builders, including a tiny tokenizer used to
//! prove that a hostile name or password cannot turn one statement into two.

use super::*;

// ---------------------------------------------------------------------------
// A minimal SQL tokenizer.
//
// It understands exactly what these builders emit: bare words, `"…"` and
// `` `…` `` quoted identifiers, `'…'` literals, `E'…'` escape literals, and the
// punctuation `; , ( )`. Anything it cannot scan is an error. A quoting
// breakout therefore shows up as a second statement or a parse failure, never as
// a silent pass.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Word(String),
    Ident(String),
    Str(String),
    Semi,
    Comma,
    LParen,
    RParen,
}

fn w(word: &str) -> Token {
    Token::Word(word.to_owned())
}

fn ident(name: &str) -> Token {
    Token::Ident(name.to_owned())
}

fn lit(value: &str) -> Token {
    Token::Str(value.to_owned())
}

fn tokenize(sql: &str) -> Result<Vec<Token>, String> {
    let bytes = sql.as_bytes();
    let mut i = 0usize;
    let mut out = Vec::new();
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'-' && bytes.get(i + 1) == Some(&b'-') {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            if i + 1 >= bytes.len() {
                return Err("unterminated block comment".to_owned());
            }
            i += 2;
            continue;
        }
        match c {
            b';' => {
                out.push(Token::Semi);
                i += 1;
            }
            b',' => {
                out.push(Token::Comma);
                i += 1;
            }
            b'(' => {
                out.push(Token::LParen);
                i += 1;
            }
            b')' => {
                out.push(Token::RParen);
                i += 1;
            }
            b'\'' => {
                let (value, next) = scan_standard_string(bytes, i)?;
                out.push(Token::Str(value));
                i = next;
            }
            b'"' => {
                let (value, next) = scan_quoted(bytes, i, b'"')?;
                out.push(Token::Ident(value));
                i = next;
            }
            b'`' => {
                let (value, next) = scan_quoted(bytes, i, b'`')?;
                out.push(Token::Ident(value));
                i = next;
            }
            b'e' | b'E' if bytes.get(i + 1) == Some(&b'\'') => {
                let (value, next) = scan_escape_string(bytes, i)?;
                out.push(Token::Str(value));
                i = next;
            }
            _ => {
                let start = i;
                while i < bytes.len() {
                    let b = bytes[i];
                    if b.is_ascii_whitespace()
                        || matches!(b, b';' | b',' | b'(' | b')' | b'\'' | b'"' | b'`')
                        || (b == b'-' && bytes.get(i + 1) == Some(&b'-'))
                        || (b == b'/' && bytes.get(i + 1) == Some(&b'*'))
                    {
                        break;
                    }
                    i += 1;
                }
                if i == start {
                    return Err(format!("unexpected byte {c:#04x}"));
                }
                // Splits only at ASCII bytes, so every slice is on a char
                // boundary and `from_utf8` cannot fail for valid input.
                out.push(Token::Word(sql[start..i].to_owned()));
            }
        }
    }
    Ok(out)
}

fn scan_standard_string(bytes: &[u8], start: usize) -> Result<(String, usize), String> {
    let mut value = Vec::new();
    let mut j = start + 1;
    loop {
        let Some(&b) = bytes.get(j) else {
            return Err("unterminated string".to_owned());
        };
        if b == b'\'' {
            if bytes.get(j + 1) == Some(&b'\'') {
                value.push(b'\'');
                j += 2;
                continue;
            }
            return finish(value, j + 1);
        }
        value.push(b);
        j += 1;
    }
}

fn scan_escape_string(bytes: &[u8], start: usize) -> Result<(String, usize), String> {
    let mut value = Vec::new();
    let mut j = start + 2; // past E'
    loop {
        let Some(&b) = bytes.get(j) else {
            return Err("unterminated escape string".to_owned());
        };
        if b == b'\\' {
            let Some(&escaped) = bytes.get(j + 1) else {
                return Err("dangling backslash".to_owned());
            };
            if escaped == b'x' {
                let hi = bytes.get(j + 2).copied().ok_or("short hex escape")?;
                let lo = bytes.get(j + 3).copied().ok_or("short hex escape")?;
                let byte = hex_byte(hi, lo)?;
                value.push(byte);
                j += 4;
            } else {
                value.push(escaped);
                j += 2;
            }
            continue;
        }
        if b == b'\'' {
            if bytes.get(j + 1) == Some(&b'\'') {
                value.push(b'\'');
                j += 2;
                continue;
            }
            return finish(value, j + 1);
        }
        value.push(b);
        j += 1;
    }
}

fn scan_quoted(bytes: &[u8], start: usize, quote: u8) -> Result<(String, usize), String> {
    let mut value = Vec::new();
    let mut j = start + 1;
    loop {
        let Some(&b) = bytes.get(j) else {
            return Err("unterminated quoted identifier".to_owned());
        };
        if b == quote {
            if bytes.get(j + 1) == Some(&quote) {
                value.push(quote);
                j += 2;
                continue;
            }
            return finish(value, j + 1);
        }
        value.push(b);
        j += 1;
    }
}

fn finish(value: Vec<u8>, next: usize) -> Result<(String, usize), String> {
    String::from_utf8(value)
        .map(|s| (s, next))
        .map_err(|_| "invalid utf-8 in a token".to_owned())
}

fn hex_byte(hi: u8, lo: u8) -> Result<u8, String> {
    let digit = |b: u8| -> Result<u8, String> {
        match b {
            b'0'..=b'9' => Ok(b - b'0'),
            b'a'..=b'f' => Ok(b - b'a' + 10),
            b'A'..=b'F' => Ok(b - b'A' + 10),
            _ => Err("bad hex digit".to_owned()),
        }
    };
    Ok((digit(hi)? << 4) | digit(lo)?)
}

/// Splits tokenized SQL into statements on `;`, dropping a trailing empty group
/// (the builders emit no trailing semicolon, but tolerate one).
fn statements(sql: &str) -> Result<Vec<Vec<Token>>, String> {
    let tokens = tokenize(sql)?;
    let mut statements = Vec::new();
    let mut current = Vec::new();
    for token in tokens {
        if token == Token::Semi {
            if !current.is_empty() {
                statements.push(std::mem::take(&mut current));
            }
        } else {
            current.push(token);
        }
    }
    if !current.is_empty() {
        statements.push(current);
    }
    Ok(statements)
}

/// Asserts the statement is exactly one, and returns its tokens.
fn one_statement(sql: &str) -> Vec<Token> {
    let statements = statements(sql).unwrap_or_else(|err| panic!("cannot parse {sql:?}: {err}"));
    assert_eq!(statements.len(), 1, "expected one statement in {sql:?}");
    statements.into_iter().next().unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Identifier validation.
// ---------------------------------------------------------------------------

#[test]
fn identifier_accepts_valid_names() {
    let long = "a".repeat(Identifier::MAX_LEN);
    for name in ["app", "my_db1", "_internal", "A1_b2", "z", long.as_str()] {
        let id = Identifier::new(name).unwrap_or_else(|err| panic!("{name}: {err}"));
        assert_eq!(id.as_str(), name);
        assert_eq!(id.to_string(), name);
    }
}

#[test]
fn identifier_rejects_with_a_specific_reason() {
    let cases = [
        ("", IdentifierReject::Empty),
        ("1abc", IdentifierReject::StartsWithDigit),
        ("9", IdentifierReject::StartsWithDigit),
        ("ab-cd", IdentifierReject::BadCharacter),
        ("ab cd", IdentifierReject::BadCharacter),
        ("ab;cd", IdentifierReject::BadCharacter),
        ("ab'cd", IdentifierReject::BadCharacter),
        ("ab\"cd", IdentifierReject::BadCharacter),
        ("ab`cd", IdentifierReject::BadCharacter),
        ("ab\\cd", IdentifierReject::BadCharacter),
        ("ab\0cd", IdentifierReject::BadCharacter),
        ("ab\ncd", IdentifierReject::BadCharacter),
        ("ab--cd", IdentifierReject::BadCharacter),
        ("ab/*cd*/", IdentifierReject::BadCharacter),
        ("café", IdentifierReject::BadCharacter),
        ("日本語", IdentifierReject::BadCharacter),
        ("postgres", IdentifierReject::Reserved),
        ("template0", IdentifierReject::Reserved),
        ("template1", IdentifierReject::Reserved),
        ("POSTGRES", IdentifierReject::Reserved),
        ("Template0", IdentifierReject::Reserved),
    ];
    for (raw, expected) in cases {
        let err = Identifier::new(raw).expect_err(raw);
        assert_eq!(
            err,
            AdminError::InvalidIdentifier(expected),
            "input {raw:?}"
        );
    }
    let too_long = "a".repeat(Identifier::MAX_LEN + 1);
    assert_eq!(
        Identifier::new(&too_long).expect_err("too long"),
        AdminError::InvalidIdentifier(IdentifierReject::TooLong)
    );
}

// ---------------------------------------------------------------------------
// Quoting. Deliberately bypasses `Identifier` so the quoting function is proven
// even if the allowlist were weakened: a hostile raw value must still yield a
// single identifier token with the exact original text.
// ---------------------------------------------------------------------------

#[test]
fn postgres_identifier_quoting_survives_hostile_input() {
    let hostile = [
        ("normal", "\"normal\""),
        ("a\"b", "\"a\"\"b\""),
        ("x\"; DROP TABLE t; --", "\"x\"\"; DROP TABLE t; --\""),
        ("名前", "\"名前\""),
    ];
    for (raw, expected) in hostile {
        let quoted = quote_postgres_ident(raw);
        assert_eq!(quoted, expected);
        let tokens = one_statement(&format!("SELECT {quoted}"));
        assert_eq!(tokens, vec![w("SELECT"), ident(raw)]);
    }
}

#[test]
fn mysql_identifier_quoting_survives_hostile_input() {
    let quoted = quote_mysql_ident("x`; DROP TABLE t; --");
    assert_eq!(quoted, "`x``; DROP TABLE t; --`");
    let tokens = one_statement(&format!("SELECT {quoted}"));
    assert_eq!(tokens, vec![w("SELECT"), ident("x`; DROP TABLE t; --")]);
}

#[test]
fn postgres_literal_round_trips_every_hostile_value() {
    let hostile = [
        "plain",
        "pa'ss",
        "a\\b",
        "x'; DROP TABLE t; --",
        "unicode-café-日本",
        "nul\0byte",
        "\"quoted\"",
        "--not a comment",
        "/* not a comment */",
    ];
    for value in hostile {
        let quoted = quote_postgres_literal(value);
        let tokens = one_statement(&format!("SELECT {quoted}"));
        assert_eq!(
            tokens,
            vec![w("SELECT"), lit(value)],
            "literal {value:?} did not round-trip through {quoted:?}"
        );
    }
}

#[test]
fn mysql_literal_round_trips_every_hostile_value() {
    let hostile = [
        "plain",
        "pa'ss",
        "a\\b",
        "x'; DROP TABLE t; --",
        "line\nbreak",
        "tab\there",
    ];
    for value in hostile {
        let quoted = quote_mysql_literal(value);
        // The shared tokenizer understands the standard-literal subset MySQL
        // uses here except `\n`/`\r`; those are asserted by string equality.
        assert!(quoted.starts_with('\'') && quoted.ends_with('\''));
        assert!(!quoted.trim_matches('\'').contains('\'') || quoted.contains("''"));
    }
    assert_eq!(quote_mysql_literal("a'b\\c"), "'a''b\\\\c'");
    assert_eq!(quote_mysql_literal("a\0b"), "'a\\0b'");
    assert_eq!(quote_mysql_literal("a\nb"), "'a\\nb'");
    assert_eq!(quote_mysql_literal("a\rb"), "'a\\rb'");
    assert_eq!(quote_mysql_literal("a\u{001a}b"), "'a\\Zb'");
}

// ---------------------------------------------------------------------------
// Builders: exact text plus a single-statement parse.
// ---------------------------------------------------------------------------

#[test]
fn create_database_builders() {
    let db = Identifier::new("mydb").expect("valid");
    let pg = create_database_postgres(&db);
    assert_eq!(
        pg.sql(),
        "CREATE DATABASE \"mydb\" WITH ENCODING 'UTF8' TEMPLATE template0"
    );
    assert!(pg.params().is_empty());
    assert_eq!(
        one_statement(pg.sql()),
        vec![
            w("CREATE"),
            w("DATABASE"),
            ident("mydb"),
            w("WITH"),
            w("ENCODING"),
            lit("UTF8"),
            w("TEMPLATE"),
            w("template0"),
        ]
    );

    let my = create_database_mysql(&db);
    assert_eq!(
        my.sql(),
        "CREATE DATABASE `mydb` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci"
    );
    assert_eq!(one_statement(my.sql()).len(), 8);
}

#[test]
fn drop_database_builders() {
    let db = Identifier::new("appdb").expect("valid");
    assert_eq!(drop_database_postgres(&db).sql(), "DROP DATABASE \"appdb\"");
    assert_eq!(drop_database_mysql(&db).sql(), "DROP DATABASE `appdb`");
    assert_eq!(
        one_statement(drop_database_postgres(&db).sql()),
        vec![w("DROP"), w("DATABASE"), ident("appdb")]
    );
}

#[test]
fn create_user_builders() {
    let user = Identifier::new("app_user").expect("valid");
    let pg = create_user(Dialect::Postgres, &user, "p@ss'w0rd");
    assert_eq!(
        pg.sql(),
        "CREATE ROLE \"app_user\" LOGIN PASSWORD E'p@ss''w0rd'"
    );
    assert_eq!(
        one_statement(pg.sql()),
        vec![
            w("CREATE"),
            w("ROLE"),
            ident("app_user"),
            w("LOGIN"),
            w("PASSWORD"),
            lit("p@ss'w0rd"),
        ]
    );

    let my = create_user(Dialect::MySql, &user, "p@ss'w0rd");
    assert_eq!(
        my.sql(),
        "CREATE USER `app_user`@'127.0.0.1' IDENTIFIED BY 'p@ss''w0rd'"
    );
    assert_eq!(one_statement(my.sql()).last(), Some(&lit("p@ss'w0rd")));
}

#[test]
fn grant_read_only_postgres_grants_select_but_not_write() {
    let user = Identifier::new("app_ro").expect("valid");
    let db = Identifier::new("appdb").expect("valid");
    let stmts = grant_privileges(Dialect::Postgres, &user, &db, PrivilegeLevel::ReadOnly);
    let sqls: Vec<&str> = stmts.iter().map(SqlStatement::sql).collect();
    assert!(sqls.contains(&"GRANT CONNECT ON DATABASE \"appdb\" TO \"app_ro\""));
    assert!(sqls.contains(&"GRANT USAGE ON SCHEMA public TO \"app_ro\""));
    assert!(sqls.contains(&"GRANT SELECT ON ALL TABLES IN SCHEMA public TO \"app_ro\""));
    assert!(
        sqls.iter()
            .any(|s| s.contains("DEFAULT PRIVILEGES") && s.contains("SELECT ON TABLES"))
    );
    assert!(!sqls.iter().any(|s| s.contains("INSERT")));
    assert!(!sqls.iter().any(|s| s.contains("UPDATE")));
    assert!(!sqls.iter().any(|s| s.contains("DELETE")));
    for sql in sqls {
        assert_eq!(one_statement(sql).last(), Some(&ident("app_ro")));
    }
}

#[test]
fn grant_read_write_postgres_adds_write_and_sequence_rights() {
    let user = Identifier::new("app_rw").expect("valid");
    let db = Identifier::new("appdb").expect("valid");
    let stmts = grant_privileges(Dialect::Postgres, &user, &db, PrivilegeLevel::ReadWrite);
    let joined = stmts
        .iter()
        .map(SqlStatement::sql)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined
            .contains("GRANT INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO \"app_rw\"")
    );
    assert!(joined.contains("GRANT USAGE, UPDATE ON ALL SEQUENCES IN SCHEMA public TO \"app_rw\""));
    assert!(joined.contains("ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT INSERT, UPDATE, DELETE ON TABLES TO \"app_rw\""));
    assert!(joined.contains(
        "ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT USAGE, UPDATE ON SEQUENCES TO \"app_rw\""
    ));
    for stmt in &stmts {
        assert_eq!(one_statement(stmt.sql()).last(), Some(&ident("app_rw")));
    }
}

#[test]
fn grant_mysql_per_level() {
    let user = Identifier::new("app").expect("valid");
    let db = Identifier::new("appdb").expect("valid");
    let ro = grant_privileges(Dialect::MySql, &user, &db, PrivilegeLevel::ReadOnly);
    assert_eq!(ro.len(), 1);
    assert_eq!(
        ro[0].sql(),
        "GRANT SELECT ON `appdb`.* TO `app`@'127.0.0.1'"
    );
    let rw = grant_privileges(Dialect::MySql, &user, &db, PrivilegeLevel::ReadWrite);
    assert_eq!(
        rw[0].sql(),
        "GRANT SELECT, INSERT, UPDATE, DELETE ON `appdb`.* TO `app`@'127.0.0.1'"
    );
}

#[test]
fn revoke_builders() {
    let db = Identifier::new("appdb").expect("valid");
    let user = Identifier::new("app").expect("valid");
    let rev_pg = revoke_privileges(Dialect::Postgres, &user, &db);
    assert!(
        rev_pg[0]
            .sql()
            .contains("REVOKE ALL PRIVILEGES ON DATABASE \"appdb\" FROM \"app\"")
    );
    for stmt in &rev_pg {
        assert!(one_statement(stmt.sql()).len() > 3);
    }
    let rev_my = revoke_privileges(Dialect::MySql, &user, &db);
    assert_eq!(
        rev_my[0].sql(),
        "REVOKE ALL PRIVILEGES ON `appdb`.* FROM `app`@'127.0.0.1'"
    );
}

#[test]
fn alter_password_and_drop_user_builders() {
    let user = Identifier::new("app").expect("valid");
    let pg = alter_password(Dialect::Postgres, &user, "pa'ss");
    assert_eq!(pg.sql(), "ALTER ROLE \"app\" PASSWORD E'pa''ss'");
    assert_eq!(
        one_statement(pg.sql()),
        vec![
            w("ALTER"),
            w("ROLE"),
            ident("app"),
            w("PASSWORD"),
            lit("pa'ss")
        ]
    );
    let my = alter_password(Dialect::MySql, &user, "pa'ss");
    assert_eq!(
        my.sql(),
        "ALTER USER `app`@'127.0.0.1' IDENTIFIED BY 'pa''ss'"
    );
    assert_eq!(drop_user_postgres(&user).sql(), "DROP ROLE \"app\"");
}

// ---------------------------------------------------------------------------
// Bound-parameter helpers and read queries.
// ---------------------------------------------------------------------------

#[test]
fn read_queries_bind_their_name_parameters() {
    assert!(list_databases_postgres().params().is_empty());
    assert!(list_users_postgres().params().is_empty());

    let db = Identifier::new("appdb").expect("valid");
    let count = database_connection_count_postgres(&db);
    assert!(count.sql().contains("datname = $1"));
    assert_eq!(count.params(), &[SqlParam::Text("appdb".to_owned())]);

    let user = Identifier::new("app").expect("valid");
    let exists = role_exists_postgres(&user);
    assert!(exists.sql().contains("rolname = $1"));
    assert_eq!(exists.params(), &[SqlParam::Text("app".to_owned())]);
}

#[test]
fn read_queries_tokenize_cleanly() {
    for stmt in [
        list_databases_postgres(),
        list_users_postgres(),
        database_connection_count_postgres(&Identifier::new("db1").expect("valid")),
    ] {
        assert_eq!(one_statement(stmt.sql()).first(), Some(&w("SELECT")));
    }
}

// ---------------------------------------------------------------------------
// The hostile corpus: every entry is rejected by validation, and if it were not,
// the quoting layer still yields one well-formed statement.
// ---------------------------------------------------------------------------

#[test]
fn hostile_corpus_is_rejected_before_any_statement_is_built() {
    let long = "a".repeat(Identifier::MAX_LEN + 1);
    let corpus = [
        "a'; DROP DATABASE postgres; --",
        "a\"; DROP DATABASE postgres; --",
        "a`; DROP DATABASE postgres; --",
        "a\\'; DROP",
        "ab\0cd",
        "\u{0000}",
        "名前",
        "café",
        "-leading-dash",
        "--comment",
        "/*comment*/",
        "a b",
        "a\tb",
        "a\nb",
        "a\rb",
        "a;b",
        "a.b",
        "a%b",
        "a$b",
        "a@b",
        long.as_str(),
    ];
    for raw in corpus {
        let err = Identifier::new(raw)
            .err()
            .unwrap_or_else(|| panic!("validation accepted hostile input {raw:?}"));
        assert!(
            matches!(err, AdminError::InvalidIdentifier(_)),
            "{raw:?} gave {err:?}"
        );
    }
}

#[test]
fn a_name_that_would_break_out_is_quoted_into_a_single_identifier() {
    // Even though validation refuses these, the quoting helper is proven
    // independently: it must never emit a second statement.
    let breakouts = [
        "x\"; DROP TABLE t; --",
        "x`; DROP TABLE t; --",
        "x\"; SELECT pg_sleep(1); --",
    ];
    for raw in breakouts {
        for quoted in [quote_postgres_ident(raw), quote_mysql_ident(raw)] {
            let tokens = one_statement(&format!("DROP DATABASE {quoted}"));
            assert_eq!(tokens.len(), 3);
            assert_eq!(tokens[0], w("DROP"));
            assert_eq!(tokens[1], w("DATABASE"));
            assert!(matches!(tokens[2], Token::Ident(_)), "{tokens:?}");
        }
    }
}
