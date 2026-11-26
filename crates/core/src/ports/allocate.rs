//! Choosing the next free port for an engine (`F-PRT-01`).
//!
//! The rule from the PRD: the canonical port first (5432 / 3306 / 3307 / 6379),
//! then `+1` upward, skipping anything another DBLocal instance already reserved
//! and anything the operating system reports as taken. The search is bounded by
//! [`crate::consts::MAX_PORT`] and returns a typed error instead of looping.

use std::collections::HashSet;

use crate::DblError;
use crate::error::Result;
use crate::model::{EngineKey, Port};
use crate::ports::probe::{BindReport, BindStatus, probe_or_in_use};

/// How many ports a single suggestion may examine.
///
/// A bound keeps the worst case finite and predictable for a click handler; the
/// search only ever needs a handful of candidates in practice.
pub const MAX_SCAN: u16 = 1000;

/// Whether a port is bindable, abstracted so allocation can be tested without
/// touching real sockets.
pub trait PortChecker {
    /// Returns true when the port can be bound here.
    fn is_free(&self, port: Port) -> bool;
}

/// Real checker: a bind probe on loopback (`F-PRT-02`).
#[derive(Debug, Default, Clone, Copy)]
pub struct BindProbeChecker;

impl PortChecker for BindProbeChecker {
    fn is_free(&self, port: Port) -> bool {
        match probe_or_in_use(port).status() {
            BindStatus::Free => true,
            BindStatus::InUse { .. } => false,
        }
    }
}

/// Outcome of a successful allocation, kept explicit so callers can tell why a
/// port was chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortSuggestion {
    port: Port,
    canonical: bool,
}

impl PortSuggestion {
    /// The chosen port.
    pub const fn port(self) -> Port {
        self.port
    }

    /// Whether this is the engine's canonical port.
    pub const fn is_canonical(self) -> bool {
        self.canonical
    }
}

/// Suggests a port for `engine`, skipping `reserved` and ports in use.
///
/// `reserved` holds ports held by other DBLocal instances (`PortAllocation`
/// rows). A port in that set is skipped even when the OS would allow the bind:
/// two of our own instances must never be offered the same port.
pub fn suggest_port(engine: EngineKey, reserved: &HashSet<Port>) -> Result<Port> {
    Ok(suggest_port_with(engine, reserved, &BindProbeChecker)?.port())
}

/// [`suggest_port`] with an explicit checker, returning the reason as well.
pub fn suggest_port_with(
    engine: EngineKey,
    reserved: &HashSet<Port>,
    checker: &dyn PortChecker,
) -> Result<PortSuggestion> {
    suggest_port_from_with(engine.canonical_port(), reserved, checker)
}

/// Suggests a free port starting at `start_port`, skipping `reserved` and ports in use.
pub fn suggest_port_from_with(
    start_port: u16,
    reserved: &HashSet<Port>,
    checker: &dyn PortChecker,
) -> Result<PortSuggestion> {
    let canonical = Port::new(start_port).map_err(|_| DblError::InvalidInput {
        field: "port",
        reason: "canonical port is outside the allowed range".to_owned(),
    })?;

    let mut candidate = canonical;
    for _ in 0..MAX_SCAN {
        if !reserved.contains(&candidate) && checker.is_free(candidate) {
            return Ok(PortSuggestion {
                port: candidate,
                canonical: candidate == canonical,
            });
        }
        // `Port` already caps at `MAX_PORT`, so a checked increment is the only
        // way forward and terminates the loop at the top of the range.
        let Some(following) = candidate
            .get()
            .checked_add(1)
            .and_then(|number| Port::new(number).ok())
        else {
            break;
        };
        candidate = following;
    }

    Err(DblError::PortInUse {
        port: canonical.get(),
    })
}

/// Convenience for the UI: is this specific port usable for `engine`?
///
/// Returns the offending owner's port when `reserved` already holds it, so
/// `F-PRT-01` AC3 can name the other instance; the caller maps the port back to
/// a name through the store.
pub fn port_conflict(port: Port, reserved: &HashSet<Port>) -> Option<PortConflict> {
    if reserved.contains(&port) {
        return Some(PortConflict::ReservedByInstance);
    }
    let report: BindReport = probe_or_in_use(port);
    match report.status() {
        BindStatus::Free => None,
        BindStatus::InUse { detail } => Some(PortConflict::InUse {
            detail: detail.clone(),
        }),
    }
}

/// Why a port cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PortConflict {
    /// Another DBLocal instance already reserved it.
    ReservedByInstance,
    /// Something outside DBLocal is listening.
    InUse {
        /// Interface description, best effort.
        detail: String,
    },
}

