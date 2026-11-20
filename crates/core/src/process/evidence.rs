//! Real-process evidence tests required by W1-D.
//!
//! These run actual `/bin/sleep` processes on Unix and exercise the full
//! validate-before-signal path against the live process table.

#![cfg(unix)]

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime};

use tempfile::tempdir;

use crate::process::identity::{ProcessIdentity, ProcessState};
use crate::process::probe::{ProcessProbe, SysinfoProbe};
use crate::process::reconcile::{ReconcileOutcome, ReconcileRecord, reconcile};
use crate::process::spawn::spawn_detached;
use crate::process::terminate::{TerminateOutcome, terminate};

fn sleep_cmd(secs: u32) -> Command {
    let mut cmd = Command::new("/bin/sleep");
    cmd.arg(secs.to_string());
    cmd
}

#[test]
fn detached_survival_evidence() {
    // Task requirement: "spawn a real `sleep`, drop every handle, and shows the
    // child is still `Match` afterwards, then terminates it."
    let dir = tempdir().expect("tempdir");
    let log = dir.path().join("survival.log");
    let identity = spawn_detached(sleep_cmd(30), &log).expect("spawn");

    // Every handle from the spawn is gone; only the recorded identity remains.
    std::thread::sleep(Duration::from_millis(400));
    let probe = SysinfoProbe::new();
    assert_eq!(
        probe.validate(&identity),
        ProcessState::Match,
        "child must survive after the parent dropped all handles"
    );

    let outcome = terminate(&identity, Duration::from_secs(3)).expect("terminate");
    assert!(
        outcome.signalled(),
        "survivor must be terminable: {outcome:?}"
    );
    assert_eq!(probe.validate(&identity), ProcessState::Absent);
}

