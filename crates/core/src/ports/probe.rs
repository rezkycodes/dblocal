//! TCP bind probing for port availability (`F-PRT-02`).
//!
//! The probe performs a **real** bind on loopback only — `127.0.0.1` and, when
//! the host can bind it, `::1` — with default socket options. It never sets
//! `SO_REUSEADDR`: the PRD edge cases and W0-B both note that reuse makes a bind
//! succeed while another listener owns the port, which would defeat the check.
//! On Windows the standard library's listener already uses `SO_EXCLUSIVEADDRUSE`
//! semantics.
//!
//! The probe socket is closed before returning (`F-PRT-02` AC3) so the engine can
//! bind immediately afterwards without racing us.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6, TcpListener};

use crate::error::Result;
use crate::model::Port;

/// Outcome of probing one port on loopback.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BindStatus {
    /// The port is bindable on every loopback interface the host has.
    Free,
    /// At least one probed interface refused the bind.
    InUse {
        /// Which interface, best effort. Never contains a secret.
        detail: String,
    },
}

/// Report returned by [`probe_bind`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindReport {
    port: Port,
    status: BindStatus,
}

impl BindReport {
    /// Builds a report; normally only [`probe_bind`] and test doubles do this.
    pub const fn new(port: Port, status: BindStatus) -> Self {
        Self { port, status }
    }

    /// Port that was probed.
    pub const fn port(&self) -> Port {
        self.port
    }

    /// What the bind said.
    pub const fn status(&self) -> &BindStatus {
        &self.status
    }

    /// Whether the port is free on every probed interface.
    pub const fn is_free(&self) -> bool {
        matches!(self.status, BindStatus::Free)
    }
}

/// Probes `port` with a real bind, then closes the socket.
///
/// Both loopback families are checked because the PRD edge cases require it:
/// `::1` can be occupied while `127.0.0.1` is free. A host without an IPv6
/// loopback is not a conflict and reports `Free`.
pub fn probe_bind(port: Port) -> Result<BindReport> {
    let number = port.get();

    match bind_v4(number) {
        Ok(()) => {}
        Err(err) if is_addr_in_use(&err) => {
            return Ok(BindReport::new(
                port,
                BindStatus::InUse {
                    detail: format!("127.0.0.1:{number}"),
                },
            ));
        }
        Err(err) => return Err(err.into()),
    }

    match bind_v6(number) {
        Ok(()) => Ok(BindReport::new(port, BindStatus::Free)),
        Err(err) if is_addr_in_use(&err) => Ok(BindReport::new(
            port,
            BindStatus::InUse {
                detail: format!("[::1]:{number}"),
            },
        )),
        // No IPv6 loopback on this host: only the IPv4 answer is meaningful.
        Err(err) if is_no_ipv6(&err) => Ok(BindReport::new(port, BindStatus::Free)),
        Err(err) => Err(err.into()),
    }
}

/// [`probe_bind`] with an unexpected socket error folded into `InUse`.
///
/// Reconciliation and port suggestion should never abort because one probe hit
/// an odd platform error; the conservative answer is "not usable".
pub fn probe_or_in_use(port: Port) -> BindReport {
    probe_bind(port).unwrap_or_else(|err| {
        tracing::warn!(port = port.get(), error = %err, "bind probe failed");
        BindReport::new(
            port,
            BindStatus::InUse {
                detail: format!("probe failed: {}", err.code()),
            },
        )
    })
}

fn bind_v4(port: u16) -> std::io::Result<()> {
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))?;
    drop(listener);
    Ok(())
}

fn bind_v6(port: u16) -> std::io::Result<()> {
    let listener = TcpListener::bind(SocketAddrV6::new(Ipv6Addr::LOCALHOST, port, 0, 0))?;
    drop(listener);
    Ok(())
}

fn is_addr_in_use(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::AddrInUse
}

fn is_no_ipv6(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::AddrNotAvailable | std::io::ErrorKind::Unsupported
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ephemeral() -> (TcpListener, Port) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("ephemeral bind");
        let port = listener.local_addr().expect("local addr").port();
        (listener, Port::new(port).expect("valid ephemeral port"))
    }

    #[test]
    fn a_listening_port_is_in_use() {
        let (listener, port) = ephemeral();
        let report = probe_bind(port).expect("probe");
        assert!(!report.is_free(), "expected InUse while listener is open");
        assert_eq!(report.port(), port);
        drop(listener);
    }

    #[test]
    fn a_closed_port_is_free() {
        let (listener, port) = ephemeral();
        drop(listener);
        let report = probe_bind(port).expect("probe");
        assert!(report.is_free(), "expected Free after closing the listener");
        assert_eq!(report.status(), &BindStatus::Free);
    }

    #[test]
    fn probing_is_repeatable_and_leaves_no_socket_behind() {
        // `F-PRT-02` AC3: the probe socket must be closed again, otherwise the
        // engine could not bind right after.
        let (listener, port) = ephemeral();
        drop(listener);
        assert!(probe_bind(port).expect("probe").is_free());
        assert!(probe_bind(port).expect("probe").is_free());
        assert!(probe_bind(port).expect("probe").is_free());
    }

    #[test]
    fn an_in_use_report_names_the_interface() {
        let (listener, port) = ephemeral();
        match probe_bind(port).expect("probe").status().clone() {
            BindStatus::InUse { detail } => assert!(
                detail.starts_with("127.0.0.1:") || detail.starts_with("[::1]:"),
                "unexpected detail {detail}"
            ),
            BindStatus::Free => panic!("expected InUse"),
        }
        drop(listener);
    }

    #[test]
    fn probe_or_in_use_keeps_the_report_shape() {
        let (listener, port) = ephemeral();
        assert!(!probe_or_in_use(port).is_free());
        drop(listener);
        assert!(probe_or_in_use(port).is_free());
    }

    #[test]
    fn error_classification_matches_platform_kinds() {
        let in_use = std::io::Error::from(std::io::ErrorKind::AddrInUse);
        assert!(is_addr_in_use(&in_use));
        assert!(!is_no_ipv6(&in_use));

        let no_v6 = std::io::Error::from(std::io::ErrorKind::AddrNotAvailable);
        assert!(is_no_ipv6(&no_v6));
        assert!(!is_addr_in_use(&no_v6));

        let unsupported = std::io::Error::from(std::io::ErrorKind::Unsupported);
        assert!(is_no_ipv6(&unsupported));

        let other = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert!(!is_addr_in_use(&other));
        assert!(!is_no_ipv6(&other));
    }

    #[test]
    fn a_v6_only_listener_is_reported_in_use() {
        // PRD edge case: `::1` occupied while `127.0.0.1` is free. If this host
        // has no IPv6 loopback the test is vacuously satisfied by the v4 answer.
        let Ok(listener) = TcpListener::bind((Ipv6Addr::LOCALHOST, 0)) else {
            return;
        };
        let port = listener.local_addr().expect("addr").port();
        let port = Port::new(port).expect("valid port");
        let report = probe_bind(port).expect("probe");
        // Either v6 is exclusive and we see InUse, or the v4 probe already
        // failed first — in both cases the port must not be reported free.
        assert!(!report.is_free(), "a v6 listener must not read as free");
        drop(listener);
        assert!(probe_bind(port).expect("probe").is_free());
    }

    #[test]
    fn privileged_ports_cannot_even_be_constructed() {
        // `Port` validates at the boundary, so the probe can never be asked to
        // test a privileged port (PRD `F-PRT` edge case).
        assert!(Port::new(80).is_err());
        assert!(Port::new(1023).is_err());
    }
}
