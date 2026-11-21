//! Connection URI rendering for the mock service.
//!
//! Mirrors `F-CRD-02`: the URI keeps credentials, percent-encodes anything that
//! would break parsing, and only ever emits loopback hosts. The rendered shape
//! follows the real engine URI builder (`crate::engine::uri`) per engine so the
//! demo shows what the product would show.

use crate::engine::uri;
use crate::model::{AuthMode, EngineKey, Port};

/// Renders the bare connection URI for an instance.
pub(crate) fn uri(
    engine: EngineKey,
    port: Port,
    user: &str,
    password: &str,
    auth: AuthMode,
) -> String {
    let encoded_user = uri::encode_userinfo(user);
    let encoded_pass = uri::encode_userinfo(password);
    match engine {
        EngineKey::Postgresql => {
            let db = crate::consts::DEFAULT_DATABASE;
            match auth {
                AuthMode::Password => {
                    format!("postgresql://{encoded_user}:{encoded_pass}@127.0.0.1:{port}/{db}")
                }
                AuthMode::None => format!("postgresql://{encoded_user}@127.0.0.1:{port}/{db}"),
            }
        }
        EngineKey::Mysql | EngineKey::Mariadb => match auth {
            AuthMode::Password => {
                format!("mysql://{encoded_user}:{encoded_pass}@127.0.0.1:{port}/")
            }
            AuthMode::None => format!("mysql://{encoded_user}@127.0.0.1:{port}/"),
        },
        EngineKey::Valkey => match auth {
            AuthMode::Password => {
                format!("redis://{encoded_user}:{encoded_pass}@127.0.0.1:{port}/0")
            }
            // Valkey has no database name; an empty user means no userinfo.
            AuthMode::None => format!("redis://127.0.0.1:{port}/0"),
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
    let connection_uri = uri(engine, port, user, password, auth);
    let mut out = String::new();
    match engine {
        EngineKey::Postgresql => {
            out.push_str(&format!("DATABASE_URL={connection_uri}\n"));
            out.push_str("DB_HOST=127.0.0.1\n");
            out.push_str(&format!("DB_PORT={port}\n"));
            out.push_str(&format!(
                "DB_DATABASE={}\n",
                crate::consts::DEFAULT_DATABASE
            ));
            out.push_str(&format!("DB_USERNAME={user}\n"));
        }
        EngineKey::Mysql | EngineKey::Mariadb => {
            out.push_str(&format!("DATABASE_URL={connection_uri}\n"));
            out.push_str("DB_HOST=127.0.0.1\n");
            out.push_str(&format!("DB_PORT={port}\n"));
            out.push_str("DB_DATABASE=\n");
            out.push_str(&format!("DB_USERNAME={user}\n"));
        }
        EngineKey::Valkey => {
            out.push_str(&format!("REDIS_URL={connection_uri}\n"));
            out.push_str("REDIS_HOST=127.0.0.1\n");
            out.push_str(&format!("REDIS_PORT={port}\n"));
        }
    }
    if auth == AuthMode::Password {
        let key = if engine == EngineKey::Valkey {
            "REDIS_PASSWORD"
        } else {
            "DB_PASSWORD"
        };
        out.push_str(&format!("{key}={}\n", uri::encode_userinfo(password)));
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
        assert_eq!(
            text,
            "postgresql://postgres:p%40ss%20word@127.0.0.1:5432/postgres"
        );
        assert!(!text.contains("0.0.0.0"));
    }

    #[test]
    fn mariadb_uri_has_no_postgres_database_name() {
        // The demo's `wp-blog` is MariaDB on 3307; before the fix it printed
        // `.../postgres`, which is what the coordinator saw.
        let text = uri(
            EngineKey::Mariadb,
            port(3307),
            "root",
            "hunter2",
            AuthMode::Password,
        );
        assert_eq!(text, "mysql://root:hunter2@127.0.0.1:3307/");
        assert!(!text.contains("postgres"));
    }

    #[test]
    fn mysql_uri_has_no_database_name() {
        let text = uri(
            EngineKey::Mysql,
            port(3306),
            "root",
            "hunter2",
            AuthMode::Password,
        );
        assert_eq!(text, "mysql://root:hunter2@127.0.0.1:3306/");
    }

    #[test]
    fn valkey_uri_uses_redis_scheme_and_index_zero() {
        let text = uri(
            EngineKey::Valkey,
            port(6379),
            "default",
            "hunter2",
            AuthMode::Password,
        );
        assert_eq!(text, "redis://default:hunter2@127.0.0.1:6379/0");
        assert!(!text.contains("postgres"));
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
        assert_eq!(pg, "postgresql://postgres@127.0.0.1:5432/postgres");
        let redis = uri(EngineKey::Valkey, port(6379), "default", "", AuthMode::None);
        assert_eq!(redis, "redis://127.0.0.1:6379/0");
    }

    #[test]
    fn percent_encoding_escapes_reserved_characters() {
        assert_eq!(uri::encode_userinfo("a:b/c?d"), "a%3Ab%2Fc%3Fd");
        assert_eq!(uri::encode_userinfo("a.b-c_d~e9"), "a.b-c_d~e9");
        assert_eq!(uri::encode_userinfo("é"), "%C3%A9");
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
            "DATABASE_URL=mysql://root:se%20cret@127.0.0.1:3306/",
            "DB_HOST=127.0.0.1",
            "DB_PORT=3306",
            "DB_DATABASE=",
            "DB_USERNAME=root",
            "DB_PASSWORD=se%20cret",
        ] {
            assert!(text.contains(key), "missing {key} in {text}");
        }
        assert!(
            !text.contains("postgres"),
            "mysql .env leaked postgres: {text}"
        );
    }

    #[test]
    fn valkey_dotenv_uses_redis_keys_and_no_database_name() {
        let text = dotenv(
            EngineKey::Valkey,
            port(6379),
            "default",
            "pw",
            AuthMode::Password,
        );
        assert!(text.contains("REDIS_URL=redis://default:pw@127.0.0.1:6379/0"));
        assert!(text.contains("REDIS_PASSWORD=pw"));
        assert!(!text.contains("DB_DATABASE"));
        assert!(!text.contains("postgres"));
    }

    #[test]
    fn dotenv_without_password_has_no_password_key() {
        let text = dotenv(EngineKey::Mariadb, port(3307), "root", "", AuthMode::None);
        assert!(!text.contains("DB_PASSWORD"));
        assert!(text.contains("DATABASE_URL=mysql://root@127.0.0.1:3307/"));
    }
}
