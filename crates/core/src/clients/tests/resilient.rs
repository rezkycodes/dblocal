use std::time::Duration;

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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::clients::{ClientError, LaunchPlan, SystemSpawner};

    use super::spawn_resilient;

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
}
