use dblocal_core::model::{
    AuthMode, CatalogStatus, CredentialBackend, EngineKey, InstanceId, InstanceName, InstanceSpec,
    InstanceStatus, Port, Runtime, RuntimeState, StartFailure,
};
use dblocal_core::platform::Arch;
use dblocal_core::service::FailureInfo;

use super::*;

#[path = "palette_tests.rs"]
mod palette_tests;

fn view(name: &str, engine: EngineKey, status: InstanceStatus, port: u16) -> InstanceView {
    view_version(name, engine, "17.4", status, port)
}

fn view_version(
    name: &str,
    engine: EngineKey,
    version: &str,
    status: InstanceStatus,
    port: u16,
) -> InstanceView {
    let spec = InstanceSpec::new(
        InstanceName::new(name).expect("valid name"),
        engine,
        version,
        Some(Port::new(port).expect("valid port")),
        AuthMode::Password,
    )
    .expect("spec");
    let now = std::time::SystemTime::UNIX_EPOCH;
    let instance = dblocal_core::model::Instance::new(&spec, InstanceId::new(), status, now);
    InstanceView::from_instance(&instance)
}

fn runtime(engine: EngineKey, version: &str) -> Runtime {
    Runtime::new(
        format!("rt-{}-{version}", engine.as_str()),
        engine,
        version,
        Arch::X86_64,
        std::path::PathBuf::from("/tmp/runtime"),
        40 * 1024 * 1024,
        std::time::SystemTime::UNIX_EPOCH,
        RuntimeState::Installed,
    )
}

#[test]
fn every_status_has_a_glyph_and_a_nonempty_label() {
    for status in [
        InstanceStatus::Running,
        InstanceStatus::Stopped,
        InstanceStatus::Failed,
        InstanceStatus::Provisioning,
        InstanceStatus::Initializing,
        InstanceStatus::Starting,
        InstanceStatus::Stopping,
        InstanceStatus::Deleting,
        InstanceStatus::Draft,
    ] {
        assert!(!status_label(status).is_empty(), "{status:?}");
    }
    assert_eq!(
        status_label(InstanceStatus::Running),
        strings::STATUS_RUNNING
    );
    assert_eq!(status_glyph(InstanceStatus::Running), StatusGlyph::Running);
    assert_eq!(status_glyph(InstanceStatus::Stopped), StatusGlyph::Stopped);
    assert_eq!(status_glyph(InstanceStatus::Failed), StatusGlyph::Failed);
    for busy in [
        InstanceStatus::Provisioning,
        InstanceStatus::Initializing,
        InstanceStatus::Starting,
        InstanceStatus::Stopping,
        InstanceStatus::Deleting,
    ] {
        assert_eq!(status_glyph(busy), StatusGlyph::Busy, "{busy:?}");
        assert!(is_busy(busy));
    }
    assert!(!is_busy(InstanceStatus::Running));
}

/// `F-PLH-02` AC2: status is never colour alone — every status maps to a
/// non-empty label **and** a glyph shape, and no two status families share a
/// shape. This is the table the requirement names: a label per variant, and a
/// distinct glyph per family so the shape alone also distinguishes them.
#[test]
fn every_status_has_a_label_and_the_glyph_families_are_distinct() {
    let all = [
        InstanceStatus::Running,
        InstanceStatus::Stopped,
        InstanceStatus::Failed,
        InstanceStatus::Provisioning,
        InstanceStatus::Initializing,
        InstanceStatus::Starting,
        InstanceStatus::Stopping,
        InstanceStatus::Deleting,
        InstanceStatus::Draft,
    ];
    // Every variant: a non-empty label (never colour alone).
    for status in all {
        assert!(
            !status_label(status).trim().is_empty(),
            "{status:?} must have a text label, not colour alone"
        );
    }
    // The four glyph shapes are pairwise distinct, so the shape is a second,
    // colour-blind-safe signal.
    let glyphs = [
        StatusGlyph::Running,
        StatusGlyph::Stopped,
        StatusGlyph::Failed,
        StatusGlyph::Busy,
    ];
    for (i, a) in glyphs.iter().enumerate() {
        for b in &glyphs[i + 1..] {
            assert_ne!(a, b, "glyph shapes must be distinct: {a:?} vs {b:?}");
        }
    }
    // Each family is reachable from at least one status, so none is dead.
    let mapped: Vec<StatusGlyph> = all.iter().map(|s| status_glyph(*s)).collect();
    for glyph in glyphs {
        assert!(
            mapped.contains(&glyph),
            "glyph {glyph:?} is never used by any status"
        );
    }
}

