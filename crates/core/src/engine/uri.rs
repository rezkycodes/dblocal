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
    let user = encode_userinfo(&credential.username);
    let pass = encode_userinfo(credential.password.expose());
    match engine {
        EngineKey::Postgresql => SecretString::new(format!(
            "postgresql://{user}:{pass}@127.0.0.1:{port}/{}",
            consts::DEFAULT_DATABASE
        )),
        EngineKey::Valkey => SecretString::new(format!("redis://{user}:{pass}@127.0.0.1:{port}/0")),
        EngineKey::Mysql | EngineKey::Mariadb => {
            SecretString::new(format!("mysql://{user}:{pass}@127.0.0.1:{port}/"))
        }
    }
}

/// Builds the `.env` block for an engine instance.
pub fn build_dotenv(engine: EngineKey, port: u16, credential: &Credential) -> SecretString {
    let uri = build_uri(engine, port, credential);
    let mut out = String::new();
    match engine {
        EngineKey::Postgresql => {
            let _ = writeln!(out, "DATABASE_URL={}", uri.expose());
            let _ = writeln!(out, "DB_HOST=127.0.0.1");
            let _ = writeln!(out, "DB_PORT={port}");
            let _ = writeln!(out, "DB_DATABASE={}", consts::DEFAULT_DATABASE);
            let _ = writeln!(out, "DB_USERNAME={}", credential.username);
            let _ = writeln!(out, "DB_PASSWORD={}", credential.password.expose());
        }
        EngineKey::Valkey => {
            let _ = writeln!(out, "REDIS_URL={}", uri.expose());
            let _ = writeln!(out, "REDIS_HOST=127.0.0.1");
            let _ = writeln!(out, "REDIS_PORT={port}");
            let _ = writeln!(out, "REDIS_PASSWORD={}", credential.password.expose());
        }
        EngineKey::Mysql | EngineKey::Mariadb => {
            let _ = writeln!(out, "DATABASE_URL={}", uri.expose());
            let _ = writeln!(out, "DB_HOST=127.0.0.1");
            let _ = writeln!(out, "DB_PORT={port}");
            let _ = writeln!(out, "DB_DATABASE=");
            let _ = writeln!(out, "DB_USERNAME={}", credential.username);
            let _ = writeln!(out, "DB_PASSWORD={}", credential.password.expose());
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
    match format {
        UriFormat::Uri => build_uri(engine, port, credential),
        UriFormat::DotEnv => build_dotenv(engine, port, credential),
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
