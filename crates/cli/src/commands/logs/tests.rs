//! Tests for the log follower: the pure diff, and the command driven by a
//! service that scripts one snapshot per read.

use std::sync::atomic::{AtomicUsize, Ordering};

use dblocal_core::model::{
    AuthMode, DeleteMode, EngineKey, InstanceId, InstanceName, InstanceSpec, InstanceStatus, Port,
    Runtime,
};
use dblocal_core::service::mock::MockService;
use dblocal_core::service::{
    CatalogView, DblService, InstanceView, OperationId, ServiceEvent, UriFormat,
};

use super::{Follower, Poll, new_lines, overlap};

fn lines(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

fn numbered(prefix: &str, from: usize, to: usize) -> Vec<String> {
    (from..=to).map(|n| format!("{prefix}{n}")).collect()
}

#[test]
fn overlap_finds_the_shared_boundary() {
    let old = lines(&["a", "b", "c"]);
    assert_eq!(overlap(&old, &lines(&["b", "c", "d"])), 2);
    assert_eq!(overlap(&old, &lines(&["a", "b", "c"])), 3);
    assert_eq!(overlap(&old, &lines(&["x"])), 0);
    assert_eq!(overlap(&[], &lines(&["x"])), 0);
    assert_eq!(overlap(&old, &[]), 0);
}

#[test]
fn overlap_prefers_the_longest_match() {
    // A repeated line must not make an earlier window look like the boundary.
    let old = lines(&["x", "a", "x"]);
    assert_eq!(overlap(&old, &lines(&["x", "y"])), 1);
}

/// (a) A 20-line log with `--lines 2`: the history is exactly the last 2 lines,
/// and a tick that sees no change prints nothing.
#[test]
fn first_snapshot_prints_only_the_requested_history() {
    let snapshot = numbered("l", 1, 20);
    let follower = Follower::new(snapshot.clone());
    assert_eq!(follower.history(2), &snapshot[18..]);
    assert_eq!(new_lines(&snapshot, &snapshot), &[] as &[String]);
}

/// (b) One line appended: the printed history (2) plus the one new line is
/// exactly 3 lines.
#[test]
fn one_appended_line_is_exactly_one_new_line() {
    let previous = numbered("l", 1, 20);
    let mut next = previous.clone();
    next.push("l21".to_owned());
    let fresh = new_lines(&previous, &next);
    assert_eq!(fresh, &["l21".to_owned()]);
    let history = Follower::new(previous).history(2).to_vec();
    assert_eq!(history.len() + fresh.len(), 3);
}

/// (c) The window rotated by an append: only the appended lines are new.
#[test]
fn a_full_window_that_rotated_reports_only_the_append() {
    let window = crate::consts::LOG_FOLLOW_WINDOW;
    let previous = numbered("l", 1, window);
    // Five more lines landed; the service can only show the last `window`.
    let next = numbered("l", 6, window + 5);
    assert_eq!(next.len(), window);
    assert_eq!(
        new_lines(&previous, &next),
        numbered("l", window + 1, window + 5)
    );
}

/// (d) Truncation or rotation replaced the file: the new snapshot shares no line
/// with the old one and is printed once, not swallowed.
#[test]
fn a_rotated_file_is_printed_once() {
    let previous = lines(&["old-1", "old-2"]);
    let next = lines(&["new-1", "new-2"]);
    assert_eq!(new_lines(&previous, &next), next.as_slice());
}

/// (e) Repeated identical lines are not swallowed while the window has room.
#[test]
fn duplicate_lines_are_not_swallowed_below_the_window() {
    let previous = lines(&["x", "x", "x"]);
    let next = lines(&["x", "x", "x", "x"]);
    assert_eq!(new_lines(&previous, &next), &["x".to_owned()]);
}

/// (e, limit) When the *whole* window is one repeated text and another copy is
/// appended, the service's snapshot is byte-identical to the old one, so the
/// copy is output as if it were part of the history. `cli.md` records the
/// limitation: without timestamps or offsets there is nothing left to compare.
#[test]
fn duplicates_filling_the_whole_window_are_indistinguishable() {
    let window = crate::consts::LOG_FOLLOW_WINDOW;
    let previous = vec!["x".to_owned(); window];
    let next = previous.clone();
    assert_eq!(new_lines(&previous, &next), &[] as &[String]);
}

#[test]
fn history_never_reports_more_than_it_holds() {
    let follower = Follower::new(lines(&["a"]));
    assert_eq!(follower.history(10), &["a".to_owned()]);
    assert!(follower.history(0).is_empty());
}

// --- the command path, driven by scripted snapshots -------------------------

/// A service whose log reads follow a script: one entry per call. A missing
/// entry answers `NotFound` (instance gone). Every other method is the mock's.
struct ScriptedLogs {
    inner: MockService,
    script: Vec<Option<Vec<String>>>,
    calls: AtomicUsize,
    /// Calls (1-based) that fail transiently instead of returning their entry.
    retry_at: Vec<usize>,
}

impl ScriptedLogs {
    fn new(inner: MockService, script: Vec<Option<Vec<String>>>) -> Self {
        Self {
            inner,
            script,
            calls: AtomicUsize::new(0),
            retry_at: Vec::new(),
        }
    }

    fn with_retry(mut self, at: usize) -> Self {
        self.retry_at.push(at);
        self
    }
}

impl DblService for ScriptedLogs {
    fn tail_logs(&self, _id: &InstanceId, _lines: usize) -> dblocal_core::Result<Vec<String>> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.retry_at.contains(&call) {
            return Err(dblocal_core::DblError::Storage {
                reason: "transient read".to_owned(),
            });
        }
        match self.script.get(call - 1) {
            Some(Some(snapshot)) => Ok(snapshot.clone()),
            _ => Err(dblocal_core::DblError::NotFound {
                kind: "instance",
                name: "gone".to_owned(),
            }),
        }
    }
    fn list_instances(&self) -> dblocal_core::Result<Vec<InstanceView>> {
        self.inner.list_instances()
    }
    fn catalog(&self) -> dblocal_core::Result<CatalogView> {
        self.inner.catalog()
    }
    fn refresh_catalog(&self, force: bool) -> dblocal_core::Result<OperationId> {
        self.inner.refresh_catalog(force)
    }
    fn validate_spec(&self, spec: &InstanceSpec) -> dblocal_core::Result<()> {
        self.inner.validate_spec(spec)
    }
    fn suggest_port(&self, engine: EngineKey) -> dblocal_core::Result<Port> {
        self.inner.suggest_port(engine)
    }
    fn create_instance(&self, spec: InstanceSpec) -> dblocal_core::Result<OperationId> {
        self.inner.create_instance(spec)
    }
    fn start_instance(&self, id: &InstanceId) -> dblocal_core::Result<OperationId> {
        self.inner.start_instance(id)
    }
    fn stop_instance(&self, id: &InstanceId) -> dblocal_core::Result<OperationId> {
        self.inner.stop_instance(id)
    }
    fn delete_instance(
        &self,
        id: &InstanceId,
        mode: DeleteMode,
    ) -> dblocal_core::Result<OperationId> {
        self.inner.delete_instance(id, mode)
    }
    fn change_port(&self, id: &InstanceId, port: Port) -> dblocal_core::Result<()> {
        self.inner.change_port(id, port)
    }
    fn data_size(&self, id: &InstanceId) -> dblocal_core::Result<u64> {
        self.inner.data_size(id)
    }
    fn set_autostart(&self, id: &InstanceId, enabled: bool) -> dblocal_core::Result<()> {
        self.inner.set_autostart(id, enabled)
    }
    fn cancel_operation(&self, id: &OperationId) -> dblocal_core::Result<()> {
        self.inner.cancel_operation(id)
    }
    fn connection_uri(
        &self,
        id: &InstanceId,
        format: UriFormat,
    ) -> dblocal_core::Result<dblocal_core::secret::SecretString> {
        self.inner.connection_uri(id, format)
    }
    fn watch_logs(&self, id: &InstanceId, enabled: bool) -> dblocal_core::Result<()> {
        self.inner.watch_logs(id, enabled)
    }
    fn list_runtimes(&self) -> dblocal_core::Result<Vec<Runtime>> {
        self.inner.list_runtimes()
    }
    fn remove_runtime(&self, runtime: &str) -> dblocal_core::Result<()> {
        self.inner.remove_runtime(runtime)
    }
    fn list_archives(&self) -> dblocal_core::Result<Vec<dblocal_core::model::ArchivedData>> {
        self.inner.list_archives()
    }
    fn purge_archive(&self, archive: &str) -> dblocal_core::Result<()> {
        self.inner.purge_archive(archive)
    }
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<ServiceEvent> {
        self.inner.subscribe()
    }
}

