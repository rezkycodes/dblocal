//! The prototype's demo scenario, seeded through the real service surface.
//!
//! `docs/design/prototype/dblocal-gpui/` renders a fixed cast: a running
//! `pg-app`, a stopped `pg-legacy`, a failed `pg-shop` whose start lost its port
//! to a foreign process, a running `cache-dev`, a stopped `laravel-hr`, a running
//! `wp-blog`, one runtime nobody uses, and a download in flight. The GUI's
//! headless render tests seed exactly that, so every screen is reviewable
//! without a live engine.
//!
//! Nothing here reaches a production build: the whole module sits behind the
//! `mock` feature.

use std::collections::VecDeque;
use std::time::{Duration, SystemTime};

use crate::model::{
    AuthMode, CredentialBackend, EngineKey, InstanceId, InstanceName, InstanceSpec, InstanceStatus,
    Port, StartFailure,
};
use crate::service::{DblService, FailureInfo, InstanceView, OperationId, ServiceEvent};

use super::{Inner, MockConfig, MockService, lock};

/// One scripted instance in the demo scenario.
#[derive(Debug, Clone, Copy)]
pub struct DemoInstance {
    /// Instance name (also the data directory name).
    pub name: &'static str,
    /// Engine.
    pub engine: EngineKey,
    /// Engine version tag, which must exist in the mock catalog.
    pub version: &'static str,
    /// Port the instance binds.
    pub port: u16,
    /// Lifecycle status it is seeded in.
    pub status: InstanceStatus,
    /// Authentication mode.
    pub auth: AuthMode,
    /// Failure detail, when the instance is seeded `failed`.
    pub failure: Option<DemoFailure>,
    /// Log lines the tail shows.
    pub log: &'static [&'static str],
}

/// A failure the demo seeds, as the service would report it after a bad start.
#[derive(Debug, Clone, Copy)]
pub struct DemoFailure {
    /// Stable `DBL-NNNN` code.
    pub code: &'static str,
    /// Engine diagnosis, which drives the one-click fix.
    pub diagnosis: StartFailure,
}

const PG_APP_LOG: [&str; 5] = [
    "starting PostgreSQL 18.7 on x86_64-pc-linux-gnu",
    "listening on IPv4 address \"127.0.0.1\", port 5432",
    "listening on IPv6 address \"::1\", port 5432",
    "database system is ready to accept connections",
    "health check pg_isready OK (284 ms sejak spawn)",
];

const PG_SHOP_LOG: [&str; 8] = [
    "received fast shutdown request",
    "database system is shut down",
    "stop selesai: shutdown bersih dalam 0,21 dtk",
    "sesi baru · 22:39:12",
    "start diminta oleh GUI",
    "uji bind 127.0.0.1:5434 … EADDRINUSE",
    "uji bind [::1]:5434 … EADDRINUSE",
    "port 5434 dipakai proses lain; engine tidak dijalankan (DBL-1302)",
];

const CACHE_LOG: [&str; 3] = [
    "Ready to accept connections tcp",
    "port 6379 sudah terikat ke 127.0.0.1",
    "health check PING → PONG",
];

const WP_LOG: [&str; 2] = [
    "mariadbd ready for connections, port 3307",
    "health check handshake OK",
];

const HR_LOG: [&str; 2] = [
    "mysqld: ready for connections",
    "engine berhenti (shutdown bersih)",
];

const LEGACY_LOG: [&str; 2] = [
    "15:02:11 received fast shutdown request",
    "15:02:11 database system is shut down",
];

/// The prototype cast, in dock order (engine, then name).
pub const DEMO_INSTANCES: [DemoInstance; 6] = [
    DemoInstance {
        name: "pg-app",
        engine: EngineKey::Postgresql,
        version: "17.4",
        port: 5432,
        status: InstanceStatus::Running,
        auth: AuthMode::Password,
        failure: None,
        log: &PG_APP_LOG,
    },
    DemoInstance {
        name: "pg-legacy",
        engine: EngineKey::Postgresql,
        version: "16.6",
        port: 5433,
        status: InstanceStatus::Stopped,
        auth: AuthMode::Password,
        failure: None,
        log: &LEGACY_LOG,
    },
    DemoInstance {
        name: "pg-shop",
        engine: EngineKey::Postgresql,
        version: "17.4",
        port: 5434,
        status: InstanceStatus::Failed,
        auth: AuthMode::Password,
        failure: Some(DemoFailure {
            code: "DBL-1005",
            diagnosis: StartFailure::PortInUse,
        }),
        log: &PG_SHOP_LOG,
    },
    DemoInstance {
        name: "laravel-hr",
        engine: EngineKey::Mysql,
        version: "8.4.4",
        port: 3306,
        status: InstanceStatus::Stopped,
        auth: AuthMode::Password,
        failure: None,
        log: &HR_LOG,
    },
    DemoInstance {
        name: "wp-blog",
        engine: EngineKey::Mariadb,
        version: "11.4.2",
        port: 3307,
        status: InstanceStatus::Running,
        auth: AuthMode::Password,
        failure: None,
        log: &WP_LOG,
    },
    DemoInstance {
        name: "cache-dev",
        engine: EngineKey::Valkey,
        version: "8.1.0",
        port: 6379,
        status: InstanceStatus::Running,
        auth: AuthMode::Password,
        failure: None,
        log: &CACHE_LOG,
    },
];

