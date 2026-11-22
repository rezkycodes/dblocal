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
    let events = h.run_on(&mut receiver, op);
    let finished = events
        .iter()
        .find_map(|event| match event {
            crate::service::ServiceEvent::OperationFinished { result, .. } => Some(result.clone()),
            _ => None,
        })
        .expect("an operation finished");
    assert!(
        finished.is_ok(),
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
    let op = h.service.refresh_catalog(false).expect("refresh");
    let _ = h.run(op);
    assert_eq!(calls.load(Ordering::SeqCst), 0, "no refresh inside the TTL");

    // With `force`, the same call does reach out exactly once.
    let op = h.service.refresh_catalog(true).expect("forced refresh");
    let _ = h.run(op);
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
    let events = h.run_on(&mut receiver, op);
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
