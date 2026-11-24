//! Names of archived data directories (`F-INS-05` AC3).
//!
//! An archive lives at `<home>/archive/<name>-<YYYYMMDD-HHMM>`, the stamp being
//! the user's **local** time when the delete ran. A minute is a coarse unit, so
//! two deletes of the same name can land on the same stamp (create, delete,
//! create again, delete again). The second one must never reuse or overwrite the
//! first: [`free_target`] appends `-2`, `-3`, … until the name is unused.
//!
//! Directories written by earlier builds used `<name>-<epoch milliseconds>`;
//! [`matches_archive_name`] still recognises them so crash recovery keeps working
//! for data archived before this format.

use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use jiff::ZonedDateTime;

/// `YYYYMMDD-HHMM`.
const STAMP_FORMAT: &str = "%Y%m%d-%H%M";

/// Highest collision suffix tried before giving up.
const MAX_SUFFIX: u32 = 999;

/// The local-time stamp for `now`.
///
/// The zone is the operating system's (`TZ`, `/etc/localtime`, the Windows time
/// zone setting). If it cannot be applied, which only happens for a time
/// outside the representable range, the stamp falls back to whole seconds since
/// the epoch: digits only, so [`matches_archive_name`] still recognises it.
pub(super) fn stamp(now: SystemTime) -> String {
    match ZonedDateTime::try_from(now) {
        Ok(local) => format_stamp(&local),
        Err(_) => now
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs())
            .to_string(),
    }
}

fn format_stamp(moment: &ZonedDateTime) -> String {
    moment.strftime(STAMP_FORMAT).to_string()
}

/// The first unused `<name>-<stamp>[-N]` path under `root`.
///
/// A name counts as used when anything is there, including a dangling symlink
/// (`symlink_metadata` does not follow it) or an entry that cannot be inspected.
pub(super) fn free_target(root: &Path, name: &str, stamp: &str) -> io::Result<PathBuf> {
    let base = format!("{name}-{stamp}");
    let first = root.join(&base);
    if !is_taken(&first) {
        return Ok(first);
    }
    for suffix in 2..=MAX_SUFFIX {
        let candidate = root.join(format!("{base}-{suffix}"));
        if !is_taken(&candidate) {
            return Ok(candidate);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("no free archive name for {base} under {}", root.display()),
    ))
}

fn is_taken(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(_) => true,
        Err(err) => err.kind() != io::ErrorKind::NotFound,
    }
}

/// Whether the directory name `dir_name` is `<prefix><stamp>` for an archive of
/// the instance whose prefix (`<name>-`) is `prefix`.
///
/// The match is exact, not a prefix: `pg` must never claim `pg-main-…`, whose
/// remainder is not a stamp. The accepted remainders are
///
/// - `YYYYMMDD-HHMM`, optionally followed by one `-N` collision suffix, and
/// - a run of ASCII digits (the epoch-millisecond form older builds wrote).
pub(super) fn matches_archive_name(dir_name: &str, prefix: &str) -> bool {
    let Some(rest) = dir_name.strip_prefix(prefix) else {
        return false;
    };
    all_digits(rest) || is_minute_stamp(rest)
}

fn is_minute_stamp(rest: &str) -> bool {
    let mut parts = rest.split('-');
    let (Some(date), Some(time)) = (parts.next(), parts.next()) else {
        return false;
    };
    let suffix = parts.next();
    parts.next().is_none()
        && date.len() == 8
        && all_digits(date)
        && time.len() == 4
        && all_digits(time)
        && suffix.is_none_or(all_digits)
}

