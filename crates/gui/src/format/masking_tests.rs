//! Masking tests, split from `format/tests.rs` to stay under the file limit.

use dblocal_core::model::EngineKey;

use super::*;

#[test]
fn uri_masking_hides_the_password_only() {
    let masked = mask_uri("postgresql://postgres:hunter2@127.0.0.1:5432/postgres?sslmode=disable");
    assert!(masked.starts_with("postgresql://postgres:••••••@127.0.0.1:5432/postgres"));
    assert!(!masked.contains("hunter2"));
    // MongoDB-style passwords with an encoded `@` still resolve on the last `@`.
    let tricky = mask_uri("redis://default:p%40ss@127.0.0.1:6379");
    assert!(!tricky.contains("p%40ss"));
    // No credentials: unchanged.
    assert_eq!(
        mask_uri("postgresql://127.0.0.1:5432/db"),
        "postgresql://127.0.0.1:5432/db"
    );
}

#[test]
fn dotenv_masking_replaces_only_the_password_line() {
    let block = "DB_CONNECTION=postgresql\nDB_PASSWORD=hunter2\nDB_PORT=5432";
    let masked = mask_dotenv(block);
    assert!(masked.contains("DB_PASSWORD=••••••"));
    assert!(!masked.contains("hunter2"));
    assert!(masked.contains("DB_PORT=5432"));
    // An instance created without a password has no value to hide.
    assert_eq!(
        mask_dotenv("DB_USERNAME=postgres\nDB_PASSWORD="),
        "DB_USERNAME=postgres\nDB_PASSWORD="
    );
    // The Valkey block uses a different key and must be masked too.
    let redis = mask_dotenv("REDIS_HOST=127.0.0.1\nREDIS_PASSWORD=hunter2");
    assert!(redis.contains("REDIS_PASSWORD=••••••"));
    assert!(!redis.contains("hunter2"));
}

#[test]
fn dotenv_masking_with_secret_covers_real_builder_output() {
    use dblocal_core::engine::{Credential, uri as core_uri};
    use dblocal_core::secret::SecretString;

    // The secret contains characters that need percent-encoding in the URI.
    let raw = "p@ss:w/rd%";
    let encoded = core_uri::encode_userinfo(raw);
    assert_ne!(raw, encoded, "test must exercise percent-encoding");

    for (engine, port) in [
        (EngineKey::Postgresql, 5432u16),
        (EngineKey::Mysql, 3306),
        (EngineKey::Mariadb, 3307),
        (EngineKey::Valkey, 6379),
    ] {
        let credential = Credential::new(
            if engine == EngineKey::Valkey {
                "default"
            } else {
                "root"
            },
            SecretString::new(raw),
        );
        let uri = core_uri::build_uri(engine, port, &credential);
        let dotenv = core_uri::build_dotenv(engine, port, &credential);
        let masked = mask_dotenv_with_secret(dotenv.expose(), uri.expose());

        assert!(
            !masked.contains(raw),
            "{engine:?} masked dotenv leaked raw secret: {masked}"
        );
        assert!(
            !masked.contains(&encoded),
            "{engine:?} masked dotenv leaked percent-encoded secret: {masked}"
        );
        assert!(
            masked.contains(SECRET_MASK),
            "{engine:?} masked dotenv lost the mask token: {masked}"
        );
        // URL/URI lines must not keep the secret in userinfo.
        for line in masked.lines() {
            if let Some((key, value)) = line.split_once('=') {
                let upper = key.to_ascii_uppercase();
                if upper.ends_with("_URL") || upper.ends_with("_URI") {
                    assert!(
                        !value.contains(raw) && !value.contains(&encoded),
                        "{engine:?} {key} leaked secret: {line}"
                    );
                }
            }
        }
    }
}

#[test]
fn dotenv_masking_with_secret_hides_unknown_uri_keys() {
    let uri = "postgresql://postgres:hunter2@127.0.0.1:5432/postgres";
    let block = "DATABASE_URL=postgresql://postgres:hunter2@127.0.0.1:5432/postgres\n\
                 CUSTOM_CONNECTION_URI=postgresql://postgres:hunter2@127.0.0.1:5432/postgres\n\
                 DB_PASSWORD=hunter2\n\
                 APP_SECRET_URL=https://user:hunter2@example.test/path";
    let masked = mask_dotenv_with_secret(block, uri);
    assert!(!masked.contains("hunter2"), "secret leaked: {masked}");
    assert!(masked.contains("DATABASE_URL=postgresql://postgres:••••••@127.0.0.1:5432/postgres"));
    assert!(
        masked
            .contains("CUSTOM_CONNECTION_URI=postgresql://postgres:••••••@127.0.0.1:5432/postgres")
    );
    assert!(masked.contains("DB_PASSWORD=••••••"));
    assert!(masked.contains("APP_SECRET_URL=https://user:••••••@example.test/path"));
}

#[test]
fn mask_uri_with_secret_replaces_known_secret_forms() {
    let uri = "postgresql://postgres:p%40ss%3Aw%2Frd%25@127.0.0.1:5432/postgres";
    let masked = mask_dotenv_with_secret(
        "DATABASE_URL=postgresql://postgres:p%40ss%3Aw%2Frd%25@127.0.0.1:5432/postgres\nDB_PASSWORD=p@ss:w/rd%",
        uri,
    );
    assert!(!masked.contains("p@ss:w/rd%"));
    assert!(!masked.contains("p%40ss%3Aw%2Frd%25"));
    assert!(masked.contains("DB_PASSWORD=••••••"));
}