fn seed(service: &MockService) -> InstanceId {
    let spec = InstanceSpec::new(
        InstanceName::new("scripts").expect("name"),
        EngineKey::Valkey,
        "9.2.0",
        Some(Port::new(24_000).expect("port")),
        AuthMode::Password,
    )
    .expect("spec");
    service.seed_instance(spec, InstanceStatus::Running)
}

/// Runs the follow command to completion. The script ends with a missing
/// snapshot, which is what stops the loop without a signal.
fn follow(service: &ScriptedLogs, lines: usize) -> (u8, Vec<String>) {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let no_confirm = &|| None;
    let mut ctx = crate::commands::Ctx {
        service,
        out: &mut out,
        err: &mut err,
        is_interactive: false,
        stdout_is_tty: false,
        stderr_is_tty: false,
        confirm: no_confirm,
    };
    let code = crate::commands::logs::run(&mut ctx, "scripts", true, lines);
    let text = String::from_utf8_lossy(&out).to_string();
    let printed = text.lines().map(str::to_owned).collect();
    (code, printed)
}

#[test]
fn follow_prints_the_history_plus_one_append_and_nothing_else() {
    let inner = MockService::deterministic();
    seed(&inner);
    let twenty = numbered("l", 1, 20);
    let mut twenty_one = twenty.clone();
    twenty_one.push("l21".to_owned());
    let service = ScriptedLogs::new(
        inner,
        vec![
            Some(twenty.clone()),
            Some(twenty.clone()),
            Some(twenty_one),
            None,
        ],
    );
    let (code, printed) = follow(&service, 2);
    assert_eq!(code, 0);
    assert_eq!(
        printed,
        vec!["l19".to_owned(), "l20".to_owned(), "l21".to_owned()]
    );
}

