//! Connection URI and `.env` rendering.
//!
//! Passwords are percent-encoded in the URI userinfo so hostile characters
//! never break parsing (W0-B). Returned values are secrets because a URI may
//! embed the password.

use std::fmt::Write as _;

use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

use crate::consts;
use crate::engine::Credential;
use crate::model::EngineKey;
use crate::secret::SecretString;
use crate::service::UriFormat;

/// Characters that must be percent-encoded inside a URI userinfo value.
///
/// RFC 3986 `userinfo` allows unreserved, sub-delims and `:`; we encode every
/// byte that is not unreserved (`ALPHA / DIGIT / "-" / "." / "_" / "~"`) so a
/// username or password can be embedded without breaking on delimiters such as
/// `@`, `:`, `/`, `?`, `#`, `[`, `]`, `%`, space or any non-ASCII byte.
const USERINFO: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'<')
    .add(b'=')
    .add(b'>')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}')
    .add(b'!')
    .add(b'$')
    .add(b'&')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b'+')
    .add(b',');

/// Percent-encodes a URI userinfo component (username or password).
pub fn encode_userinfo(value: &str) -> String {
    utf8_percent_encode(value, USERINFO).to_string()
}

/// Appends `.exe` on Windows so bundled binaries are addressable everywhere.
pub fn exe_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.to_owned()
    }
}

/// Builds the bare connection URI for an engine instance.
pub fn build_uri(engine: EngineKey, port: u16, credential: &Credential) -> SecretString {
    uri_with(
        engine,
        port,
        &credential.username,
        Some(&credential.password),
    )
}

/// Builds the connection URI for an instance whose auth mode is
/// [`crate::model::AuthMode::None`].
///
/// No password exists, so the userinfo carries only the username (PostgreSQL,
/// MySQL/MariaDB) or nothing at all (Valkey, which has no user without a
/// password). Emitting a bare `:` would be read as an empty password by many
/// clients and is exactly the shape this path must avoid.
pub fn build_uri_no_password(engine: EngineKey, port: u16, username: &str) -> SecretString {
    uri_with(engine, port, username, None)
}

fn uri_with(
    engine: EngineKey,
    port: u16,
    username: &str,
    password: Option<&SecretString>,
) -> SecretString {
    let user = encode_userinfo(username);
    match engine {
        EngineKey::Postgresql => {
            let authority = userinfo(&user, password);
            SecretString::new(format!(
                "postgresql://{authority}@127.0.0.1:{port}/{}",
                consts::DEFAULT_DATABASE
            ))
        }
        EngineKey::Valkey | EngineKey::Redis => match password {
            Some(password) => SecretString::new(format!(
                "redis://{}@127.0.0.1:{port}/0",
                userinfo(&user, Some(password))
            )),
            None => SecretString::new(format!("redis://127.0.0.1:{port}/0")),
        },
        EngineKey::Mysql | EngineKey::Mariadb => {
            let authority = userinfo(&user, password);
            SecretString::new(format!("mysql://{authority}@127.0.0.1:{port}/"))
        }
    }
}

/// `user` or `user:password`, both percent-encoded.
fn userinfo(user: &str, password: Option<&SecretString>) -> String {
    match password {
        Some(password) => format!("{user}:{}", encode_userinfo(password.expose())),
        None => user.to_owned(),
    }
}

/// Builds the `.env` block for an engine instance.
pub fn build_dotenv(engine: EngineKey, port: u16, credential: &Credential) -> SecretString {
    dotenv_with(
        engine,
        port,
        &credential.username,
        Some(&credential.password),
    )
}

/// Builds the `.env` block for an instance whose auth mode is
/// [`crate::model::AuthMode::None`].
///
/// The password key is omitted rather than written empty: a literal
/// `DB_PASSWORD=` is indistinguishable from a blank password and would invite a
/// client to attempt password authentication against a server that has none.
pub fn build_dotenv_no_password(engine: EngineKey, port: u16, username: &str) -> SecretString {
    dotenv_with(engine, port, username, None)
}