#[test]
fn actions_follow_the_state_machine() {
    assert!(can_start(InstanceStatus::Stopped));
    assert!(can_start(InstanceStatus::Failed));
    assert!(!can_start(InstanceStatus::Running));
    assert!(can_stop(InstanceStatus::Running));
    assert!(!can_stop(InstanceStatus::Stopped));
    assert!(can_restart(InstanceStatus::Running));
    assert!(!can_restart(InstanceStatus::Failed));
    assert!(can_delete(InstanceStatus::Stopped));
    assert!(can_delete(InstanceStatus::Failed));
    assert!(!can_delete(InstanceStatus::Running));
    assert!(delete_needs_stop(InstanceStatus::Running));
    assert!(!delete_needs_stop(InstanceStatus::Stopped));
}

#[test]
fn monograms_match_the_prototype() {
    assert_eq!(engine_monogram(EngineKey::Postgresql), "PG");
    assert_eq!(engine_monogram(EngineKey::Mysql), "MY");
    assert_eq!(engine_monogram(EngineKey::Mariadb), "MA");
    assert_eq!(engine_monogram(EngineKey::Valkey), "VK");
}

#[test]
fn grouping_skips_empty_engines_and_sorts_names() {
    let instances = vec![
        view("zeta", EngineKey::Postgresql, InstanceStatus::Stopped, 5432),
        view(
            "alpha",
            EngineKey::Postgresql,
            InstanceStatus::Running,
            5433,
        ),
        view("cache", EngineKey::Valkey, InstanceStatus::Running, 6379),
    ];
    let groups = group_by_engine(&instances);
    assert_eq!(groups.len(), 2, "mysql and mariadb are empty");
    assert_eq!(groups[0].engine, EngineKey::Postgresql);
    let names: Vec<&str> = groups[0]
        .instances
        .iter()
        .map(|v| v.name.as_str())
        .collect();
    assert_eq!(names, ["alpha", "zeta"]);
    assert_eq!(groups[1].label, "Valkey");
}

#[test]
fn grouping_on_no_instances_is_empty() {
    assert!(group_by_engine(&[]).is_empty());
}

#[test]
fn filtering_is_case_insensitive_and_keeps_everything_when_empty() {
    let instances = vec![
        view(
            "pg-app",
            EngineKey::Postgresql,
            InstanceStatus::Running,
            5432,
        ),
        view(
            "cache-dev",
            EngineKey::Valkey,
            InstanceStatus::Running,
            6379,
        ),
    ];
    assert_eq!(filter_instances(&instances, "").len(), 2);
    assert_eq!(filter_instances(&instances, "   ").len(), 2);
    let hits = filter_instances(&instances, "CACHE");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name, "cache-dev");
    assert!(filter_instances(&instances, "shop").is_empty());
}

#[test]
fn counts_are_bucketed_by_status() {
    let instances = vec![
        view(
            "aa-app",
            EngineKey::Postgresql,
            InstanceStatus::Running,
            5432,
        ),
        view(
            "bb-app",
            EngineKey::Postgresql,
            InstanceStatus::Stopped,
            5433,
        ),
        view(
            "cc-app",
            EngineKey::Postgresql,
            InstanceStatus::Failed,
            5434,
        ),
        view(
            "dd-cache",
            EngineKey::Valkey,
            InstanceStatus::Starting,
            6379,
        ),
    ];
    let counts = instance_counts(&instances);
    assert_eq!(counts.running, 1);
    assert_eq!(counts.stopped, 1);
    assert_eq!(counts.failed, 1);
    assert!(!counts.is_empty());
    assert_eq!(running_count(&instances), 1);
    assert_eq!(stopped_count(&instances), 1);
    assert_eq!(failed_count(&instances), 1);
    assert!(instance_counts(&[]).is_empty());
}