#[test]
fn follow_keeps_the_snapshot_across_a_transient_read_error() {
    let inner = MockService::deterministic();
    seed(&inner);
    let twenty = numbered("l", 1, 20);
    let mut twenty_one = twenty.clone();
    twenty_one.push("l21".to_owned());
    // Call 1 is the history snapshot, call 2 fails transiently, call 3 is the
    // same snapshot with the append, call 4 is the instance disappearing.
    let service = ScriptedLogs::new(
        inner,
        vec![Some(twenty.clone()), Some(twenty), Some(twenty_one), None],
    )
    .with_retry(2);
    let (code, printed) = follow(&service, 2);
    assert_eq!(code, 0, "a transient read error must not end the follow");
    // The failure is not printed as history and does not reset the diff.
    assert_eq!(
        printed,
        vec!["l19".to_owned(), "l20".to_owned(), "l21".to_owned()]
    );
}

#[test]
fn poll_reports_gone_for_a_missing_instance() {
    let inner = MockService::deterministic();
    let id = seed(&inner);
    let service = ScriptedLogs::new(inner, vec![None]);
    let mut follower = Follower::new(lines(&["a"]));
    assert_eq!(follower.poll(&service, &id), Poll::Gone);
}

#[test]
fn poll_reports_retry_and_keeps_the_snapshot() {
    let inner = MockService::deterministic();
    let id = seed(&inner);
    let service = ScriptedLogs::new(
        inner,
        vec![Some(lines(&["a", "b"])), Some(lines(&["a", "b"]))],
    )
    .with_retry(1);
    let mut follower = Follower::new(lines(&["a"]));
    assert_eq!(follower.poll(&service, &id), Poll::Retry);
    // The snapshot was not replaced by an empty one: the next read still knows
    // that "a" is old, so only "b" is reported.
    assert_eq!(follower.poll(&service, &id), Poll::Lines(lines(&["b"])));
}