fn all_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::time::Duration;

    use jiff::Timestamp;
    use jiff::tz::{Offset, TimeZone};

    use super::*;

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    /// 2026-11-24 04:11:46 UTC.
    const SAMPLE: u64 = 1_795_493_506;

    fn in_offset(secs: u64, offset_secs: i32) -> ZonedDateTime {
        let offset = Offset::from_seconds(offset_secs).expect("offset");
        Timestamp::try_from(at(secs))
            .expect("timestamp")
            .to_zoned_datetime(TimeZone::fixed(offset))
    }

    #[test]
    fn stamp_is_year_month_day_hour_minute() {
        assert_eq!(format_stamp(&in_offset(SAMPLE, 0)), "20261124-0411");
    }

    #[test]
    fn stamp_follows_the_offset_it_is_given() {
        assert_eq!(format_stamp(&in_offset(SAMPLE, 7 * 3600)), "20261124-1111");
        assert_eq!(format_stamp(&in_offset(SAMPLE, -5 * 3600)), "20261123-2311");
    }

    #[test]
    fn local_stamp_has_the_documented_shape() {
        let text = stamp(SystemTime::now());
        assert_eq!(text.len(), 13, "{text}");
        let (date, time) = text.split_once('-').expect("one dash");
        assert!(all_digits(date) && date.len() == 8, "{text}");
        assert!(all_digits(time) && time.len() == 4, "{text}");
    }

    #[test]
    fn the_first_free_name_is_the_plain_stamp() {
        let dir = tempfile::tempdir().expect("tmp");
        let target = free_target(dir.path(), "pg", "20261124-0411").expect("free");
        assert_eq!(target, dir.path().join("pg-20261124-0411"));
    }

    #[test]
    fn taken_names_get_a_counting_suffix() {
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::create_dir(dir.path().join("pg-20261124-0411")).expect("first");
        let second = free_target(dir.path(), "pg", "20261124-0411").expect("second");
        assert_eq!(second, dir.path().join("pg-20261124-0411-2"));
        std::fs::create_dir(&second).expect("second dir");
        let third = free_target(dir.path(), "pg", "20261124-0411").expect("third");
        assert_eq!(third, dir.path().join("pg-20261124-0411-3"));
    }

    #[test]
    fn another_name_or_minute_does_not_collide() {
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::create_dir(dir.path().join("pg-20261124-0411")).expect("existing");
        assert_eq!(
            free_target(dir.path(), "pg", "20261124-0412").expect("next minute"),
            dir.path().join("pg-20261124-0412")
        );
        assert_eq!(
            free_target(dir.path(), "db", "20261124-0411").expect("other name"),
            dir.path().join("db-20261124-0411")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_symlink_still_occupies_the_name() {
        let dir = tempfile::tempdir().expect("tmp");
        std::os::unix::fs::symlink("/nonexistent-dblocal-target", dir.path().join("pg-1-2"))
            .expect("link");
        let target = free_target(dir.path(), "pg", "1-2").expect("free");
        assert_eq!(target, dir.path().join("pg-1-2-2"));
    }

    #[test]
    fn running_out_of_suffixes_is_an_error_not_an_overwrite() {
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::create_dir(dir.path().join("pg-s")).expect("base");
        for suffix in 2..=MAX_SUFFIX {
            std::fs::create_dir(dir.path().join(format!("pg-s-{suffix}"))).expect("suffix");
        }
        let err = free_target(dir.path(), "pg", "s").expect_err("exhausted");
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn recognises_minute_stamps_with_and_without_a_collision_suffix() {
        assert!(matches_archive_name("pg-20261124-0411", "pg-"));
        assert!(matches_archive_name("pg-20261124-0411-2", "pg-"));
        assert!(matches_archive_name("pg-20261124-0411-17", "pg-"));
    }

    #[test]
    fn still_recognises_the_epoch_millisecond_form() {
        assert!(matches_archive_name("pg-1700000000000", "pg-"));
        assert!(matches_archive_name("abc-1", "abc-"));
    }

    #[test]
    fn never_claims_a_longer_name() {
        assert!(!matches_archive_name("pg-main-1700000000000", "pg-"));
        assert!(!matches_archive_name("pg-main-20261124-0411", "pg-"));
        // An instance whose own name ends in a stamp is not mistaken either.
        assert!(!matches_archive_name(
            "pg-20261124-0411-20261124-0412",
            "pg-"
        ));
        assert!(!matches_archive_name("pgx-20261124-0411", "pg-"));
    }

    #[test]
    fn rejects_malformed_stamps() {
        for name in [
            "pg-",
            "pg-20261124-",
            "pg-20261124-041",
            "pg-20261124-04111",
            "pg-2026112-0411",
            "pg-20261124-0411-",
            "pg-20261124-0411-x",
            "pg-20261124-0411-2-3",
            "pg-2026112a-0411",
        ] {
            assert!(!matches_archive_name(name, "pg-"), "{name}");
        }
    }
}
