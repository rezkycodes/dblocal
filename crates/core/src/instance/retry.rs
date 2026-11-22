//! Bounded retries for operations that race another live writer.
//!
//! Split out of `helpers.rs` so each file stays under the size limit.

use std::path::Path;

use crate::DblError;
use crate::error::Result;

/// Removes a directory tree that another live writer may be recreating inside.
///
/// Two services can share one home (the GUI and a CLI, or two test services). A
/// delete holds the cross-process claim on the row, but a sibling that lost the
/// claim can still be finishing its own cleanup — for example recreating
/// `delete.inflight` inside `instances/<name>/` right after `InstanceLock::create`
/// wrote it — while this delete's `remove_dir_all` is walking that directory. The
/// kernel then reports `DirectoryNotEmpty` (`ENOTEMPTY`) for an entry that appeared
/// after the walk read it. That is a transient race, not a permissions or disk
/// problem, so retry a bounded number of times with a short pause; if the tree is
/// genuinely unremovable the last error is reported.
pub(crate) fn remove_tree_retrying(path: &Path) -> Result<()> {
    retry_while_not_empty(|| std::fs::remove_dir_all(path))
}

/// Bounded retry around one directory-removal attempt.
///
/// Retries only the transient "an entry appeared while we were removing" race; any
/// other error is returned at once, and a tree that stays non-empty is reported
/// with its last error rather than looping forever.
fn retry_while_not_empty(mut attempt: impl FnMut() -> std::io::Result<()>) -> Result<()> {
    const ATTEMPTS: u32 = 20;
    const PAUSE: std::time::Duration = std::time::Duration::from_millis(10);
    let mut last = None;
    for _ in 0..ATTEMPTS {
        match attempt() {
            Ok(()) => return Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) if is_directory_not_empty(&err) => {
                last = Some(err);
                std::thread::sleep(PAUSE);
            }
            Err(err) => return Err(DblError::Io { source: err }),
        }
    }
    match last {
        Some(err) => Err(DblError::Io { source: err }),
        None => Ok(()),
    }
}

/// Whether `err` is `ENOTEMPTY` / `EEXIST` from a directory that changed under a
/// concurrent writer (`remove_dir_all` returns either, depending on the kernel and
/// the moment the entry appeared).
fn is_directory_not_empty(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::DirectoryNotEmpty | std::io::ErrorKind::AlreadyExists
    ) || matches!(err.raw_os_error(), Some(39) | Some(17))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// A sibling racing the delete can recreate an entry inside the tree between
    /// `remove_dir_all`'s read and its `rmdir`, which surfaces as `ENOTEMPTY`. The
    /// retry must absorb that transient error and eventually succeed, exactly as a
    /// delete of an instance whose marker file a losing service keeps recreating
    /// needs it to.
    #[test]
    fn retry_absorbs_directory_not_empty_until_it_succeeds() {
        let mut calls = 0;
        let result = retry_while_not_empty(|| {
            calls += 1;
            if calls < 3 {
                Err(std::io::Error::from(std::io::ErrorKind::DirectoryNotEmpty))
            } else {
                Ok(())
            }
        });
        assert!(
            result.is_ok(),
            "the transient race must be absorbed: {result:?}"
        );
        assert_eq!(calls, 3, "it retried until the tree was removable");
    }

    /// A directory that never empties is reported, not retried forever: the bound
    /// is what keeps a genuinely stuck tree from hanging the delete.
    #[test]
    fn retry_reports_a_tree_that_never_empties() {
        let mut calls = 0;
        let result = retry_while_not_empty(|| {
            calls += 1;
            Err(std::io::Error::from(std::io::ErrorKind::DirectoryNotEmpty))
        });
        assert_eq!(calls, 20, "the retry is bounded");
        assert!(
            matches!(result, Err(DblError::Io { .. })),
            "a stuck tree is a typed I/O error: {result:?}"
        );
    }

    /// Any other error is reported at once, without retrying: a permission or disk
    /// failure is not a transient race.
    #[test]
    fn retry_does_not_retry_other_errors() {
        let mut calls = 0;
        let result = retry_while_not_empty(|| {
            calls += 1;
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
        });
        assert_eq!(calls, 1, "a non-race error is not retried");
        assert!(matches!(result, Err(DblError::Io { .. })));
    }

    /// Absence is success, so a delete that another process already finished is not
    /// reported as a failure.
    #[test]
    fn retry_treats_absence_as_success() {
        let result =
            retry_while_not_empty(|| Err(std::io::Error::from(std::io::ErrorKind::NotFound)));
        assert!(result.is_ok(), "absence is not an error: {result:?}");
    }
}
