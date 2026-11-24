//! Test doubles: scripted adapter, fake process table, terminator, secrets and
//! a gated probe.
//!
//! Split out of `harness.rs` to keep every file under the 800-line limit.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime};

use crate::engine::{Credential, EngineAdapter, Health, InitContext, RunContext};
use crate::error::Result;
use crate::model::{CredentialBackend, CredentialRef, EngineKey, StartFailure};
use crate::process::identity::{ProcessIdentity, ProcessState};
use crate::process::probe::{ProcessObservation, ProcessProbe};
use crate::process::terminate::TerminateOutcome;
use crate::secret::SecretString;
use crate::secrets::{BackendKind, FakeBackend, SecretBackend};
use crate::service::Terminator;

/// Where a scripted create/start operation should fail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailAt {
    None,
    Initialize,
    Health,
    Shutdown,
}

/// Adapter whose behaviour the test scripts.
#[derive(Debug)]
pub struct ScriptedAdapter {
    engine: EngineKey,
    pub fail_at: Mutex<FailAt>,
    pub health_ready: AtomicBool,
    pub init_calls: AtomicUsize,
    pub health_calls: AtomicUsize,
    pub shutdown_calls: AtomicUsize,
    /// Output the adapter should return from `diagnose_start_failure`.
    pub marker: Mutex<Option<String>>,
}

impl ScriptedAdapter {
    pub fn new(engine: EngineKey) -> Self {
        Self {
            engine,
            fail_at: Mutex::new(FailAt::None),
            health_ready: AtomicBool::new(true),
            init_calls: AtomicUsize::new(0),
            health_calls: AtomicUsize::new(0),
            shutdown_calls: AtomicUsize::new(0),
            marker: Mutex::new(None),
        }
    }

    pub fn failing(engine: EngineKey, at: FailAt) -> Self {
        let adapter = Self::new(engine);
        *adapter.fail_at.lock().unwrap() = at;
        adapter
    }

    fn fail_at(&self) -> FailAt {
        *self.fail_at.lock().unwrap()
    }
}

impl EngineAdapter for ScriptedAdapter {
    fn key(&self) -> EngineKey {
        self.engine
    }

    fn initialize(&self, ctx: &InitContext) -> Result<()> {
        self.init_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_at() == FailAt::Initialize {
            return Err(crate::DblError::Engine {
                reason: "initdb failed on purpose".to_owned(),
            });
        }
        // Really write a data directory so rollback has something to remove.
        std::fs::create_dir_all(&ctx.data_dir)?;
        std::fs::write(ctx.data_dir.join("PG_VERSION"), b"18\n")?;
        Ok(())
    }

    fn spawn_command(&self, _ctx: &RunContext) -> std::process::Command {
        // `true` exists everywhere on Unix and exits instantly; tests never run
        // this for real because they inject a fake spawn.
        std::process::Command::new("true")
    }

    fn health_check(&self, _ctx: &RunContext) -> Result<Health> {
        self.health_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_at() == FailAt::Health {
            return Err(crate::DblError::Engine {
                reason: "health probe failed on purpose".to_owned(),
            });
        }
        if self.health_ready.load(Ordering::SeqCst) {
            Ok(Health::Ready)
        } else {
            Ok(Health::NotReady("starting".to_owned()))
        }
    }

    fn shutdown(&self, _ctx: &RunContext) -> Result<()> {
        self.shutdown_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_at() == FailAt::Shutdown {
            return Err(crate::DblError::Engine {
                reason: "shutdown failed on purpose".to_owned(),
            });
        }
        Ok(())
    }

    fn connection_uri(
        &self,
        ctx: &RunContext,
        credential: &Credential,
        _format: crate::service::UriFormat,
    ) -> SecretString {
        SecretString::new(format!(
            "{}://{}:{}@127.0.0.1:{}/db",
            self.engine.as_str(),
            credential.username,
            credential.password.expose(),
            ctx.port.get()
        ))
    }

    fn diagnose_start_failure(&self, _output: &str, _exit_code: Option<i32>) -> StartFailure {
        match self.marker.lock().unwrap().as_deref() {
            Some("port_in_use") => StartFailure::PortInUse,
            Some("permission_denied") => StartFailure::PermissionDenied,
            Some("data_dir_version_mismatch") => StartFailure::DataDirVersionMismatch,
            Some("binary_missing") => StartFailure::BinaryMissing,
            _ => StartFailure::Unknown,
        }
    }
}

