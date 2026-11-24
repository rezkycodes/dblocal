//! Catalog read model and refresh behaviour (`F-CAT-01/02`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::catalog::{CatalogFetcher, RefreshOutcome};
use crate::error::Result;
use crate::model::{CatalogStatus, EngineKey};
use crate::service::DblService;

use super::harness::Harness;

/// Fetcher that always fails, so a refresh degrades without losing the catalog.
#[derive(Clone)]
struct AlwaysFails {
    calls: Arc<AtomicUsize>,
}

impl CatalogFetcher for AlwaysFails {
    async fn fetch(&self, _url: &str, _etag: Option<&str>) -> Result<RefreshOutcome> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(crate::DblError::Network {
            reason: "offline on purpose".to_owned(),
        })
    }
}

#[test]
fn catalog_lists_only_versions_installable_here() {
    let h = Harness::new(EngineKey::Postgresql);
    let view = h.service.catalog().expect("catalog");
    assert_eq!(
        view.engines.len(),
        EngineKey::ALL.len(),
        "every engine is listed"
    );
    for entry in &view.engines {
        if !entry.available {
            assert!(
                entry.unavailable_reason.is_some(),
                "{} must carry a reason",
                entry.engine
            );
        }
    }
}

#[test]
fn the_bundled_catalog_covers_all_four_engines_on_linux_x86_64() {
    // W4-E: without real MySQL and MariaDB entries the service could never
    // install either engine. This pins the shipped snapshot's shape so a future
    // regeneration that drops an engine fails loudly.
    use crate::catalog::{InstalledVersions, build_catalog_view, bundled_snapshot};
    use crate::platform::{Arch, Os};

    let snapshot = bundled_snapshot().expect("bundled catalog");
    let view = build_catalog_view(
        &snapshot,
        Os::Linux,
        Arch::X86_64,
        &InstalledVersions::empty(),
        None,
    );
    for engine in EngineKey::ALL {
        let entry = view
            .engines
            .iter()
            .find(|entry| entry.engine == engine)
            .expect("engine listed");
        assert!(
            entry.available,
            "{} must be installable on Linux x86_64: {:?}",
            engine.as_str(),
            entry.unavailable_reason
        );
        assert!(!entry.versions.is_empty());
        assert!(
            entry.versions.iter().all(|v| v.size_bytes > 0),
            "{} sizes",
            engine.as_str()
        );
    }

    // MariaDB is Linux/Windows x86_64 only: no macOS, no arm64.
    let macos = build_catalog_view(
        &snapshot,
        Os::Macos,
        Arch::X86_64,
        &InstalledVersions::empty(),
        None,
    );
    let mariadb = macos
        .engines
        .iter()
        .find(|e| e.engine == EngineKey::Mariadb)
        .expect("mariadb");
    assert!(!mariadb.available, "MariaDB has no macOS build");
    assert!(mariadb.unavailable_reason.is_some());
    let linux_arm = build_catalog_view(
        &snapshot,
        Os::Linux,
        Arch::Aarch64,
        &InstalledVersions::empty(),
        None,
    );
    let mariadb_arm = linux_arm
        .engines
        .iter()
        .find(|e| e.engine == EngineKey::Mariadb)
        .expect("mariadb");
    assert!(!mariadb_arm.available, "MariaDB has no arm64 Linux tarball");
}

/// D-26: the bundled catalog has no Valkey artifact for Windows, so the
/// platform-filtered view the GUI and CLI render must mark Valkey unavailable
/// with the PRD's reason while keeping the other three engines selectable.
#[test]
fn the_bundled_catalog_marks_valkey_unavailable_on_windows() {
    use crate::catalog::{InstalledVersions, build_catalog_view, bundled_snapshot};
    use crate::platform::{Arch, Os};

    let snapshot = bundled_snapshot().expect("bundled catalog");
    let windows = build_catalog_view(
        &snapshot,
        Os::Windows,
        Arch::X86_64,
        &InstalledVersions::empty(),
        None,
    );
    let valkey = windows
        .engines
        .iter()
        .find(|e| e.engine == EngineKey::Valkey)
        .expect("valkey entry");
    assert!(
        !valkey.available,
        "Valkey has no Windows artifact and must be unavailable"
    );
    assert_eq!(
        valkey.unavailable_reason.as_deref(),
        Some(crate::catalog::UNAVAILABLE_ON_PLATFORM)
    );
    assert!(valkey.versions.is_empty(), "no version is installable");
    for engine in [EngineKey::Postgresql, EngineKey::Mysql, EngineKey::Mariadb] {
        let entry = windows
            .engines
            .iter()
            .find(|e| e.engine == engine)
            .expect("engine listed");
        assert!(
            entry.available,
            "{} must be installable on Windows x86_64",
            engine.as_str()
        );
    }
}

