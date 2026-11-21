use std::path::Path;
use std::time::{Duration, Instant};

use crate::clients::{ClientError, LaunchPlan, SystemSpawner, run};

/// Runs a real spawn, retrying only a transient `LaunchFailed`.
///
/// On a loaded shared machine `fork` can momentarily fail with EAGAIN/ENOMEM,
/// which the spawner reports as `LaunchFailed`. Any other error (wrong path,
/// missing script, bad argv) is returned immediately, so a real defect still
/// fails on the first attempt.
pub fn spawn_resilient(plan: &LaunchPlan, spawner: &SystemSpawner) -> Result<(), ClientError> {
    let mut last = None;
    for _ in 0..5 {
        match run(plan, spawner) {
            Ok(()) => return Ok(()),
            Err(err @ ClientError::LaunchFailed { .. }) => last = Some(err),
            Err(err) => return Err(err),
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    match last {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Waits until `path` exists and contains exactly `expected`.
///
/// The child creates the file with a shell redirect, which makes it visible
/// empty for a moment before the write lands, so polling for existence alone
/// races. Panics with the last content seen if the deadline passes.
pub fn wait_for_contents(path: &Path, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = String::new();
    while Instant::now() < deadline {
        if let Ok(read) = std::fs::read_to_string(path) {
            if read == expected {
                return;
            }
            last = read;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "{} never reached {expected:?}; last content {last:?}",
        path.display()
    );
}

/// Waits until `path` exists and is non-empty, returning its contents.
pub fn wait_for_nonempty(path: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(read) = std::fs::read_to_string(path)
            && !read.is_empty()
        {
            return read;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("{} never produced output", path.display());
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::clients::{ClientError, LaunchPlan, SystemSpawner};

    use super::{spawn_resilient, wait_for_contents, wait_for_nonempty};

    #[test]
    fn a_missing_client_is_not_retried_away() {
        // ClientNotFound is a real defect, not a transient fork failure.
        let plan = LaunchPlan::new(
            PathBuf::from("/nonexistent/dblocal-client"),
            vec![],
            vec![],
            None,
            None,
            None,
        );
        let err = spawn_resilient(&plan, &SystemSpawner::default()).unwrap_err();
        assert!(matches!(err, ClientError::ClientNotFound { .. }), "{err:?}");
    }

    #[test]
    fn wait_for_contents_accepts_an_already_complete_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("done.txt");
        std::fs::write(&path, "hello").unwrap();
        wait_for_contents(&path, "hello");
        assert_eq!(wait_for_nonempty(&path), "hello");
    }

    #[test]
    fn wait_for_contents_ignores_an_empty_file_until_it_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("late.txt");
        std::fs::write(&path, "").unwrap();
        let writer = path.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            std::fs::write(&writer, "later").unwrap();
        });
        wait_for_contents(&path, "later");
    }
}
