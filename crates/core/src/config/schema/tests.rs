//! Tests for the setting schema, sizes and validation.

use super::*;

fn pg(key: &str) -> &'static SettingSpec {
    spec_for(EngineKey::Postgresql, key).expect("pg setting")
}
fn my(key: &str) -> &'static SettingSpec {
    spec_for(EngineKey::Mysql, key).expect("mysql setting")
}
fn vk(key: &str) -> &'static SettingSpec {
    spec_for(EngineKey::Valkey, key).expect("valkey setting")
}

#[test]
fn the_subset_is_exactly_the_prd_list_per_engine() {
    let keys = |engine| {
        settings_for(engine)
            .iter()
            .map(|s| s.key)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        keys(EngineKey::Postgresql),
        vec!["max_connections", "shared_buffers", "work_mem", "timezone"]
    );
    assert_eq!(
        keys(EngineKey::Mysql),
        vec![
            "max_connections",
            "innodb_buffer_pool_size",
            "sort_buffer_size",
            "default-time-zone",
            "character-set-server"
        ]
    );
    assert_eq!(
        keys(EngineKey::Mariadb),
        vec![
            "max_connections",
            "innodb_buffer_pool_size",
            "sort_buffer_size",
            "default-time-zone",
            "character-set-server"
        ]
    );
    assert_eq!(
        keys(EngineKey::Valkey),
        vec![
            "maxmemory",
            "maxmemory-policy",
            "appendonly",
            "save",
            "timeout"
        ]
    );
}

#[test]
fn defaults_match_the_real_engines() {
    // PostgreSQL 18.7 boot values.
    assert_eq!(pg("max_connections").default, "100");
    assert_eq!(pg("shared_buffers").default, "128MB");
    assert_eq!(pg("work_mem").default, "4MB");
    assert_eq!(pg("timezone").default, "GMT");
    // MySQL 8.4.11.
    assert_eq!(my("max_connections").default, "151");
    assert_eq!(my("innodb_buffer_pool_size").default, "128MB");
    assert_eq!(my("sort_buffer_size").default, "256KB");
    assert_eq!(my("default-time-zone").default, "");
    assert_eq!(my("character-set-server").default, "utf8mb4");
    // MariaDB 11.8.10 differs from MySQL only in sort_buffer_size (2MB).
    let mdb = spec_for(EngineKey::Mariadb, "sort_buffer_size").expect("mdb");
    assert_eq!(mdb.default, "2MB");
    // Valkey 9.2.0.
    assert_eq!(vk("maxmemory").default, "0");
    assert_eq!(vk("maxmemory-policy").default, "noeviction");
    assert_eq!(vk("appendonly").default, "no");
    assert_eq!(vk("save").default, "3600 1 300 100 60 10000");
    assert_eq!(vk("timeout").default, "0");
}

#[test]
fn every_default_parses_as_its_own_kind() {
    for engine in EngineKey::ALL {
        for spec in settings_for(engine) {
            if spec.default.is_empty() {
                continue;
            }
            validate_value(spec, spec.default).unwrap_or_else(|e| {
                panic!("{engine:?} {} default {:?}: {e:?}", spec.key, spec.default)
            });
        }
    }
}

#[test]
fn every_setting_needs_a_restart() {
    for engine in EngineKey::ALL {
        for spec in settings_for(engine) {
            assert_eq!(spec.apply, ApplyTime::Restart, "{}", spec.key);
        }
    }
}

#[test]
fn parse_size_accepts_the_engine_units() {
    assert_eq!(parse_size("128MB").unwrap(), 128 * 1024 * 1024);
    assert_eq!(parse_size("1GB").unwrap(), 1024 * 1024 * 1024);
    assert_eq!(parse_size("256KB").unwrap(), 256 * 1024);
    assert_eq!(parse_size("4MB").unwrap(), 4 * 1024 * 1024);
    assert_eq!(parse_size("512kB").unwrap(), 512 * 1024);
    assert_eq!(parse_size("134217728").unwrap(), 134217728);
    assert_eq!(parse_size("1T").unwrap(), 1024u64 * 1024 * 1024 * 1024);
    assert_eq!(parse_size("0").unwrap(), 0);
}

#[test]
fn parse_size_rejects_junk_and_fractions() {
    for bad in ["", "MB", "12XB", "1.5GB", "-1MB", "1 2"] {
        assert!(parse_size(bad).is_err(), "{bad:?} should fail");
    }
}

#[test]
fn format_size_is_inverse_on_whole_units() {
    assert_eq!(format_size(128 * 1024 * 1024), "128MB");
    assert_eq!(format_size(1024 * 1024 * 1024), "1GB");
    assert_eq!(format_size(256 * 1024), "256kB");
    assert_eq!(format_size(5000), "5000");
}

#[test]
fn integer_range_is_enforced() {
    let spec = pg("max_connections");
    validate_value(spec, "1").expect("min");
    validate_value(spec, "262143").expect("max");
    assert!(validate_value(spec, "0").is_err());
    assert!(validate_value(spec, "262144").is_err());
    assert!(validate_value(spec, "abc").is_err());
    assert!(validate_value(spec, "-1").is_err());
}

#[test]
fn size_range_is_enforced() {
    let spec = pg("shared_buffers");
    validate_value(spec, "128kB").expect("min");
    validate_value(spec, "1GB").expect("mid");
    assert!(validate_value(spec, "64kB").is_err(), "below min");
    assert!(validate_value(spec, "junk").is_err());
}

