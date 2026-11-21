//! Deterministic clock and random source for the mock service.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

/// A seedable clock whose wall time only moves when the test advances it, plus
/// an xorshift64* generator so generated ids and values are reproducible.
#[derive(Debug)]
pub(crate) struct Deterministic {
    seed: Mutex<u64>,
    base_time: SystemTime,
    offset_ms: AtomicU64,
}

impl Deterministic {
    pub(crate) fn new(seed: u64, base_time: SystemTime) -> Self {
        Self {
            seed: Mutex::new(if seed == 0 { 1 } else { seed }),
            base_time,
            offset_ms: AtomicU64::new(0),
        }
    }

    pub(crate) fn now(&self) -> SystemTime {
        let ms = self.offset_ms.load(Ordering::Relaxed);
        self.base_time + Duration::from_millis(ms)
    }

    pub(crate) fn advance(&self, by: Duration) {
        self.offset_ms
            .fetch_add(by.as_millis() as u64, Ordering::Relaxed);
    }

    pub(crate) fn next_u64(&self) -> u64 {
        let mut guard = self.seed.lock().unwrap_or_else(|e| e.into_inner());
        let mut x = *guard;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        *guard = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub(crate) fn gen_id(&self, prefix: &str) -> String {
        format!("{prefix}-{:016x}", self.next_u64())
    }

    /// A deterministic RFC 4122-shaped (version 4 variant) UUID drawn from the
    /// seedable RNG. `seed_instance` uses it so `BTreeMap` iteration order — and
    /// therefore the render tests — is reproducible.
    pub(crate) fn gen_uuid(&self) -> String {
        let hi = self.next_u64();
        let lo = self.next_u64();
        let bytes = [
            (hi >> 56) as u8,
            (hi >> 48) as u8,
            (hi >> 40) as u8,
            (hi >> 32) as u8,
            (hi >> 24) as u8,
            (hi >> 16) as u8,
            (hi >> 8) as u8,
            hi as u8,
            (lo >> 56) as u8,
            (lo >> 48) as u8,
            (lo >> 40) as u8,
            (lo >> 32) as u8,
            (lo >> 24) as u8,
            (lo >> 16) as u8,
            (lo >> 8) as u8,
            lo as u8,
        ];
        format!(
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-4{:01x}{:02x}-8{:01x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            bytes[0],
            bytes[1],
            bytes[2],
            bytes[3],
            bytes[4],
            bytes[5],
            bytes[6] & 0x0f,
            bytes[7],
            bytes[8] & 0x0f,
            bytes[9],
            bytes[10],
            bytes[11],
            bytes[12],
            bytes[13],
            bytes[14],
            bytes[15],
        )
    }
}

/// Instant used only for internal bookkeeping; kept out of reported values.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_advances_deterministically() {
        let det = Deterministic::new(7, SystemTime::UNIX_EPOCH);
        assert_eq!(det.now(), SystemTime::UNIX_EPOCH);
        det.advance(Duration::from_millis(1500));
        assert_eq!(
            det.now(),
            SystemTime::UNIX_EPOCH + Duration::from_millis(1500)
        );
    }

    #[test]
    fn rng_is_seedable_and_reproducible() {
        let a = Deterministic::new(42, SystemTime::UNIX_EPOCH);
        let b = Deterministic::new(42, SystemTime::UNIX_EPOCH);
        for _ in 0..8 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        let c = Deterministic::new(43, SystemTime::UNIX_EPOCH);
        assert_ne!(a.next_u64(), c.next_u64());
    }

    #[test]
    fn generated_uuids_are_deterministic_and_parse() {
        use crate::model::InstanceId;
        let a = Deterministic::new(7, SystemTime::UNIX_EPOCH);
        let b = Deterministic::new(7, SystemTime::UNIX_EPOCH);
        for _ in 0..4 {
            let first = a.gen_uuid();
            let second = b.gen_uuid();
            assert_eq!(first, second, "same seed must yield the same uuid");
            assert!(
                InstanceId::parse(&first).is_ok(),
                "gen_uuid must be a parseable InstanceId: {first}"
            );
        }
        let c = Deterministic::new(8, SystemTime::UNIX_EPOCH);
        assert_ne!(a.gen_uuid(), c.gen_uuid());
    }

    #[test]
    fn ids_are_prefixed() {
        let det = Deterministic::new(1, SystemTime::UNIX_EPOCH);
        assert!(det.gen_id("op").starts_with("op-"));
    }
}