#[test]
fn byte_formatting_uses_sensible_units() {
    assert_eq!(format_bytes(0), "0 B");
    assert_eq!(format_bytes(512), "512 B");
    assert_eq!(format_bytes(1024), "1.00 KB");
    assert_eq!(format_bytes(54_525_952), "52.0 MB");
    assert_eq!(format_bytes(281_000_000), "268 MB");
    assert_eq!(format_bytes(2_147_483_648), "2.00 GB");
}

#[test]
fn speed_and_eta_formatting() {
    assert_eq!(format_speed(13 * 1024 * 1024), "13.0 MB/s");
    assert_eq!(format_eta(0), "0s");
    assert_eq!(format_eta(45), "45s");
    assert_eq!(format_eta(65), "1m 5s");
    assert_eq!(format_eta(120), "2m");
    assert_eq!(format_eta(7_380), "2h 3m");
}

#[test]
fn dock_meta_combines_short_version_and_port() {
    let v = view("app", EngineKey::Postgresql, InstanceStatus::Stopped, 5432);
    assert_eq!(dock_meta(&v), "17.4 · :5432");
    assert_eq!(short_version("18.7.0"), "18.7");
    assert_eq!(short_version("18"), "18");
}

#[test]
fn header_subtitle_names_engine_endpoint_state_and_process_mode() {
    let v = view("app", EngineKey::Postgresql, InstanceStatus::Running, 5432);
    assert_eq!(
        header_subtitle(&v),
        "PostgreSQL 17.4 · 127.0.0.1:5432 · running · detached"
    );
}

#[test]
fn credential_labels_follow_the_backend() {
    assert_eq!(
        credential_backend_label(Some(CredentialBackend::Keychain)),
        strings::UI_CREDENTIAL_KEYCHAIN
    );
    assert_eq!(
        credential_backend_label(Some(CredentialBackend::File)),
        strings::UI_CREDENTIAL_FILE
    );
    assert_eq!(
        credential_backend_label(None),
        strings::UI_CREDENTIAL_KEYCHAIN
    );
    assert!(credential_is_file(Some(CredentialBackend::File)));
    assert!(!credential_is_file(Some(CredentialBackend::Keychain)));
    assert!(!credential_is_file(None));
}

#[test]
fn detail_values_name_the_engine_and_mark_the_canonical_port() {
    let v = view("app", EngineKey::Postgresql, InstanceStatus::Running, 5432);
    assert_eq!(engine_detail(&v, "x86_64"), "PostgreSQL 17.4 · x86_64");
    assert!(is_canonical_port(&v));
    let off = view("app2", EngineKey::Postgresql, InstanceStatus::Running, 5433);
    assert!(!is_canonical_port(&off));
    // `data_dir` joins natively, so a POSIX literal root prints with the host
    // separator for the joined components (`/home/u/.dblocal\instances\app\data`
    // on Windows). Build the expectation the same way instead of hardcoding `/`.
    let data_root = std::path::Path::new("/home/u/.dblocal");
    let expected_data = data_root
        .join("instances")
        .join("app")
        .join("data")
        .to_string_lossy()
        .into_owned();
    assert_eq!(data_dir(data_root, None, "app"), expected_data);
    assert_eq!(credential_ref("app"), "dblocal:app");
}

#[test]
fn download_row_reports_percent_detail_and_eta() {
    let row = download_row(
        EngineKey::Postgresql,
        "17.4",
        26 * 1024 * 1024,
        52 * 1024 * 1024,
        13 * 1024 * 1024,
        Some(2),
    );
    assert_eq!(row.percent, 50);
    assert!(row.title.contains("PostgreSQL"));
    assert!(row.detail.contains("26.0 MB"));
    assert!(row.detail.contains("MB/s"));
    assert_eq!(row.eta.as_deref(), Some("2s"));
    assert_eq!(
        download_row(EngineKey::Valkey, "8.1.0", 1, 0, 0, None).percent,
        0
    );
}

