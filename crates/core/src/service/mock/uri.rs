//! Connection URI rendering for the mock service.
//!
//! Mirrors `F-CRD-02`: the URI keeps credentials, percent-encodes anything that
//! would break parsing, and only ever emits loopback hosts.

use crate::model::{AuthMode, EngineKey, Port};

/// Percent-encodes a URI component, leaving RFC 3986 unreserved bytes alone.
pub(crate) fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Renders the bare connection URI for an instance.
pub(crate) fn uri(
    engine: EngineKey,
    port: Port,
    user: &str,
    password: &str,
    auth: AuthMode,
) -> String {
    let user = percent_encode(user);
    let secret = percent_encode(password);
    let db = crate::consts::DEFAULT_DATABASE;
    match engine {
        EngineKey::Postgresql => match auth {
            AuthMode::Password => {
                format!("postgresql://{user}:{secret}@127.0.0.1:{port}/{db}?sslmode=disable")
            }
            AuthMode::None => format!("postgresql://{user}@127.0.0.1:{port}/{db}?sslmode=disable"),
        },
        EngineKey::Mysql | EngineKey::Mariadb => match auth {
            AuthMode::Password => format!("mysql://{user}:{secret}@127.0.0.1:{port}/{db}"),
            AuthMode::None => format!("mysql://{user}@127.0.0.1:{port}/{db}"),
        },
        EngineKey::Valkey => match auth {
            AuthMode::Password => format!("redis://{user}:{secret}@127.0.0.1:{port}"),
            AuthMode::None => format!("redis://127.0.0.1:{port}"),
        },
    }
}

/// Renders the framework-style `.env` block (`F-CRD-02` AC3).
pub(crate) fn dotenv(
    engine: EngineKey,
    port: Port,
    user: &str,
    password: &str,
    auth: AuthMode,
) -> String {
    let mut out = format!(
        "DB_CONNECTION={}\nDB_HOST=127.0.0.1\nDB_PORT={port}\nDB_DATABASE={}\nDB_USERNAME={user}",
        engine.as_str(),
        crate::consts::DEFAULT_DATABASE
    );
    if auth == AuthMode::Password {
        out.push_str(&format!("\nDB_PASSWORD={}", percent_encode(password)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(value: u16) -> Port {
        Port::new(value).expect("valid port")
    }

    #[test]
    fn postgres_uri_keeps_password_and_loopback() {
        let text = uri(
            EngineKey::Postgresql,
            port(5432),
            "postgres",
            "p@ss word",
            AuthMode::Password,
        );
        assert!(text.starts_with("postgresql://postgres:p%40ss%20word@127.0.0.1:5432/postgres"));
        assert!(text.contains("sslmode=disable"));
        assert!(!text.contains("0.0.0.0"));
    }

    #[test]
    fn valkey_uri_uses_redis_scheme() {
        let text = uri(
            EngineKey::Valkey,
            port(6379),
            "default",
            "hunter2",
            AuthMode::Password,
        );
        assert_eq!(text, "redis://default:hunter2@127.0.0.1:6379");
    }

    #[test]
    fn no_auth_uri_omits_credentials() {
        let pg = uri(
            EngineKey::Postgresql,
            port(5432),
            "postgres",
            "",
            AuthMode::None,
        );
        assert_eq!(
            pg,
            "postgresql://postgres@127.0.0.1:5432/postgres?sslmode=disable"
        );
        let redis = uri(EngineKey::Valkey, port(6379), "default", "", AuthMode::None);
        assert_eq!(redis, "redis://127.0.0.1:6379");
    }

    #[test]
    fn percent_encoding_escapes_reserved_characters() {
        assert_eq!(percent_encode("a:b/c?d"), "a%3Ab%2Fc%3Fd");
        assert_eq!(percent_encode("a.b-c_d~e9"), "a.b-c_d~e9");
        assert_eq!(percent_encode("é"), "%C3%A9");
    }

    #[test]
    fn dotenv_block_has_every_key() {
        let text = dotenv(
            EngineKey::Mysql,
            port(3306),
            "root",
            "se cret",
            AuthMode::Password,
        );
        for key in [
            "DB_CONNECTION=mysql",
            "DB_HOST=127.0.0.1",
            "DB_PORT=3306",
            "DB_DATABASE=",
            "DB_USERNAME=root",
            "DB_PASSWORD=se%20cret",
        ] {
            assert!(text.contains(key), "missing {key} in {text}");
        }
    }

    #[test]
    fn dotenv_without_password_has_no_password_key() {
        let text = dotenv(EngineKey::Mariadb, port(3307), "root", "", AuthMode::None);
        assert!(!text.contains("DB_PASSWORD"));
        assert!(text.contains("DB_CONNECTION=mariadb"));
    }
}