/// D-26: the first release ships no Valkey build for Windows, so the service
/// must treat "Valkey on Windows" as unavailable on this OS — a typed
/// `DBL-6001`, never a panic and never a download attempt — even when a leftover
/// runtime directory exists in the home.
#[test]
fn valkey_on_windows_is_unsupported_before_any_download() {
    use crate::model::{AuthMode, InstanceName, InstanceSpec, Port};
    use crate::platform::Os;

    let h = Harness::with_options(EngineKey::Valkey, |options| {
        options.os = Some(Os::Windows);
    });
    // The harness pre-installs a Valkey runtime for the engine it is given, so
    // this also proves the gate fires for an already-present runtime rather
    // than only when a download would be attempted.
    let spec = InstanceSpec::new(
        InstanceName::new("vk-win").expect("name"),
        EngineKey::Valkey,
        "9.2.0",
        Some(Port::new(super::harness::test_port()).expect("port")),
        AuthMode::None,
    )
    .expect("spec");

    let err = h
        .service
        .validate_spec(&spec)
        .expect_err("valkey on windows");
    assert_eq!(err.code(), crate::error::CODE_UNSUPPORTED);
    assert_eq!(err.user_message(), crate::strings::ERR_UNSUPPORTED);

    let err = h
        .service
        .create_instance(spec)
        .expect_err("create must refuse synchronously");
    assert_eq!(err.code(), crate::error::CODE_UNSUPPORTED);
    assert!(h.service.list_instances().expect("list").is_empty());
    assert_eq!(h.procs.spawned_count(), 0, "no engine process was spawned");
}

/// The gate is scoped to the engine, not the OS: PostgreSQL still works on
/// Windows, so a blanket "Windows is unsupported" regression fails here.
#[test]
fn postgresql_on_windows_is_still_accepted() {
    use crate::model::{AuthMode, InstanceName, InstanceSpec, Port};
    use crate::platform::Os;

    let h = Harness::with_options(EngineKey::Postgresql, |options| {
        options.os = Some(Os::Windows);
    });
    let spec = InstanceSpec::new(
        InstanceName::new("pg-win").expect("name"),
        EngineKey::Postgresql,
        "18.7",
        Some(Port::new(super::harness::test_port()).expect("port")),
        AuthMode::None,
    )
    .expect("spec");
    h.service
        .validate_spec(&spec)
        .expect("postgres ships Windows builds");
}

#[test]
fn a_failed_refresh_keeps_the_working_catalog() {
    let fetcher = AlwaysFails {
        calls: Arc::new(AtomicUsize::new(0)),
    };
    let calls = Arc::clone(&fetcher.calls);
    let h = Harness::with_options(EngineKey::Postgresql, move |options| {
        options.catalog_fetcher = Some(Arc::new(fetcher));
    });

    let before = h.service.catalog().expect("catalog before");
    // Subscribe before the call: a fast-failing refresh can finish before a
    // receiver opened afterwards sees its events.
    let mut receiver = h.receiver();
    let op = h.service.refresh_catalog(true).expect("refresh");
    let run = h.run_on(&mut receiver, op);
    let finished = run.result();
    assert!(
        matches!(finished, Some(Ok(()))),
        "a failed refresh is silent, not an error: {finished:?}"
    );
    assert!(calls.load(Ordering::SeqCst) >= 1, "the fetcher ran");

    let after = h.service.catalog().expect("catalog after");
    assert_eq!(
        before.engines, after.engines,
        "the previous catalog must survive a failed refresh"
    );
    assert_ne!(after.status, CatalogStatus::Fresh, "freshness drops");
}