fn dotenv_with(
    engine: EngineKey,
    port: u16,
    username: &str,
    password: Option<&SecretString>,
) -> SecretString {
    let uri = uri_with(engine, port, username, password);
    let mut out = String::new();
    match engine {
        EngineKey::Postgresql => {
            let _ = writeln!(out, "DATABASE_URL={}", uri.expose());
            let _ = writeln!(out, "DB_HOST=127.0.0.1");
            let _ = writeln!(out, "DB_PORT={port}");
            let _ = writeln!(out, "DB_DATABASE={}", consts::DEFAULT_DATABASE);
            let _ = writeln!(out, "DB_USERNAME={username}");
            if let Some(password) = password {
                let _ = writeln!(out, "DB_PASSWORD={}", password.expose());
            }
        }
        EngineKey::Valkey | EngineKey::Redis => {
            let _ = writeln!(out, "REDIS_URL={}", uri.expose());
            let _ = writeln!(out, "REDIS_HOST=127.0.0.1");
            let _ = writeln!(out, "REDIS_PORT={port}");
            if let Some(password) = password {
                let _ = writeln!(out, "REDIS_PASSWORD={}", password.expose());
            }
        }
        EngineKey::Mysql | EngineKey::Mariadb => {
            let _ = writeln!(out, "DATABASE_URL={}", uri.expose());
            let _ = writeln!(out, "DB_HOST=127.0.0.1");
            let _ = writeln!(out, "DB_PORT={port}");
            let _ = writeln!(out, "DB_DATABASE=");
            let _ = writeln!(out, "DB_USERNAME={username}");
            if let Some(password) = password {
                let _ = writeln!(out, "DB_PASSWORD={}", password.expose());
            }
        }
    }
    SecretString::new(out)
}

/// Renders a connection value in the requested format.
pub fn render(
    engine: EngineKey,
    port: u16,
    credential: &Credential,
    format: UriFormat,
) -> SecretString {
    render_with(
        engine,
        port,
        &credential.username,
        Some(&credential.password),
        format,
    )
}

/// Renders a connection value for an instance with no password.
pub fn render_no_password(
    engine: EngineKey,
    port: u16,
    username: &str,
    format: UriFormat,
) -> SecretString {
    render_with(engine, port, username, None, format)
}