#[test]
fn env_lines_split_key_from_value() {
    let rows = env_lines("DB_HOST=127.0.0.1\n# comment\nDB_PASSWORD=secret");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].key.as_deref(), Some("DB_HOST"));
    assert_eq!(rows[0].value, "127.0.0.1");
    assert!(rows[1].key.is_none());
    assert_eq!(rows[2].value, "secret");
}

#[test]
fn catalog_banner_and_label_follow_status() {
    let catalog = CatalogView {
        status: CatalogStatus::Stale,
        banner: Some(strings::CATALOG_STALE.to_owned()),
        engines: vec![],
    };
    assert_eq!(catalog_banner(&catalog), Some(strings::CATALOG_STALE));
    assert_eq!(catalog_status_label(&catalog), strings::CATALOG_STALE_LABEL);
    assert_eq!(
        catalog_freshness_word(CatalogStatus::Stale),
        strings::CATALOG_STALE_LABEL
    );
    let offline = CatalogView {
        status: CatalogStatus::Offline,
        banner: None,
        engines: vec![],
    };
    assert_eq!(
        catalog_status_label(&offline),
        strings::CATALOG_OFFLINE_LABEL
    );
    assert_eq!(
        catalog_freshness_word(CatalogStatus::Fresh),
        strings::CATALOG_FRESH_LABEL
    );
    assert!(catalog_banner(&offline).is_none());
}

#[test]
fn every_diagnosis_maps_to_readable_text_not_a_slug() {
    use dblocal_core::model::StartFailure as D;
    for (diagnosis, expected) in [
        (D::PortInUse, strings::DIAGNOSIS_PORT_IN_USE),
        (
            D::DataDirVersionMismatch,
            strings::DIAGNOSIS_VERSION_MISMATCH,
        ),
        (D::PermissionDenied, strings::DIAGNOSIS_PERMISSION_DENIED),
        (D::BinaryMissing, strings::DIAGNOSIS_BINARY_MISSING),
        (D::Unknown, strings::DIAGNOSIS_UNKNOWN),
    ] {
        let label = diagnosis_label(diagnosis);
        assert_eq!(label, expected);
        // Never the machine slug the review caught.
        assert_ne!(label, diagnosis.as_str(), "{diagnosis:?} leaked its slug");
        assert!(!label.contains('_'));
    }
}

#[test]
fn only_a_port_conflict_offers_the_one_click_fix() {
    let failed = view("shop", EngineKey::Postgresql, InstanceStatus::Failed, 5434).with_failure(
        FailureInfo {
            code: "DBL-1005".to_owned(),
            message: strings::ERR_PORT_IN_USE.to_owned(),
            diagnosis: StartFailure::PortInUse,
            log_tail: vec!["bind failed".to_owned()],
            restore_available: false,
        },
    );
    assert!(offers_port_fix(&failed));
    assert_eq!(suggested_fix_port(&failed, Some(5435)), 5435);
    assert_eq!(suggested_fix_port(&failed, None), 5435);
    // A suggestion equal to the current port is useless; fall through.
    assert_eq!(suggested_fix_port(&failed, Some(5434)), 5435);

    let other =
        view("boom", EngineKey::Valkey, InstanceStatus::Failed, 6379).with_failure(FailureInfo {
            code: "DBL-4001".to_owned(),
            message: strings::ERR_ENGINE.to_owned(),
            diagnosis: StartFailure::Unknown,
            log_tail: vec![],
            restore_available: false,
        });
    assert!(!offers_port_fix(&other));

    let running = view("ok", EngineKey::Postgresql, InstanceStatus::Running, 5432);
    assert!(!offers_port_fix(&running));
}