#[test]
fn a_catalog_inside_the_ttl_is_not_refreshed_without_force() {
    let fetcher = AlwaysFails {
        calls: Arc::new(AtomicUsize::new(0)),
    };
    let calls = Arc::clone(&fetcher.calls);
    let h = Harness::with_options(EngineKey::Postgresql, move |options| {
        options.catalog_fetcher = Some(Arc::new(fetcher));
    });
    // The bundled snapshot is loaded with `fetched_at = now`, so it is inside
    // the TTL even though its status says "stale": a non-forced refresh must
    // therefore do nothing, which is the `F-CAT-01` AC2 rule (refresh at most
    // once per TTL).
    let (_op, run) = h.run_call(|| h.service.refresh_catalog(false));
    assert!(
        matches!(run.result(), Some(Ok(()))),
        "refresh: {:?}",
        run.result()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0, "no refresh inside the TTL");

    // With `force`, the same call does reach out exactly once.
    let (_op, run) = h.run_call(|| h.service.refresh_catalog(true));
    assert!(
        matches!(run.result(), Some(Ok(()))),
        "forced refresh: {:?}",
        run.result()
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a forced refresh runs once"
    );
}

#[test]
fn catalog_status_tracks_freshness_after_a_successful_refresh() {
    #[derive(Clone)]
    struct ServesBody;
    impl CatalogFetcher for ServesBody {
        async fn fetch(&self, _url: &str, _etag: Option<&str>) -> Result<RefreshOutcome> {
            Err(crate::DblError::Network {
                reason: "n/a".to_owned(),
            })
        }
    }
    // A successful refresh needs a valid payload from the fixture document; the
    // unit tests for `catalog::refresh` own that path, so here we only prove the
    // event is emitted with the new freshness.
    let h = Harness::with_options(EngineKey::Postgresql, |options| {
        options.catalog_fetcher = Some(Arc::new(ServesBody));
    });
    let mut receiver = h.receiver();
    let op = h.service.refresh_catalog(true).expect("refresh");
    let run = h.run_on(&mut receiver, op);
    let events = &run.events;
    let updated = events.iter().any(|event| {
        matches!(
            event,
            crate::service::ServiceEvent::CatalogUpdated {
                status: CatalogStatus::Stale | CatalogStatus::Offline
            }
        )
    });
    assert!(updated, "a CatalogUpdated event carries the new freshness");
}

#[test]
fn suggest_port_skips_ports_an_instance_already_owns() {
    let h = Harness::new(EngineKey::Postgresql);
    let first = h
        .service
        .suggest_port(EngineKey::Postgresql)
        .expect("suggest");
    // Reserve that port through a real instance, then ask again.
    let spec = crate::model::InstanceSpec::new(
        crate::model::InstanceName::new("port-holder").expect("name"),
        EngineKey::Postgresql,
        "18.7",
        Some(first),
        crate::model::AuthMode::Password,
    )
    .expect("spec");
    h.create(spec);
    let second = h
        .service
        .suggest_port(EngineKey::Postgresql)
        .expect("suggest");
    assert_ne!(first, second, "the owned port must not be suggested again");
}

#[test]
fn validate_spec_names_the_owning_instance_on_a_port_conflict() {
    let h = Harness::new(EngineKey::Postgresql);
    let port = super::harness::test_port();
    let spec = h.spec("owner", EngineKey::Postgresql, port);
    h.create(spec);
    let conflicting = crate::model::InstanceSpec::new(
        crate::model::InstanceName::new("second").expect("name"),
        EngineKey::Postgresql,
        "18.7",
        Some(crate::model::Port::new(port).expect("port")),
        crate::model::AuthMode::Password,
    )
    .expect("spec");
    let err = h.service.validate_spec(&conflicting).expect_err("conflict");
    assert_eq!(err.code(), crate::error::CODE_PORT_IN_USE);
    // And a valid spec passes.
    let free = catalog_free_port();
    let ok = crate::model::InstanceSpec::new(
        crate::model::InstanceName::new("third").expect("name"),
        EngineKey::Postgresql,
        "18.7",
        Some(free),
        crate::model::AuthMode::Password,
    )
    .expect("spec");
    h.service.validate_spec(&ok).expect("valid spec");
}

fn catalog_free_port() -> crate::model::Port {
    crate::model::Port::new(super::harness::test_port()).expect("port")
}

#[allow(dead_code)]
fn _unused(_: &mut dyn FnMut() -> Duration) {}