fn render_with(
    engine: EngineKey,
    port: u16,
    username: &str,
    password: Option<&SecretString>,
    format: UriFormat,
) -> SecretString {
    match format {
        UriFormat::Uri => uri_with(engine, port, username, password),
        UriFormat::DotEnv => dotenv_with(engine, port, username, password),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cred(user: &str, pass: &str) -> Credential {
        Credential::new(user, SecretString::from(pass))
    }

    #[test]
    fn encode_userinfo_handles_hostile_characters() {
        let raw = "p@ss:w/rd?#[]% x\u{00E9}";
        let enc = encode_userinfo(raw);
        assert!(!enc.contains('@'));
        assert!(!enc.contains(':'));
        assert!(!enc.contains('/'));
        assert!(!enc.contains('?'));
        assert!(!enc.contains('#'));
        assert!(!enc.contains('['));
        assert!(!enc.contains(']'));
        assert!(!enc.contains(' '));
        assert!(enc.contains("%40"));
        assert!(enc.contains("%3A"));
        assert!(enc.contains("%2F"));
        assert!(enc.contains("%3F"));
        assert!(enc.contains("%23"));
        assert!(enc.contains("%5B"));
        assert!(enc.contains("%5D"));
        assert!(enc.contains("%25"));
        assert!(enc.contains("%20"));
    }

    #[test]
    fn encode_userinfo_keeps_unreserved() {
        assert_eq!(encode_userinfo("Aa0-._~"), "Aa0-._~");
    }

    #[test]
    fn postgres_uri_encodes_userinfo() {
        let c = cred("postgres", "p@:/?#[]% x");
        let uri = build_uri(EngineKey::Postgresql, 5432, &c);
        let s = uri.expose();
        assert!(s.starts_with("postgresql://postgres:"));
        assert!(s.ends_with("@127.0.0.1:5432/postgres"));
        assert!(s.contains("%40"));
        assert!(!s.contains("p@"));
    }

    #[test]
    fn valkey_uri_uses_redis_scheme_and_default_user() {
        let c = cred("default", "s3cr:et");
        let uri = build_uri(EngineKey::Valkey, 6379, &c);
        assert_eq!(uri.expose(), "redis://default:s3cr%3Aet@127.0.0.1:6379/0");
    }

    #[test]
    fn postgres_dotenv_contains_required_vars() {
        let c = cred("postgres", "pw");
        let env = build_dotenv(EngineKey::Postgresql, 5432, &c)
            .expose()
            .to_owned();
        assert!(env.contains("DATABASE_URL=postgresql://postgres:pw@127.0.0.1:5432/postgres\n"));
        assert!(env.contains("DB_HOST=127.0.0.1\n"));
        assert!(env.contains("DB_PORT=5432\n"));
        assert!(env.contains("DB_DATABASE=postgres\n"));
        assert!(env.contains("DB_USERNAME=postgres\n"));
        assert!(env.contains("DB_PASSWORD=pw\n"));
    }

    #[test]
    fn valkey_dotenv_contains_required_vars() {
        let c = cred("default", "pw");
        let env = build_dotenv(EngineKey::Valkey, 6379, &c)
            .expose()
            .to_owned();
        assert!(env.contains("REDIS_URL=redis://default:pw@127.0.0.1:6379/0\n"));
        assert!(env.contains("REDIS_HOST=127.0.0.1\n"));
        assert!(env.contains("REDIS_PORT=6379\n"));
        assert!(env.contains("REDIS_PASSWORD=pw\n"));
        assert!(!env.contains("DB_USERNAME="));
        assert!(!env.contains("DATABASE_URL="));
    }

    #[test]
    fn render_dispatches_format() {
        let c = cred("postgres", "pw");
        let uri = render(EngineKey::Postgresql, 5432, &c, UriFormat::Uri);
        let env = render(EngineKey::Postgresql, 5432, &c, UriFormat::DotEnv);
        assert!(uri.expose().starts_with("postgresql://"));
        assert!(env.expose().starts_with("DATABASE_URL="));
    }

    // Item 10: no-password instances must still have a URI (PRD F-CRD-03, F-CLI-03).
    // Both engines x both formats x both auth modes.

    #[test]
    fn postgres_uri_matrix_password_and_no_password() {
        let with_pw = render(
            EngineKey::Postgresql,
            5432,
            &cred("postgres", "pw"),
            UriFormat::Uri,
        );
        assert_eq!(
            with_pw.expose(),
            "postgresql://postgres:pw@127.0.0.1:5432/postgres"
        );
        let no_pw = render_no_password(EngineKey::Postgresql, 5432, "postgres", UriFormat::Uri);
        assert_eq!(
            no_pw.expose(),
            "postgresql://postgres@127.0.0.1:5432/postgres",
            "no-password PostgreSQL URI carries the user, never `:`"
        );
        assert!(!no_pw.expose().contains(":@"));
        assert!(!no_pw.expose().starts_with("postgresql://postgres:"));
    }

    #[test]
    fn valkey_uri_matrix_password_and_no_password() {
        let with_pw = render(
            EngineKey::Valkey,
            6379,
            &cred("default", "pw"),
            UriFormat::Uri,
        );
        assert_eq!(with_pw.expose(), "redis://default:pw@127.0.0.1:6379/0");
        let no_pw = render_no_password(EngineKey::Valkey, 6379, "default", UriFormat::Uri);
        assert_eq!(
            no_pw.expose(),
            "redis://127.0.0.1:6379/0",
            "no-password Valkey URI has no userinfo at all"
        );
    }

    #[test]
    fn postgres_dotenv_matrix_password_and_no_password() {
        let with_pw = render(
            EngineKey::Postgresql,
            5432,
            &cred("postgres", "pw"),
            UriFormat::DotEnv,
        )
        .expose()
        .to_owned();
        assert!(
            with_pw.contains("DATABASE_URL=postgresql://postgres:pw@127.0.0.1:5432/postgres\n")
        );
        assert!(with_pw.trim_end().ends_with("DB_PASSWORD=pw"));

        let no_pw = render_no_password(EngineKey::Postgresql, 5432, "postgres", UriFormat::DotEnv)
            .expose()
            .to_owned();
        assert!(
            no_pw.contains("DATABASE_URL=postgresql://postgres@127.0.0.1:5432/postgres\n"),
            "{no_pw}"
        );
        assert!(no_pw.contains("DB_USERNAME=postgres\n"));
        assert!(
            !no_pw.contains("DB_PASSWORD"),
            "no-password .env must not carry a password key: {no_pw}"
        );
        assert!(!no_pw.contains(":@"));
    }

    #[test]
    fn valkey_dotenv_matrix_password_and_no_password() {
        let with_pw = render(
            EngineKey::Valkey,
            6379,
            &cred("default", "pw"),
            UriFormat::DotEnv,
        )
        .expose()
        .to_owned();
        assert!(with_pw.contains("REDIS_URL=redis://default:pw@127.0.0.1:6379/0\n"));
        assert!(with_pw.trim_end().ends_with("REDIS_PASSWORD=pw"));

        let no_pw = render_no_password(EngineKey::Valkey, 6379, "default", UriFormat::DotEnv)
            .expose()
            .to_owned();
        assert!(
            no_pw.contains("REDIS_URL=redis://127.0.0.1:6379/0\n"),
            "{no_pw}"
        );
        assert!(no_pw.contains("REDIS_HOST=127.0.0.1\n"));
        assert!(
            !no_pw.contains("REDIS_PASSWORD"),
            "no-password .env must not carry a password key: {no_pw}"
        );
    }

    #[test]
    fn exe_name_adds_extension_on_windows() {
        let name = exe_name("postgres");
        if cfg!(windows) {
            assert_eq!(name, "postgres.exe");
        } else {
            assert_eq!(name, "postgres");
        }
    }
}