#[test]
fn runtime_rows_name_users_or_say_unused() {
    let runtimes = vec![
        runtime(EngineKey::Postgresql, "17.4"),
        runtime(EngineKey::Postgresql, "16.6"),
    ];
    let instances = vec![
        view_version(
            "pg-app",
            EngineKey::Postgresql,
            "17.4",
            InstanceStatus::Running,
            5432,
        ),
        view_version(
            "pg-legacy",
            EngineKey::Postgresql,
            "16.6",
            InstanceStatus::Stopped,
            5433,
        ),
    ];
    let rows = runtime_rows(&runtimes, &instances);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].title, "PostgreSQL 16.6");
    assert_eq!(rows[0].users, 1);
    assert!(!rows[0].unused);
    assert_eq!(
        rows[0].usage,
        format!("1 {}", strings::UI_RUNTIME_INSTANCE_COUNT)
    );
    assert_eq!(rows[1].title, "PostgreSQL 17.4");
    assert_eq!(rows[1].users, 1);

    let unused = runtime_row(&runtime(EngineKey::Valkey, "7.2.8"), &instances);
    assert!(unused.unused);
    assert_eq!(unused.users, 0);
    assert_eq!(unused.usage, strings::UI_RUNTIME_UNUSED);
    assert_eq!(runtime_engine(&runtimes[0]), EngineKey::Postgresql);
}

#[test]
fn form_validation_rejects_short_names_and_low_ports() {
    let bad_name = validate_form("a", "5432", &[]);
    assert!(bad_name.name_error.is_some());
    assert!(!bad_name.is_valid());

    let low_port = validate_form("app-dev", "80", &[]);
    assert!(low_port.port_error.is_some());

    let ok = validate_form("app-dev", "5432", &[]);
    assert!(ok.is_valid());
    assert!(ok.name_error.is_none() && ok.port_error.is_none());
}

#[test]
fn form_validation_reports_the_port_owner() {
    let instances = vec![view(
        "taken",
        EngineKey::Postgresql,
        InstanceStatus::Stopped,
        5432,
    )];
    let result = validate_form("fresh", "5432", &instances);
    assert!(!result.is_valid());
    assert_eq!(
        result.conflict.as_deref(),
        Some(dblocal_core::strings::validate_port_taken(5432, "taken").as_str())
    );
}

#[test]
fn form_validation_allows_an_empty_port_for_auto_allocation() {
    let result = validate_form("app-dev", "", &[]);
    assert!(result.is_valid());
}

/// D-26: the create dialog's engine cards, built from the real catalog view
/// for Windows, must show Valkey as unavailable with the PRD reason. This drives
/// the same helpers the dialog renders (`create_engine_cards`,
/// `engine_unavailable_reason`), so a Linux test proves the Windows dialog
/// disables the card without a render test or a Windows machine.
#[test]
fn create_cards_mark_valkey_unavailable_on_windows() {
    use dblocal_core::catalog::{InstalledVersions, build_catalog_view, bundled_snapshot};
    use dblocal_core::platform::Os;

    let snapshot = bundled_snapshot().expect("bundled catalog");
    let view = build_catalog_view(
        &snapshot,
        Os::Windows,
        Arch::X86_64,
        &InstalledVersions::empty(),
        None,
    );
    let cards = create_engine_cards(&view.engines);
    let valkey = cards
        .iter()
        .find(|entry| entry.engine == EngineKey::Valkey)
        .expect("valkey card present");
    assert!(
        !valkey.available,
        "the Windows dialog must disable the Valkey card"
    );
    assert_eq!(
        engine_unavailable_reason(valkey),
        Some(dblocal_core::catalog::UNAVAILABLE_ON_PLATFORM)
    );
    // PostgreSQL keeps its Windows versions, so the dialog stays usable.
    let postgres = cards
        .iter()
        .find(|entry| entry.engine == EngineKey::Postgresql)
        .expect("postgres card");
    assert!(postgres.available);
    assert!(engine_unavailable_reason(postgres).is_none());
}

#[test]
fn engine_unavailability_and_auth_expose_their_state() {
    let entry = EngineEntry {
        engine: EngineKey::Mariadb,
        available: false,
        unavailable_reason: Some(strings::CATALOG_ENGINE_UNAVAILABLE_PLATFORM.to_owned()),
        versions: vec![],
    };
    assert_eq!(
        engine_unavailable_reason(&entry),
        Some(strings::CATALOG_ENGINE_UNAVAILABLE_PLATFORM)
    );
    assert_eq!(
        version_unavailable_reason(&entry),
        Some(strings::CATALOG_ENGINE_UNAVAILABLE_PLATFORM)
    );
    assert!(auth_needs_password(AuthMode::Password));
    assert!(!auth_needs_password(AuthMode::None));
}

