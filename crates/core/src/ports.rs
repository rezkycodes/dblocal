//! Port probing and allocation (engine-independent).
//!
//! Owned by W1-D. The two public entry points the service needs are
//! [`probe_bind`] (`F-PRT-02`) and [`suggest_port`] (`F-PRT-01`).

mod allocate;
mod probe;

#[cfg(test)]
pub(crate) mod test_support;

pub use allocate::{
    BindProbeChecker, MAX_SCAN, PortChecker, PortConflict, PortSuggestion, port_conflict,
    suggest_port, suggest_port_with,
};
pub use probe::{BindReport, BindStatus, probe_bind, probe_or_in_use};
