//! What we record about a process we started, and how to prove it is still ours.
//!
//! PRD `F-INS-04` and invariant 7: a PID alone proves nothing. Before any
//! signal we require the recorded executable **and** the recorded start time to
//! still belong to the live process. W0-B C-08 measured that `pg_ctl status`
//! happily calls a live unrelated `sleep` "running" when handed a stale
//! pidfile, so this check lives here and every caller goes through it.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::DblError;
use crate::error::Result;
use crate::process::probe::ProcessObservation;

/// How far the observed start time may differ from the recorded one.
///
/// Wall clocks are second-granular on most platforms and the system clock can
/// step backwards. PRD `F-INS` edge cases allow ±2 s and say that anything
/// doubtful is treated as stale, which is what [`ProcessState::Reused`] leads to.
pub const START_TOLERANCE: Duration = Duration::from_millis(2_000);

/// Everything DBLocal remembers about an engine process it spawned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessIdentity {
    pid: u32,
    exe: PathBuf,
    started_at: SystemTime,
}

impl ProcessIdentity {
    /// Records an identity read from a pidfile or returned by
    /// [`crate::process::spawn_detached`].
    ///
    /// Rejects the two values that can never identify a real process, so a
    /// malformed pidfile fails loudly at the boundary instead of producing a
    /// signal aimed at nothing.
    pub fn new(pid: u32, exe: impl Into<PathBuf>, started_at: SystemTime) -> Result<Self> {
        let exe = exe.into();
        if pid == 0 {
            return Err(DblError::InvalidInput {
                field: "pid",
                reason: "must not be zero".to_owned(),
            });
        }
        if exe.as_os_str().is_empty() {
            return Err(DblError::InvalidInput {
                field: "exe",
                reason: "must not be empty".to_owned(),
            });
        }
        Ok(Self {
            pid,
            exe,
            started_at,
        })
    }

    /// Process identifier.
    pub const fn pid(&self) -> u32 {
        self.pid
    }

    /// Absolute path of the executable at the time it was recorded.
    pub fn exe(&self) -> &Path {
        &self.exe
    }

    /// Wall clock time the process started.
    pub const fn started_at(&self) -> SystemTime {
        self.started_at
    }
}

/// Verdict of comparing a recorded identity with the live process table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ProcessState {
    /// The PID is alive and is the very process we recorded.
    Match,
    /// Nothing runs under that PID any more.
    Absent,
    /// Something runs under that PID, but it is not our process.
    Reused,
    /// Something runs under that PID and we could not prove what it is.
    ///
    /// Permissions (a process owned by another user) or a platform without
    /// `/proc` produce this. Treated like [`Self::Reused`] for signalling:
    /// nothing is ever sent.
    Unknown,
}

impl ProcessState {
    /// Stable identifier for logs and tests.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Match => "match",
            Self::Absent => "absent",
            Self::Reused => "reused",
            Self::Unknown => "unknown",
        }
    }

    /// Whether this state permits sending a signal.
    pub const fn is_match(self) -> bool {
        matches!(self, Self::Match)
    }
}

/// Compares a recorded identity with one observation from a [`ProcessProbe`].
///
/// [`ProcessProbe`]: crate::process::probe::ProcessProbe
pub fn classify(identity: &ProcessIdentity, observed: &ProcessObservation) -> ProcessState {
    let (exe, started_at) = match observed {
        ProcessObservation::Missing => return ProcessState::Absent,
        ProcessObservation::Found { exe, started_at } => (exe, started_at),
    };
    let (Some(exe), Some(started_at)) = (exe.as_deref(), *started_at) else {
        return ProcessState::Unknown;
    };
    if !same_executable(identity.exe(), exe) {
        return ProcessState::Reused;
    }
    if !within_tolerance(identity.started_at(), started_at, START_TOLERANCE) {
        return ProcessState::Reused;
    }
    ProcessState::Match
}

fn within_tolerance(a: SystemTime, b: SystemTime, tolerance: Duration) -> bool {
    match a.duration_since(b) {
        Ok(delta) => delta <= tolerance,
        Err(err) => err.duration() <= tolerance,
    }
}

