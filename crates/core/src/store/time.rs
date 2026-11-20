//! `SystemTime` encoding for SQLite columns.
//!
//! Timestamps are stored as two integers: whole seconds and nanoseconds since
//! the Unix epoch. No floating point precision is lost and pre-epoch values
//! round-trip exactly.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::DblError;

const NANOS_PER_SEC: u32 = 1_000_000_000;

/// Encodes a [`SystemTime`] as `(seconds, nanoseconds)` since the Unix epoch.
pub(crate) fn encode(time: SystemTime) -> (i64, u32) {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => (duration.as_secs() as i64, duration.subsec_nanos()),
        Err(err) => {
            let before = err.duration();
            let secs = -(before.as_secs() as i64);
            let nanos = before.subsec_nanos();
            if nanos == 0 {
                (secs, 0)
            } else {
                (secs - 1, NANOS_PER_SEC - nanos)
            }
        }
    }
}

/// Decodes `(seconds, nanoseconds)` back into a [`SystemTime`].
pub(crate) fn decode(secs: i64, nanos: u32) -> crate::Result<SystemTime> {
    if nanos >= NANOS_PER_SEC {
        return Err(DblError::Storage {
            reason: format!("invalid timestamp nanoseconds: {nanos}"),
        });
    }
    let nanos_u64 = u64::from(nanos);
    if secs >= 0 {
        let base = Duration::from_secs(secs as u64);
        let total = base
            .checked_add(Duration::from_nanos(nanos_u64))
            .ok_or_else(timestamp_overflow)?;
        Ok(UNIX_EPOCH + total)
    } else {
        let whole = secs.unsigned_abs();
        let back = whole
            .checked_mul(u64::from(NANOS_PER_SEC))
            .and_then(|value| value.checked_sub(nanos_u64))
            .ok_or_else(timestamp_overflow)?;
        Ok(UNIX_EPOCH - Duration::from_nanos(back))
    }
}

/// Encodes an optional timestamp as two optional columns.
pub(crate) fn encode_opt(time: Option<SystemTime>) -> (Option<i64>, Option<u32>) {
    match time {
        Some(t) => {
            let (secs, nanos) = encode(t);
            (Some(secs), Some(nanos))
        }
        None => (None, None),
    }
}

/// Decodes an optional timestamp pair, rejecting half-populated pairs.
pub(crate) fn decode_opt(
    secs: Option<i64>,
    nanos: Option<u32>,
) -> crate::Result<Option<SystemTime>> {
    match (secs, nanos) {
        (None, None) => Ok(None),
        (Some(secs), Some(nanos)) => Ok(Some(decode(secs, nanos)?)),
        _ => Err(DblError::Storage {
            reason: "timestamp stored with only one of its two columns".to_owned(),
        }),
    }
}

fn timestamp_overflow() -> DblError {
    DblError::Storage {
        reason: "timestamp is out of range".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_epoch_and_recent_instants() {
        let (secs, nanos) = encode(UNIX_EPOCH);
        assert_eq!(decode(secs, nanos).expect("decode"), UNIX_EPOCH);

        let instant = UNIX_EPOCH + Duration::new(1_700_000_000, 123_456_789);
        let (secs, nanos) = encode(instant);
        assert_eq!(decode(secs, nanos).expect("decode"), instant);
    }

    #[test]
    fn round_trips_before_epoch() {
        for instant in [
            UNIX_EPOCH - Duration::new(10, 0),
            UNIX_EPOCH - Duration::new(10, 500),
        ] {
            let (secs, nanos) = encode(instant);
            assert_eq!(decode(secs, nanos).expect("decode"), instant);
        }
    }

    #[test]
    fn optional_columns_must_agree() {
        assert!(decode_opt(None, None).expect("none").is_none());
        assert!(decode_opt(Some(1), Some(0)).expect("some").is_some());
        assert!(decode_opt(Some(1), None).is_err());
        assert!(decode_opt(None, Some(0)).is_err());
    }

    #[test]
    fn rejects_out_of_range_nanos() {
        assert!(decode(0, NANOS_PER_SEC).is_err());
    }
}