#[test]
fn boolean_words_are_engine_specific() {
    let spec = vk("appendonly");
    validate_value(spec, "yes").expect("yes");
    validate_value(spec, "no").expect("no");
    validate_value(spec, "YES").expect("case-insensitive");
    assert!(validate_value(spec, "on").is_err(), "valkey uses yes/no");
}

#[test]
fn enum_rejects_outside_the_set() {
    let spec = vk("maxmemory-policy");
    validate_value(spec, "allkeys-lru").expect("valid");
    assert!(validate_value(spec, "bogus").is_err());
    let cs = my("character-set-server");
    validate_value(cs, "utf8mb4").expect("valid");
    assert!(validate_value(cs, "klingon").is_err());
}

#[test]
fn timezone_accepts_named_zones_and_offsets() {
    let spec = pg("timezone");
    validate_value(spec, "UTC").expect("UTC");
    validate_value(spec, "GMT").expect("GMT");
    validate_value(spec, "Asia/Makassar").expect("Area/Location");
    validate_value(spec, "Europe/Berlin").expect("Area/Location");
    validate_value(spec, "+07:00").expect("offset");
    validate_value(spec, "-08:00").expect("offset");
    // A valid shape but a zone that does not exist: the engines' own rule is the
    // system tzdata, so this is an error where tzdata is present.
    if std::path::Path::new("/usr/share/zoneinfo").is_dir() {
        assert!(validate_value(spec, "Mars/Olympus").is_err());
    }
    // Always refused regardless of tzdata:
    assert!(validate_value(spec, "no-slash").is_err(), "shape");
    assert!(
        validate_value(spec, "/etc/passwd").is_err(),
        "absolute path"
    );
    assert!(
        validate_value(spec, "../../etc/passwd").is_err(),
        "traversal"
    );
    assert!(validate_value(spec, "+99:00").is_err());
    assert!(validate_value(spec, "").is_err());
}

#[test]
fn token_accepts_a_save_schedule_and_rejects_an_injection() {
    let spec = vk("save");
    validate_value(spec, "3600 1 300 100").expect("schedule");
    validate_value(spec, "").expect("empty disables");
    assert!(validate_value(spec, "3600 1 requirepass x").is_err());
}

#[test]
fn a_value_that_starts_a_comment_is_refused_for_every_kind() {
    // Even though none of the numeric kinds would accept `#`, the shared guard
    // must run first and return a control error, not a parse error.
    for engine in EngineKey::ALL {
        for spec in settings_for(engine) {
            for value in ["1 #x", "#x", "1 ;y"] {
                assert!(
                    validate_value(spec, value).is_err(),
                    "{engine:?} {} accepted {value:?}",
                    spec.key
                );
            }
        }
    }
}

#[test]
fn control_characters_are_refused() {
    for engine in EngineKey::ALL {
        for spec in settings_for(engine) {
            for value in ["1\n2", "1\r2", "1\u{0}2"] {
                assert!(
                    validate_value(spec, value).is_err(),
                    "{} {}",
                    spec.key,
                    value.escape_default()
                );
            }
        }
    }
}

#[test]
fn hosted_corpus_of_values_is_rejected_or_accepted_as_expected() {
    // A hostile corpus as W3-A did for identifiers: the values an operator might
    // paste to try to break out of the single line.
    let spec = my("max_connections");
    let bad = [
        "151\nmax_connections=1",
        "151; DROP TABLE x",
        "151 #comment",
        "151\u{0}",
        "1e9",
        "１５１", // fullwidth digits
        "999999999999999999999999",
    ];
    for value in bad {
        assert!(validate_value(spec, value).is_err(), "accepted {value:?}");
    }
    validate_value(spec, "151").expect("the honest value");
}

#[test]
fn managed_keys_are_recognised_case_insensitively() {
    for key in [
        "port",
        "PORT",
        "socket",
        "tmpdir",
        "log-error",
        "pid-file",
        "datadir",
        "bind-address",
        "listen_addresses",
        "unix_socket_directories",
        "requirepass",
        "dir",
    ] {
        assert!(is_managed_key(key), "{key} must be managed");
    }
    for key in ["max_connections", "shared_buffers", "work_mem", "timezone"] {
        assert!(!is_managed_key(key), "{key} must be editable");
    }
}

#[test]
fn no_managed_key_is_in_the_editable_subset() {
    for engine in EngineKey::ALL {
        for spec in settings_for(engine) {
            assert!(!is_managed_key(spec.key), "{} is managed", spec.key);
        }
    }
}

#[test]
fn readonly_keys_state_the_port_and_pg_auth() {
    assert_eq!(
        readonly_keys(EngineKey::Postgresql),
        &["port", "authentication method"]
    );
    assert_eq!(readonly_keys(EngineKey::Mysql), &["port"]);
    assert_eq!(readonly_keys(EngineKey::Valkey), &["port"]);
}

#[test]
fn provenance_names_a_version_per_engine() {
    for engine in EngineKey::ALL {
        assert!(
            DEFAULTS_PROVENANCE
                .iter()
                .any(|(e, _, _)| *e == engine.as_str()),
            "no provenance for {engine:?}"
        );
    }
}
