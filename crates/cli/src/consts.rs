//! Every tunable value the CLI uses.
//!
//! The core crate owns the domain numbers (ports, engine timeouts, log sizes) in
//! `dblocal_core::consts`; this module holds only the CLI's own defaults, so no
//! handler hardcodes a duration or a line count.

/// Default for `start --timeout`, in seconds.
pub const DEFAULT_START_TIMEOUT_SECS: u64 = 60;
/// Upper bound for the `stop` wait, in seconds (graceful shutdown + escalation).
pub const STOP_TIMEOUT_SECS: u64 = 120;
/// Default number of log lines `logs` shows from the end.
pub const DEFAULT_LOG_LINES: usize = 200;
/// How often the followed log is re-read.
pub const LOG_FOLLOW_INTERVAL_MS: u64 = 250;
/// Size of the tail window a followed log re-reads each interval.
pub const LOG_FOLLOW_WINDOW: usize = 200;
/// How often the cross-process start race loser re-reads the shared state.
pub const CROSS_PROCESS_POLL_MS: u64 = 100;

// Compile-time invariants, so a wrong default can never ship even though no
// test exercises it: a stop wait shorter than a start wait would be a lie, and a
// zero interval or window would spin or print nothing.
const _: () = assert!(STOP_TIMEOUT_SECS >= DEFAULT_START_TIMEOUT_SECS);
const _: () = assert!(DEFAULT_START_TIMEOUT_SECS > 0);
const _: () = assert!(DEFAULT_LOG_LINES > 0);
const _: () = assert!(LOG_FOLLOW_INTERVAL_MS > 0);
const _: () = assert!(LOG_FOLLOW_WINDOW > 0);
const _: () = assert!(CROSS_PROCESS_POLL_MS > 0);