/// A fake process table shared by the probe and the fake terminator.
#[derive(Debug, Default)]
pub struct FakeProcs {
    table: Mutex<HashMap<u32, ProcessObservation>>,
    next_pid: AtomicUsize,
    pub kills: AtomicUsize,
    spawns: AtomicUsize,
}

impl FakeProcs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a live process and returns its identity.
    pub fn spawn(&self, exe: &str) -> ProcessIdentity {
        self.spawns.fetch_add(1, Ordering::SeqCst);
        let pid = u32::try_from(self.next_pid.fetch_add(1, Ordering::SeqCst) + 40_000).unwrap();
        let started_at =
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + u64::from(pid));
        self.table.lock().unwrap().insert(
            pid,
            ProcessObservation::Found {
                exe: Some(PathBuf::from(exe)),
                started_at: Some(started_at),
            },
        );
        ProcessIdentity::new(pid, exe, started_at).unwrap()
    }

    pub fn kill(&self, pid: u32) {
        self.table.lock().unwrap().remove(&pid);
    }

    /// How many spawns have been recorded, for the two-service race test.
    pub fn spawned_count(&self) -> usize {
        self.spawns.load(Ordering::SeqCst)
    }
}

impl ProcessProbe for FakeProcs {
    fn observe(&self, pid: u32) -> ProcessObservation {
        self.table
            .lock()
            .unwrap()
            .get(&pid)
            .cloned()
            .unwrap_or(ProcessObservation::Missing)
    }
}

/// Terminator that removes the process from the fake table.
pub struct FakeTerminator {
    procs: Arc<FakeProcs>,
    /// Report `Forced` instead of `Graceful` when a signal lands.
    pub forced: AtomicBool,
    /// Refuse every attempt with an error and leave the process running, the
    /// way a mismatched identity makes the real process layer refuse.
    pub refuse: AtomicBool,
    /// How many terminate calls were made, for assertions.
    pub calls: AtomicUsize,
    /// How many graceful-exit waits were requested, for assertions.
    pub awaits: AtomicUsize,
}

impl FakeTerminator {
    pub fn new(procs: Arc<FakeProcs>) -> Self {
        Self {
            procs,
            forced: AtomicBool::new(false),
            refuse: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
            awaits: AtomicUsize::new(0),
        }
    }
}

impl Terminator for FakeTerminator {
    fn await_exit(&self, _identity: &ProcessIdentity, _limit: Duration) -> bool {
        self.awaits.fetch_add(1, Ordering::SeqCst);
        false
    }

    fn terminate(&self, identity: &ProcessIdentity, _grace: Duration) -> Result<TerminateOutcome> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.refuse.load(Ordering::SeqCst) {
            // Deliberately do **not** touch the process table: the caller must
            // observe that it is still alive and keep its identity.
            return Err(crate::DblError::Engine {
                reason: "the process refused to stop".to_owned(),
            });
        }
        self.procs.kills.fetch_add(1, Ordering::SeqCst);
        if self.procs.observe(identity.pid()) == ProcessObservation::Missing {
            return Ok(TerminateOutcome::NotRunning);
        }
        self.procs.kill(identity.pid());
        if self.forced.load(Ordering::SeqCst) {
            Ok(TerminateOutcome::Forced)
        } else {
            Ok(TerminateOutcome::Graceful)
        }
    }
}

/// Secret storage backed by in-memory fakes for both backends, so tests never
/// touch the developer keychain.
pub struct FakeSecrets {
    pub keychain: FakeBackend,
    pub file: FakeBackend,
    pub keychain_available: AtomicBool,
    /// When set, `get` fails with a keychain-unavailable error, which is what a
    /// locked Secret Service looks like to a stop.
    pub unreadable: AtomicBool,
}

impl Default for FakeSecrets {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeSecrets {
    pub fn new() -> Self {
        Self {
            keychain: FakeBackend::new(CredentialBackend::Keychain),
            file: FakeBackend::new(CredentialBackend::File),
            keychain_available: AtomicBool::new(false),
            unreadable: AtomicBool::new(false),
        }
    }

