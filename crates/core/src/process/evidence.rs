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
    // The helper stays alive briefly after writing the pidfile. `spawn_detached`
    // records an identity by observing the live process, and a shell that exits
    // within a couple of milliseconds can legitimately be gone before the first
    // observation — that is a property of the launcher, not of the survival
    // guarantee this test is about.
    helper.args([
        "-c",
        &format!(
            "/bin/sleep 30 >/dev/null 2>&1 & echo $! > {}; sleep 0.3",
            pidfile.display()
        ),
    ]);
    let helper_identity = spawn_detached(helper, &log).expect("spawn helper");

    let probe = SysinfoProbe::new();
    // Wait until the helper has both written the PID and actually exited, so the
    // grandchild is genuinely reparented before we validate it.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let pid = loop {
        let parsed = std::fs::read_to_string(&pidfile)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
            .filter(|pid| *pid > 0);
        let helper_gone = probe.validate(&helper_identity) != ProcessState::Match;
        if let Some(pid) = parsed.filter(|_| helper_gone) {
            break pid;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "helper never wrote a pid and exited"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_ne!(
        probe.validate(&helper_identity),
        ProcessState::Match,
        "the helper itself must be gone before we claim reparenting"
    );

    // The grandchild is alive and adoptable, with a complete identity. Poll to a
    // deadline rather than a fixed attempt count: under load the first observation
    // after the helper exits can still be the pre-exec image or arrive late, and a
    // 50 x 20 ms budget is only ~1 s.
    let identity = wait_for_identity(&probe, pid, "/bin/sleep", Duration::from_secs(10))
        .unwrap_or_else(|| panic!("reparented engine {pid} was never observable as sleep"));
    assert_eq!(probe.validate(&identity), ProcessState::Match);

    let outcome = terminate(&identity, Duration::from_secs(3)).expect("terminate");
    // Report *why* nothing was signalled, not just that it was not: `NotRunning`
    // means the grandchild had already exited, `Refused { state }` means the
    // identity no longer matched (a reuse or an unreadable process), and a `None`
    // outcome here would be impossible because `terminate` returned. The first
    // version of this assertion said only "must still be stoppable", which is why
    // the single whole-lib failure (flake 8 fix 1, F3) could not name its cause.
    assert!(
        outcome.signalled(),
        "adopted engine {pid} must still be stoppable, got {outcome:?} \
         (state now {:?}); identity exe={:?} started_at={:?}",
        probe.validate(&identity),
        identity.exe(),
        identity.started_at(),
    );
    assert_eq!(probe.validate(&identity), ProcessState::Absent);
}

/// Polls the process table until `pid` validates as the identity we would record,
/// **and** is running `program`, or the deadline passes.
///
/// Requiring the executable is what makes the capture sound. The helper starts the
/// grandchild with `&`, so between `fork` and `exec` `/proc/<pid>/exe` still names
/// the shell: recording that identity and then terminating it would see the exe
/// change to `sleep` and correctly refuse it as recycled, failing the test for a
/// reason that has nothing to do with survival. Waiting for `program` (and then
/// validating what we saw) records the post-exec identity, which is the one
/// `terminate` will validate later.
///
/// Split out so the grandchild test can say what it actually observed at the
/// deadline instead of failing on an `expect` with no facts.
fn wait_for_identity(
    probe: &dyn ProcessProbe,
    pid: u32,
    program: &str,
    limit: Duration,
) -> Option<ProcessIdentity> {
    use crate::process::probe::ProcessObservation;
    let expected = std::path::Path::new(program)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(program));
    let deadline = std::time::Instant::now() + limit;
    loop {
        if let ProcessObservation::Found {
            exe: Some(exe),
            started_at: Some(started_at),
        } = probe.observe(pid)
            && exe
                .canonicalize()
                .is_ok_and(|observed| observed == expected)
            && let Ok(candidate) = ProcessIdentity::new(pid, exe, started_at)
            && probe.validate(&candidate) == ProcessState::Match
        {
            return Some(candidate);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
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

/// `wait_for_identity` must not record the pre-exec image of a `&` grandchild.
///
/// The helper starts the grandchild with `&`, so for a moment `/proc/<pid>/exe`
/// still names `/bin/sh`. A capture at that instant yields an identity whose
/// executable changes to `/bin/sleep` on exec, and `terminate` then correctly
/// refuses it as recycled — which is exactly what `Refused { state: Reused }`
/// means, and what failed once in 18 pinned whole-lib runs (`terminate` was
/// right; the recorded identity was wrong). This drives the mechanism with a
/// fake table instead of waiting for the race: the first observations report the
/// pre-exec shell, then the post-exec programme, and only the latter may be
/// returned.
#[test]
fn wait_for_identity_skips_the_pre_exec_shell_image() {
    use crate::process::probe::ProcessObservation;
    use crate::process::probe::fake::FakeProbe;

    let probe = FakeProbe::new();
    let pid = 42_424;
    let started_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    // Pre-exec: the grandchild still carries the shell's image.
    probe.set(
        pid,
        ProcessObservation::Found {
            exe: Some(PathBuf::from("/bin/sh")),
            started_at: Some(started_at),
        },
    );
    assert!(
        wait_for_identity(&probe, pid, "/bin/sleep", Duration::from_millis(150)).is_none(),
        "a process that never execs the programme must not yield an identity"
    );

    // Post-exec: now it is the programme we launched, at the same start time.
    probe.set(
        pid,
        ProcessObservation::Found {
            exe: Some(PathBuf::from("/bin/sleep")),
            started_at: Some(started_at),
        },
    );
    let identity = wait_for_identity(&probe, pid, "/bin/sleep", Duration::from_secs(5))
        .expect("the post-exec programme is adoptable");
    assert_eq!(identity.pid(), pid);
    assert_eq!(identity.started_at(), started_at);
    assert_eq!(probe.validate(&identity), ProcessState::Match);
}