impl PortConflict {
    /// Whether a different DBLocal instance owns the port.
    pub const fn is_ours(&self) -> bool {
        matches!(self, Self::ReservedByInstance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::test_support::{attempt, bind_test_listener, reserve_test_port};
    use std::collections::HashSet;

    /// Checker that reports a fixed set of busy ports.
    struct Busy(HashSet<Port>);

    impl Busy {
        fn of(ports: &[u16]) -> Self {
            Self(
                ports
                    .iter()
                    .map(|p| Port::new(*p).expect("valid port"))
                    .collect(),
            )
        }
    }

    impl PortChecker for Busy {
        fn is_free(&self, port: Port) -> bool {
            !self.0.contains(&port)
        }
    }

    fn reserved(ports: &[u16]) -> HashSet<Port> {
        ports
            .iter()
            .map(|p| Port::new(*p).expect("valid port"))
            .collect()
    }

    #[test]
    fn first_instance_gets_the_canonical_port() {
        let busy = Busy(HashSet::new());
        for engine in EngineKey::ALL {
            let suggestion = suggest_port_with(engine, &HashSet::new(), &busy).expect("free");
            assert_eq!(suggestion.port().get(), engine.canonical_port());
            assert!(suggestion.is_canonical());
        }
    }

    #[test]
    fn a_reserved_canonical_port_moves_to_the_next_one() {
        let busy = Busy(HashSet::new());
        let suggestion =
            suggest_port_with(EngineKey::Postgresql, &reserved(&[5432]), &busy).expect("free");
        assert_eq!(suggestion.port().get(), 5433);
        assert!(!suggestion.is_canonical());
    }

    #[test]
    fn a_reserved_run_is_skipped_in_one_pass() {
        let busy = Busy(HashSet::new());
        let taken = reserved(&[5432, 5433, 5434]);
        let suggestion = suggest_port_with(EngineKey::Postgresql, &taken, &busy).expect("free");
        assert_eq!(suggestion.port().get(), 5435);
    }

    #[test]
    fn an_occupied_canonical_port_moves_to_the_next_one() {
        let busy = Busy::of(&[5432]);
        let suggestion =
            suggest_port_with(EngineKey::Postgresql, &HashSet::new(), &busy).expect("free");
        assert_eq!(suggestion.port().get(), 5433);
    }

    #[test]
    fn reserved_and_occupied_are_skipped_together() {
        // 5432 reserved by another instance, 5433 held by a foreign process.
        let busy = Busy::of(&[5433]);
        let suggestion =
            suggest_port_with(EngineKey::Postgresql, &reserved(&[5432]), &busy).expect("free");
        assert_eq!(suggestion.port().get(), 5434);
    }

    #[test]
    fn every_engine_starts_from_its_own_canonical_port() {
        let busy = Busy(HashSet::new());
        assert_eq!(
            suggest_port_with(EngineKey::Mysql, &reserved(&[3306]), &busy)
                .expect("free")
                .port()
                .get(),
            3307
        );
        assert_eq!(
            suggest_port_with(EngineKey::Mariadb, &reserved(&[3307]), &busy)
                .expect("free")
                .port()
                .get(),
            3308
        );
        assert_eq!(
            suggest_port_with(EngineKey::Valkey, &reserved(&[6379]), &busy)
                .expect("free")
                .port()
                .get(),
            6380
        );
    }

    #[test]
    fn an_exhausted_search_is_a_typed_error_not_a_hang() {
        struct AllBusy;
        impl PortChecker for AllBusy {
            fn is_free(&self, _port: Port) -> bool {
                false
            }
        }
        let err =
            suggest_port_with(EngineKey::Valkey, &HashSet::new(), &AllBusy).expect_err("must fail");
        assert!(matches!(err, DblError::PortInUse { port: 6379 }));
        assert_eq!(err.code(), crate::error::CODE_PORT_IN_USE);
    }

    #[test]
    fn conflict_on_a_free_unreserved_port_is_none() {
        // `reserve_test_port` skips busy slots, so an unheld reserved port must
        // read as no conflict. Retried in case another process takes the number
        // between hand-out and the check.
        attempt(|| {
            let port = reserve_test_port();
            match port_conflict(port, &HashSet::new()) {
                None => Ok(()),
                Some(other) => Err(format!("{port} was taken after hand-out: {other:?}")),
            }
        });
    }

    #[test]
    fn conflict_detection_distinguishes_ours_from_foreign() {
        let mine = reserved(&[5432]);
        assert_eq!(
            port_conflict(Port::new(5432).expect("valid"), &mine),
            Some(PortConflict::ReservedByInstance)
        );
        assert!(PortConflict::ReservedByInstance.is_ours());

        // A port outside our reservations but actively listening is foreign.
        let (listener, port) = bind_test_listener();
        match port_conflict(port, &HashSet::new()) {
            Some(PortConflict::InUse { detail }) => assert!(!detail.is_empty()),
            other => panic!("expected foreign conflict, got {other:?}"),
        }
        drop(listener);
        // The release side is retried: another process may grab the number.
        attempt(|| match port_conflict(port, &HashSet::new()) {
            None => Ok(()),
            Some(other) => Err(format!("{port} still conflicting after release: {other:?}")),
        });
    }

    #[test]
    fn real_checker_sees_a_listening_port() {
        let (listener, port) = bind_test_listener();
        assert!(
            !BindProbeChecker.is_free(port),
            "a held port must read busy"
        );
        drop(listener);
        attempt(|| {
            if BindProbeChecker.is_free(port) {
                Ok(())
            } else {
                Err(format!("{port} still reads busy after release"))
            }
        });
    }
}