#[test]
fn an_available_engine_has_no_version_blocker() {
    let entry = EngineEntry {
        engine: EngineKey::Postgresql,
        available: true,
        unavailable_reason: None,
        versions: vec![],
    };
    assert!(version_unavailable_reason(&entry).is_none());
    assert!(engine_unavailable_reason(&entry).is_none());
    // Unavailable without a reason still names the platform (F-CAT-02 AC2).
    let bare = EngineEntry {
        engine: EngineKey::Valkey,
        available: false,
        unavailable_reason: None,
        versions: vec![],
    };
    assert_eq!(
        version_unavailable_reason(&bare),
        Some(strings::CATALOG_ENGINE_UNAVAILABLE_PLATFORM)
    );
}

#[test]
fn create_defaults_follow_the_prototype_prefill() {
    assert_eq!(suggested_name(EngineKey::Postgresql, "17.4"), "postgres-17");
    assert_eq!(suggested_name(EngineKey::Valkey, "8.1.0"), "valkey-8");
    assert_eq!(suggested_name(EngineKey::Mysql, "8.4.4"), "mysql-8");
    assert_eq!(suggested_name(EngineKey::Mariadb, "11.4.2"), "mariadb-11");
    assert!(create_needs_download(false));
    assert!(!create_needs_download(true));
}

#[test]
fn next_free_port_skips_allocated_ports() {
    let instances = vec![
        view(
            "aa-app",
            EngineKey::Postgresql,
            InstanceStatus::Running,
            5432,
        ),
        view(
            "bb-app",
            EngineKey::Postgresql,
            InstanceStatus::Running,
            5433,
        ),
    ];
    assert_eq!(next_free_port(&instances, EngineKey::Postgresql), 5434);
    assert_eq!(next_free_port(&[], EngineKey::Valkey), 6379);
}

#[test]
fn default_version_prefers_the_newest_supported_entry() {
    use dblocal_core::service::VersionEntry;
    let entry = EngineEntry {
        engine: EngineKey::Postgresql,
        available: true,
        unavailable_reason: None,
        versions: vec![
            VersionEntry {
                version: "17.4".to_owned(),
                eol: false,
                installed: true,
                size_bytes: 1,
            },
            VersionEntry {
                version: "15.10".to_owned(),
                eol: true,
                installed: false,
                size_bytes: 1,
            },
        ],
    };
    assert_eq!(
        default_version(&entry).map(|v| v.version.as_str()),
        Some("17.4")
    );
    assert_eq!(default_version_index(&entry), 0);

    let eol_only = EngineEntry {
        engine: EngineKey::Mysql,
        available: true,
        unavailable_reason: None,
        versions: vec![VersionEntry {
            version: "8.0.40".to_owned(),
            eol: true,
            installed: false,
            size_bytes: 1,
        }],
    };
    assert_eq!(
        default_version(&eol_only).map(|v| v.version.as_str()),
        Some("8.0.40")
    );
    assert_eq!(default_version_index(&eol_only), 0);
}

#[test]
fn platform_label_joins_os_and_arch() {
    assert_eq!(platform_label("linux", "x86_64"), "linux x86_64");
}

// --- review round 1 regression guards ----------------------------------------

#[test]
fn connection_title_appears_once() {
    // Defect 2: the card chrome and the header row both drew "Connection".
    let header = connection_header(strings::UI_CREDENTIAL_KEYCHAIN);
    assert_eq!(header.card_title, None);
    assert_eq!(connection_title_occurrences(&header), 1);
}

#[test]
fn dock_row_labels_are_one_line_and_accessible() {
    // Defect 5: the row must read as glyph, name, version, :port on one line,
    // while the accessible label keeps the status word (F-PLH-02).
    let v = view(
        "pg-app",
        EngineKey::Postgresql,
        InstanceStatus::Running,
        5432,
    );
    let labels = dock_row_labels(&v);
    assert_eq!(labels, ["pg-app", "17.4", ":5432"]);
    let accessible = dock_row_accessible(&v);
    assert!(accessible.contains("pg-app"));
    assert!(accessible.contains(strings::STATUS_RUNNING));
    assert_eq!(DOCK_ROW_HEIGHT, 30.0);
}