fn same_executable(recorded: &Path, observed: &Path) -> bool {
    let recorded = strip_deleted(recorded);
    let observed = strip_deleted(observed);
    if recorded == observed {
        return true;
    }
    match (recorded.canonicalize(), observed.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Linux appends ` (deleted)` to `/proc/<pid>/exe` once the binary was replaced
/// or unlinked while the process kept running. Reinstalling a runtime in place
/// must not turn a live engine into `Reused`.
fn strip_deleted(path: &Path) -> Cow<'_, Path> {
    #[cfg(unix)]
    {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        const SUFFIX: &[u8] = b" (deleted)";
        let bytes = path.as_os_str().as_bytes();
        if let Some(stripped) = bytes.strip_suffix(SUFFIX) {
            return Cow::Owned(PathBuf::from(OsStr::from_bytes(stripped)));
        }
    }
    Cow::Borrowed(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn identity() -> ProcessIdentity {
        ProcessIdentity::new(
            4242,
            "/home/u/.dblocal/runtimes/postgresql/18.7/x86_64/bin/postgres",
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        )
        .expect("valid identity")
    }

    fn found(exe: Option<&str>, started_secs: Option<u64>) -> ProcessObservation {
        ProcessObservation::Found {
            exe: exe.map(PathBuf::from),
            started_at: started_secs.map(|s| SystemTime::UNIX_EPOCH + Duration::from_secs(s)),
        }
    }

    #[test]
    fn rejects_unusable_identities() {
        let now = SystemTime::UNIX_EPOCH;
        assert!(ProcessIdentity::new(0, "/bin/x", now).is_err());
        assert!(ProcessIdentity::new(1, "", now).is_err());
    }

    #[test]
    fn classifies_every_state() {
        let id = identity();
        assert_eq!(
            classify(&id, &ProcessObservation::Missing),
            ProcessState::Absent
        );
        assert_eq!(
            classify(
                &id,
                &found(
                    Some("/home/u/.dblocal/runtimes/postgresql/18.7/x86_64/bin/postgres"),
                    Some(1_700_000_000)
                )
            ),
            ProcessState::Match
        );
        assert_eq!(
            classify(&id, &found(Some("/usr/bin/sleep"), Some(1_700_000_000))),
            ProcessState::Reused
        );
        assert_eq!(
            classify(
                &id,
                &found(
                    Some("/home/u/.dblocal/runtimes/postgresql/18.7/x86_64/bin/postgres"),
                    Some(1_700_000_500)
                )
            ),
            ProcessState::Reused,
            "a different start time under the same PID is a reused PID"
        );
    }

    #[test]
    fn start_time_tolerance_is_symmetric_and_bounded() {
        let id = identity();
        let path = Some("/home/u/.dblocal/runtimes/postgresql/18.7/x86_64/bin/postgres");
        // ±1 s inside tolerance.
        assert_eq!(
            classify(&id, &found(path, Some(1_700_000_001))),
            ProcessState::Match
        );
        assert_eq!(
            classify(&id, &found(path, Some(1_699_999_999))),
            ProcessState::Match
        );
        // 3 s is outside.
        assert_eq!(
            classify(&id, &found(path, Some(1_700_000_003))),
            ProcessState::Reused
        );
        assert_eq!(
            classify(&id, &found(path, Some(1_699_999_997))),
            ProcessState::Reused
        );
    }

    #[test]
    fn missing_facts_are_unknown_not_match() {
        let id = identity();
        let path = "/home/u/.dblocal/runtimes/postgresql/18.7/x86_64/bin/postgres";
        assert_eq!(
            classify(&id, &found(None, Some(1_700_000_000))),
            ProcessState::Unknown,
            "no executable means we cannot prove identity"
        );
        assert_eq!(
            classify(&id, &found(Some(path), None)),
            ProcessState::Unknown,
            "no start time means we cannot rule out reuse"
        );
    }

    /// ` (deleted)` is Linux's `/proc/<pid>/exe` marker; `strip_deleted` is a
    /// no-op on Windows, so the suffix carries no meaning there.
    #[cfg(unix)]
    #[test]
    fn deleted_suffix_is_tolerated_on_the_recorded_side_too() {
        let path = "/home/u/.dblocal/runtimes/postgresql/18.7/x86_64/bin/postgres";
        let id = ProcessIdentity::new(
            7,
            format!("{path} (deleted)"),
            SystemTime::UNIX_EPOCH + Duration::from_secs(10),
        )
        .expect("valid identity");
        assert_eq!(
            classify(&id, &found(Some(path), Some(10))),
            ProcessState::Match
        );
        assert_eq!(
            classify(&id, &found(Some(&format!("{path} (deleted)")), Some(10))),
            ProcessState::Match
        );
    }

    #[test]
    fn states_have_stable_names() {
        assert_eq!(ProcessState::Match.as_str(), "match");
        assert_eq!(ProcessState::Absent.as_str(), "absent");
        assert_eq!(ProcessState::Reused.as_str(), "reused");
        assert_eq!(ProcessState::Unknown.as_str(), "unknown");
        assert!(ProcessState::Match.is_match());
        for state in [
            ProcessState::Absent,
            ProcessState::Reused,
            ProcessState::Unknown,
        ] {
            assert!(!state.is_match(), "{state:?} must never be signalled");
        }
    }

    #[test]
    fn accessors_expose_the_recorded_values() {
        let id = identity();
        assert_eq!(id.pid(), 4242);
        assert_eq!(
            id.exe(),
            Path::new("/home/u/.dblocal/runtimes/postgresql/18.7/x86_64/bin/postgres")
        );
        assert_eq!(
            id.started_at(),
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
        );
    }
}