    fn backend(&self, kind: BackendKind) -> &FakeBackend {
        match kind {
            CredentialBackend::Keychain => &self.keychain,
            CredentialBackend::File => &self.file,
        }
    }
}

impl super::super::service::SecretAccess for FakeSecrets {
    fn set(&self, reference: &CredentialRef, secret: &SecretString) -> Result<BackendKind> {
        if self.keychain_available.load(Ordering::SeqCst) {
            self.keychain.set(reference, secret)?;
            return Ok(CredentialBackend::Keychain);
        }
        self.file.set(reference, secret)?;
        Ok(CredentialBackend::File)
    }

    fn get(&self, reference: &CredentialRef) -> Result<SecretString> {
        if self.unreadable.load(Ordering::SeqCst) {
            return Err(crate::DblError::Unsupported {
                reason: "the keychain is locked".to_owned(),
            });
        }
        self.backend(reference.backend()).get(reference)
    }

    fn delete(&self, reference: &CredentialRef) -> Result<()> {
        // Both backends, like the real store; absence is not an error.
        for backend in [&self.keychain, &self.file] {
            match backend.delete(reference) {
                Ok(()) | Err(crate::DblError::NotFound { .. }) => {}
                Err(crate::DblError::Unsupported { .. }) => {}
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }
}

/// A probe that lets a test hold the supervisor at the exact point it has an
/// answer but has not acted on it.
///
/// The supervisor's `validate` call signals "reached" and then blocks until the
/// test releases the gate with the answer to return. That makes the
/// supervisor-vs-stop race deterministic: the test runs a complete stop while
/// the supervisor is parked inside `validate`, then answers `Absent`, so the
/// supervisor acts on a decision that is now stale.
#[derive(Debug)]
pub struct GatedProbe {
    inner: Arc<FakeProcs>,
    /// Set when `validate` has been entered, waited on by the test.
    reached: Mutex<bool>,
    reached_cv: Condvar,
    /// Set by the test to release `validate`, carrying the answer to return.
    release: Mutex<Option<ProcessState>>,
    release_cv: Condvar,
    /// When false, `validate` answers from the inner table without gating.
    pub gate_next_validate: AtomicBool,
}

impl GatedProbe {
    pub fn new(inner: Arc<FakeProcs>) -> Self {
        Self {
            inner,
            reached: Mutex::new(false),
            reached_cv: Condvar::new(),
            release: Mutex::new(None),
            release_cv: Condvar::new(),
            gate_next_validate: AtomicBool::new(false),
        }
    }

    /// Blocks until the supervisor is parked inside `validate`, or the timeout
    /// elapses. Returns whether it was reached.
    pub fn wait_reached(&self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        let mut reached = self.reached.lock().unwrap();
        while !*reached {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let (guard, _) = self.reached_cv.wait_timeout(reached, remaining).unwrap();
            reached = guard;
        }
        true
    }

    /// Releases a parked `validate`, which returns `answer`.
    pub fn release_with(&self, answer: ProcessState) {
        *self.release.lock().unwrap() = Some(answer);
        self.release_cv.notify_all();
    }

    /// Arm the gate so the next `validate` parks instead of answering.
    pub fn arm(&self) {
        self.gate_next_validate.store(true, Ordering::SeqCst);
    }

    /// Clears `reached` and any pending release so the gate can be used again.
    ///
    /// A test that observes "did the supervisor reach `validate`?" across two
    /// phases must reset between them, and must release any parked call before
    /// asserting: otherwise a parked task survives into teardown and hangs the
    /// test process instead of failing it.
    pub fn reset(&self) {
        *self.reached.lock().unwrap() = false;
        *self.release.lock().unwrap() = None;
        self.gate_next_validate.store(false, Ordering::SeqCst);
    }
}

impl ProcessProbe for GatedProbe {
    fn observe(&self, pid: u32) -> ProcessObservation {
        self.inner.observe(pid)
    }

    fn validate(&self, identity: &ProcessIdentity) -> ProcessState {
        if !self.gate_next_validate.swap(false, Ordering::SeqCst) {
            return self.inner.validate(identity);
        }
        // Signal the test that we are parked, then wait for its answer.
        *self.reached.lock().unwrap() = true;
        self.reached_cv.notify_all();
        let mut release = self.release.lock().unwrap();
        while release.is_none() {
            release = self.release_cv.wait(release).unwrap();
        }
        release.take().unwrap_or(ProcessState::Unknown)
    }
}