#[test]
fn create_dialog_engine_order_matches_the_prototype() {
    // Defect 7: prototype order is PostgreSQL, Valkey, MySQL, MariaDB.
    use dblocal_core::service::{EngineEntry, VersionEntry};
    let entries = vec![
        EngineEntry {
            engine: EngineKey::Mysql,
            available: true,
            unavailable_reason: None,
            versions: vec![VersionEntry {
                version: "8.4.4".to_owned(),
                eol: false,
                installed: true,
                size_bytes: 1,
            }],
        },
        EngineEntry {
            engine: EngineKey::Postgresql,
            available: true,
            unavailable_reason: None,
            versions: vec![VersionEntry {
                version: "17.4".to_owned(),
                eol: false,
                installed: true,
                size_bytes: 1,
            }],
        },
        EngineEntry {
            engine: EngineKey::Mariadb,
            available: true,
            unavailable_reason: None,
            versions: vec![VersionEntry {
                version: "11.4.2".to_owned(),
                eol: false,
                installed: true,
                size_bytes: 1,
            }],
        },
        EngineEntry {
            engine: EngineKey::Valkey,
            available: true,
            unavailable_reason: None,
            versions: vec![VersionEntry {
                version: "8.1.0".to_owned(),
                eol: false,
                installed: true,
                size_bytes: 1,
            }],
        },
    ];
    let ordered: Vec<EngineKey> = create_engine_cards(&entries)
        .into_iter()
        .map(|entry| entry.engine)
        .collect();
    assert_eq!(
        ordered,
        vec![
            EngineKey::Postgresql,
            EngineKey::Valkey,
            EngineKey::Mysql,
            EngineKey::Mariadb,
        ]
    );
}

#[test]
fn create_footer_note_depends_on_whether_a_download_is_needed() {
    // Defect 7: the "download" render must pick a version that is not installed
    // so the footer says it will download and verify SHA-256.
    assert_eq!(create_footer_note(false), strings::UI_CREATE_DOWNLOAD_NOTE);
    assert_eq!(create_footer_note(true), strings::UI_CREATE_READY_NOTE);
}

#[test]
fn the_config_tab_has_its_own_live_strings() {
    // The Config tab is live (`W4-H`): its chrome comes from `strings::UI_CONFIG_*`
    // and its rows from `config_view`, never from the removed placeholder.
    assert!(!strings::UI_CONFIG_CARD_TITLE.is_empty());
    assert!(!strings::UI_CONFIG_APPLY.is_empty());
    assert!(!strings::UI_CONFIG_RESTORE.is_empty());
    assert_eq!(
        strings::config_default("100"),
        format!("{}{}", strings::UI_CONFIG_DEFAULT_PREFIX, "100")
    );
    assert_eq!(strings::config_unit(""), None);
    assert!(strings::config_unit("bytes").is_some());
    // The backup age reads as elapsed time, not a raw epoch.
    assert_eq!(
        strings::config_backup_age(1_000, 1_030),
        strings::UI_CONFIG_AGE_SECONDS.replace("{n}", "30")
    );
    assert_eq!(
        strings::config_backup_age(1_000, 1_000),
        strings::UI_CONFIG_AGE_NOW
    );
}

#[test]
fn abbreviate_home_replaces_prefix() {
    let home = std::path::Path::new("/home/alice");
    // The remainder is joined natively onto `~`, so the expectation is built the
    // same way (`~\.dblocal` on Windows).
    let expected = std::path::Path::new("~")
        .join(".dblocal")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        abbreviate_home(std::path::Path::new("/home/alice/.dblocal"), Some(home)),
        expected
    );
    assert_eq!(
        abbreviate_home(std::path::Path::new("/home/alice"), Some(home)),
        "~"
    );
    assert_eq!(
        abbreviate_home(std::path::Path::new("/var/lib/dblocal"), Some(home)),
        "/var/lib/dblocal"
    );
    assert_eq!(
        abbreviate_home(std::path::Path::new("/var/lib/dblocal"), None),
        "/var/lib/dblocal"
    );
}