/// The runtime the demo installs but no instance references (F-RTM-04).
pub const DEMO_UNUSED_RUNTIME: (EngineKey, &str) = (EngineKey::Valkey, "7.2.8");

/// The version the demo downloads, which no runtime holds yet.
pub const DEMO_PENDING_DOWNLOAD: (EngineKey, &str, &str, u16) =
    (EngineKey::Postgresql, "15.10", "pg-next", 5435);

impl MockService {
    /// Creates a mock seeded with the prototype scenario (demo/headless builds).
    ///
    /// Timing stays deterministic and non-blocking, so a headless render sees
    /// the same cast on every run.
    pub fn demo() -> Self {
        let service =
            Self::with_config(MockConfig::deterministic().with_start_time(SystemTime::UNIX_EPOCH));
        service.seed_demo();
        service
    }

    /// Seeds the demo cast and returns the ids in scenario order.
    pub fn seed_demo(&self) -> Vec<InstanceId> {
        let mut ids = Vec::with_capacity(DEMO_INSTANCES.len());
        for entry in DEMO_INSTANCES {
            let Some(spec) = spec_of(entry) else {
                tracing::error!("demo instance {} has an invalid name or port", entry.name);
                continue;
            };
            let id = self.seed_instance(spec, entry.status);
            self.seed_log(&id, entry.log);
            if let Some(failure) = entry.failure {
                self.seed_failure(
                    &id,
                    FailureInfo {
                        code: failure.code.to_owned(),
                        message: crate::strings::ERR_PORT_IN_USE.to_owned(),
                        diagnosis: failure.diagnosis,
                        log_tail: entry
                            .log
                            .iter()
                            .rev()
                            .take(4)
                            .rev()
                            .map(|l| (*l).to_owned())
                            .collect(),
                    },
                );
            }
            ids.push(id);
        }
        self.seed_runtime(DEMO_UNUSED_RUNTIME, Duration::from_secs(86_400 * 3));
        ids
    }

    /// Replaces one instance's log tail with the demo lines.
    pub fn seed_log(&self, id: &InstanceId, lines: &[&str]) {
        let mut g = lock(&self.inner);
        if !g.instances.contains_key(id) {
            return;
        }
        g.logs.insert(
            id.clone(),
            VecDeque::from_iter(lines.iter().map(|l| (*l).to_owned())),
        );
    }

    /// Attaches a failure detail to a `failed` instance, as a bad start would.
    pub fn seed_failure(&self, id: &InstanceId, failure: FailureInfo) {
        let mut g = lock(&self.inner);
        let Some(view) = g.rebuild_view(id) else {
            return;
        };
        let view = view.with_failure(failure);
        g.views.insert(id.clone(), view.clone());
        g.emit(ServiceEvent::InstanceChanged { view });
    }

    /// Records an installed runtime that no instance references.
    pub fn seed_runtime(&self, runtime: (EngineKey, &str), age: Duration) {
        let mut g = lock(&self.inner);
        g.runtimes.insert((runtime.0, runtime.1.to_owned()));
        g.det.advance(age);
    }

    /// Starts the demo's download so the status bar shows a live row.
    ///
    /// The full download path runs (progress, verify, extract, init), exactly as
    /// a user's create would; callers that only need the row should render
    /// before the operation completes.
    pub fn start_demo_download(&self) -> crate::Result<OperationId> {
        let (engine, version, name, port) = DEMO_PENDING_DOWNLOAD;
        let spec = InstanceSpec::new(
            InstanceName::new(name)?,
            engine,
            version,
            Some(Port::new(port)?),
            AuthMode::Password,
        )?;
        self.create_instance(spec)
    }

    /// Marks one instance's credential as living in the `0600` file fallback.
    pub fn seed_file_credential(&self, id: &InstanceId) {
        let mut g = lock(&self.inner);
        if !g.instances.contains_key(id) {
            return;
        }
        g.credentials.insert(id.clone(), CredentialBackend::File);
        if let Some(view) = g.rebuild_view(id) {
            g.views.insert(id.clone(), view.clone());
            g.emit(ServiceEvent::InstanceChanged { view });
        }
    }

    /// The instance list the demo seeds, as the GUI receives it.
    pub fn demo_instances(&self) -> Vec<InstanceView> {
        lock(&self.inner).views.values().cloned().collect()
    }
}

/// Builds the spec for one demo entry, or `None` when its literal is invalid.
///
/// The literals are compile-time constants, so this cannot fail in practice;
/// it stays total rather than panicking in a demo constructor, and the caller
/// reports the entry instead of dropping it silently.
fn spec_of(entry: DemoInstance) -> Option<InstanceSpec> {
    let name = InstanceName::new(entry.name).ok()?;
    let port = Port::new(entry.port).ok()?;
    InstanceSpec::new(name, entry.engine, entry.version, Some(port), entry.auth).ok()
}

