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

#[cfg(test)]
mod tests {
    #[test]
    fn cli_defaults_are_sane() {
        assert!(super::DEFAULT_START_TIMEOUT_SECS >= 10);
        assert!(super::STOP_TIMEOUT_SECS >= super::DEFAULT_START_TIMEOUT_SECS);
        assert!(super::DEFAULT_LOG_LINES > 0);
        assert!(super::LOG_FOLLOW_INTERVAL_MS > 0);
        assert!(super::LOG_FOLLOW_WINDOW > 0);
        assert!(super::CROSS_PROCESS_POLL_MS > 0);
    }
}