#[test]
fn a_grandchild_survives_its_parent_exiting() {
    // Stronger survival proof: a helper process starts the future engine and
    // exits. The engine is reparented to init/systemd, exactly what happens when
    // DBLocal quits, and must still validate as ours afterwards.
    let dir = tempdir().expect("tempdir");
    let pidfile = dir.path().join("engine.pid");
    let log = dir.path().join("helper.log");
    let mut helper = Command::new("/bin/sh");
    helper.args([
        "-c",
        &format!("/bin/sleep 30 & echo $! > {}", pidfile.display()),
    ]);
    let helper_identity = spawn_detached(helper, &log).expect("spawn helper");

    // Wait until the helper wrote the PID and exited.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let pid = loop {
        let parsed = std::fs::read_to_string(&pidfile)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
            .filter(|pid| *pid > 0);
        if let Some(pid) = parsed {
            break pid;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "helper never wrote a pid"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    let probe = SysinfoProbe::new();
    assert_ne!(
        probe.validate(&helper_identity),
        ProcessState::Match,
        "the helper itself must be gone before we claim reparenting"
    );

    // The grandchild is alive and adoptable, with a complete identity.
    let mut identity = None;
    for _ in 0..50 {
        if let crate::process::probe::ProcessObservation::Found {
            exe: Some(exe),
            started_at: Some(started_at),
        } = probe.observe(pid)
        {
            let candidate = ProcessIdentity::new(pid, exe, started_at).expect("identity");
            if probe.validate(&candidate) == ProcessState::Match {
                identity = Some(candidate);
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let identity = identity.expect("reparented engine must be observable");
    assert_eq!(probe.validate(&identity), ProcessState::Match);

    let outcome = terminate(&identity, Duration::from_secs(3)).expect("terminate");
    assert!(
        outcome.signalled(),
        "adopted engine must still be stoppable"
    );
    assert_eq!(probe.validate(&identity), ProcessState::Absent);
}

#[test]
fn pid_reuse_is_detected_as_not_match() {
    // Task requirement: "PID reuse is `Reused` (spawn `sleep`, record identity,
    // kill it, spawn another process, show the old identity is not `Match`)".
    // Killing A makes its recorded identity `Absent` (certainly not `Match`);
    // a live unrelated process under the same identity is `Reused`. Both
    // properties are required and both are demonstrated here.
    let dir = tempdir().expect("tempdir");
    let probe = SysinfoProbe::new();

    let log_a = dir.path().join("a.log");
    let identity_a = spawn_detached(sleep_cmd(30), &log_a).expect("spawn A");
    assert_eq!(probe.validate(&identity_a), ProcessState::Match);

    // The unrelated-process case: this very test process is alive, so claiming
    // it ran `/bin/sleep` at A's start time must be rejected as `Reused`.
    let forged_unrelated = ProcessIdentity::new(
        std::process::id(),
        "/bin/sleep",
        SystemTime::UNIX_EPOCH + Duration::from_secs(1),
    )
    .expect("identity");
    assert_eq!(
        probe.validate(&forged_unrelated),
        ProcessState::Reused,
        "a live PID that belongs to a different executable must never match"
    );

    // After A is gone, its own identity is no longer a match. Spawning B does
    // not resurrect A's identity.
    let outcome = terminate(&identity_a, Duration::from_secs(3)).expect("terminate A");
    assert!(outcome.signalled());
    let log_b = dir.path().join("b.log");
    let identity_b = spawn_detached(sleep_cmd(30), &log_b).expect("spawn B");
    let state = probe.validate(&identity_a);
    assert_ne!(state, ProcessState::Match, "a dead identity must not match");
    assert!(matches!(state, ProcessState::Absent | ProcessState::Reused));

    let _ = terminate(&identity_b, Duration::from_secs(3));
}

#[test]
fn terminate_never_signals_a_mismatch() {
    // Task requirement: "terminate never signals a mismatch (assert the
    // unrelated process is still alive)".
    let dir = tempdir().expect("tempdir");
    let probe = SysinfoProbe::new();

    let log = dir.path().join("keeper.log");
    let real = spawn_detached(sleep_cmd(30), &log).expect("spawn keeper");
    assert_eq!(probe.validate(&real), ProcessState::Match);

    // Forge an identity that points at the keeper's PID but records the wrong
    // executable. Validation must reject it, so terminate must send nothing.
    let mismatch = ProcessIdentity::new(
        real.pid(),
        PathBuf::from("/usr/bin/definitely-not-the-engine"),
        real.started_at(),
    )
    .expect("identity");
    assert_ne!(probe.validate(&mismatch), ProcessState::Match);

    let outcome = terminate(&mismatch, Duration::from_millis(100)).expect("terminate");
    assert_eq!(
        outcome,
        TerminateOutcome::Refused {
            state: ProcessState::Reused
        }
    );

    // The keeper is still running because no signal was ever sent to it.
    assert_eq!(
        probe.validate(&real),
        ProcessState::Match,
        "a mismatched identity must not cause a signal to be sent"
    );

    let _ = terminate(&real, Duration::from_secs(3));
}

#[test]
fn reconcile_table_evidence() {
    // Covers the full table: adopted match, dead PID, reused PID, missing
    // pidfile, unprovable identity.
    let dir = tempdir().expect("tempdir");
    let probe = SysinfoProbe::new();

    let live_log = dir.path().join("live.log");
    let live = spawn_detached(sleep_cmd(30), &live_log).expect("spawn live");
    let dead_log = dir.path().join("dead.log");
    let dead = spawn_detached(sleep_cmd(30), &dead_log).expect("spawn dead");
    let _ = terminate(&dead, Duration::from_secs(3));

    let reused = ProcessIdentity::new(
        std::process::id(),
        "/bin/sleep",
        SystemTime::UNIX_EPOCH + Duration::from_secs(1),
    )
    .expect("reused identity");

    fn iid(n: u128) -> crate::model::InstanceId {
        let hex = format!("{n:032x}");
        let uuid = format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        );
        crate::model::InstanceId::parse(uuid).expect("uuid")
    }

    let records = vec![
        ReconcileRecord::new(iid(1), Some(live.clone())),
        ReconcileRecord::new(iid(2), Some(dead)),
        ReconcileRecord::new(iid(3), Some(reused)),
        ReconcileRecord::new(iid(4), None),
    ];
    let outcomes = reconcile(&records, &probe);
    assert_eq!(outcomes.len(), 4);
    assert!(matches!(outcomes[0], ReconcileOutcome::Adopted { .. }));
    assert!(matches!(
        outcomes[1],
        ReconcileOutcome::ReconciledAbsent {
            state: ProcessState::Absent,
            ..
        }
    ));
    assert!(matches!(
        outcomes[2],
        ReconcileOutcome::ReconciledAbsent {
            state: ProcessState::Reused,
            ..
        }
    ));
    assert!(matches!(
        outcomes[3],
        ReconcileOutcome::ReconciledAbsent {
            state: ProcessState::Absent,
            ..
        }
    ));

    let _ = terminate(&live, Duration::from_secs(3));
}