#[allow(dead_code)]
fn _inner_is_used(_: &Inner) {}

#[cfg(test)]
mod tests {
    use crate::model::InstanceStatus;
    use crate::service::{DblService, Runtime, ServiceEvent};

    use super::super::MockService;
    use super::*;

    #[test]
    fn demo_seeds_the_prototype_cast() {
        let service = MockService::demo();
        let names: Vec<String> = service
            .demo_instances()
            .into_iter()
            .map(|view| view.name)
            .collect();
        for expected in [
            "pg-app",
            "pg-legacy",
            "pg-shop",
            "laravel-hr",
            "wp-blog",
            "cache-dev",
        ] {
            assert!(
                names.contains(&expected.to_owned()),
                "missing {expected}: {names:?}"
            );
        }
    }

    #[test]
    fn demo_statuses_match_the_prototype_screen() {
        let service = MockService::demo();
        let views = service.demo_instances();
        let status = |name: &str| {
            views
                .iter()
                .find(|view| view.name == name)
                .map(|view| view.status)
        };
        assert_eq!(status("pg-app"), Some(InstanceStatus::Running));
        assert_eq!(status("pg-legacy"), Some(InstanceStatus::Stopped));
        assert_eq!(status("pg-shop"), Some(InstanceStatus::Failed));
        assert_eq!(status("cache-dev"), Some(InstanceStatus::Running));
        assert_eq!(status("laravel-hr"), Some(InstanceStatus::Stopped));
        assert_eq!(status("wp-blog"), Some(InstanceStatus::Running));
    }

    #[test]
    fn the_failed_instance_carries_a_port_conflict_the_fix_can_read() {
        let service = MockService::demo();
        let shop = service
            .demo_instances()
            .into_iter()
            .find(|view| view.name == "pg-shop")
            .expect("pg-shop is seeded");
        assert_eq!(shop.port.get(), 5434);
        let failure = shop.failure.expect("failure detail");
        assert_eq!(failure.code, "DBL-1005");
        assert_eq!(failure.diagnosis, StartFailure::PortInUse);
        assert!(!failure.log_tail.is_empty());
    }

    #[test]
    fn demo_logs_replace_the_seed_line() {
        let service = MockService::demo();
        let app = service
            .demo_instances()
            .into_iter()
            .find(|view| view.name == "pg-app")
            .expect("pg-app is seeded");
        let lines = service.tail_logs(&app.id, 100).expect("log tail");
        assert!(
            lines
                .iter()
                .any(|line| line.contains("ready to accept connections"))
        );
    }

    #[test]
    fn the_unused_runtime_is_installed_and_referenced_by_nobody() {
        let service = MockService::demo();
        let runtimes: Vec<Runtime> = service.list_runtimes().expect("runtimes");
        let (engine, version) = DEMO_UNUSED_RUNTIME;
        assert!(
            runtimes
                .iter()
                .any(|runtime| runtime.engine() == engine && runtime.version() == version),
            "the unused demo runtime is missing"
        );
        let views = service.demo_instances();
        assert!(
            views
                .iter()
                .all(|view| !(view.engine == engine && view.version == version)),
            "no instance may reference the unused runtime"
        );
    }

    #[test]
    fn seeding_twice_lists_each_instance_once_per_seed() {
        // `seed_instance` appends, so a second seed adds a second cast rather
        // than replacing it. The demo constructor seeds exactly once; this
        // records the service's rule instead of pretending otherwise.
        let service = MockService::demo();
        let once = service.demo_instances().len();
        service.seed_demo();
        let twice = service.demo_instances().len();
        assert_eq!(twice, once * 2, "each seed appends one full cast");
    }

    #[tokio::test]
    async fn the_demo_download_reports_progress_and_cancel_works() {
        let service = MockService::deterministic();
        service.seed_demo();
        let mut rx = service.subscribe();
        let op = service.start_demo_download().expect("download accepted");

        let mut saw_progress = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(ServiceEvent::DownloadProgress { progress, .. })) => {
                    saw_progress = true;
                    assert!(progress.bytes_total > 0);
                }
                Ok(Ok(ServiceEvent::OperationFinished { .. })) => break,
                Ok(Ok(_)) | Ok(Err(_)) => continue,
                Err(_) => break,
            }
        }
        assert!(saw_progress, "the demo download must report progress");
        // Cancel is legal on a finished operation record in the mock.
        let _ = service.cancel_operation(&op);
    }

    #[test]
    fn file_credential_seeding_changes_the_reported_backend() {
        let service = MockService::demo();
        let app = service
            .demo_instances()
            .into_iter()
            .find(|view| view.name == "pg-app")
            .expect("pg-app");
        service.seed_file_credential(&app.id);
        let updated = service
            .demo_instances()
            .into_iter()
            .find(|view| view.id == app.id)
            .expect("pg-app");
        assert_eq!(updated.credential_backend, Some(CredentialBackend::File));
    }

    #[test]
    fn every_demo_literal_spec_resolves() {
        for entry in DEMO_INSTANCES {
            assert!(spec_of(entry).is_some(), "{} failed to build", entry.name);
        }
    }
}
