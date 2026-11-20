//! TCP bind probing for port availability (`F-PRT-02`).
//!
//! The probe performs a **real** bind on loopback only — `127.0.0.1` and, when
//! the host can bind it, `::1` — with **no** `SO_REUSEADDR`. Reuse lets a bind
//! succeed while another listener owns the port (PRD `F-PRT` edge case, W0-B),
//! which would defeat the check. The socket is closed before returning
//! (`F-PRT-02` AC3) so the engine can bind immediately afterwards.
//!
//! Implementation note: `std::net::TcpListener::bind` sets `SO_REUSEADDR` on
//! Unix. To avoid it we construct the socket through `tokio::net::TcpSocket`
//! (which wraps `socket2` and never sets reuse unless asked) and bind it
//! directly. `tokio` is already a core dependency.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

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

    match bind_addr(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), number)) {
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

    match bind_addr(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), number)) {
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

/// Creates a socket, binds it with default options (no reuse), then closes it.
fn bind_addr(addr: SocketAddr) -> std::io::Result<()> {
    let socket = match addr {
        SocketAddr::V4(_) => tokio::net::TcpSocket::new_v4()?,
        SocketAddr::V6(_) => tokio::net::TcpSocket::new_v6()?,
    };
    socket.bind(addr)?;
    // Dropping the socket closes it immediately.
    drop(socket);
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
    use crate::ports::test_support::{attempt, bind_test_listener, bind_v6_test_listener};

    #[test]
    fn a_listening_port_is_in_use() {
        let (listener, port) = bind_test_listener();
        let report = probe_bind(port).expect("probe");
        assert!(!report.is_free(), "expected InUse while listener is open");
        assert_eq!(report.port(), port);
        drop(listener);
    }

    #[test]
    fn a_closed_port_is_free() {
        // Acquire, release and probe in one retried cycle. Another process may
        // take the number in the gap; that is not the property under test, so
        // the next candidate is tried instead.
        attempt(|| {
            let (listener, port) = bind_test_listener();
            drop(listener);
            match probe_bind(port).expect("probe").status().clone() {
                BindStatus::Free => Ok(()),
                BindStatus::InUse { detail } => {
                    Err(format!("{port} was taken again after release: {detail}"))
                }
            }
        });
    }

    #[test]
    fn probing_is_repeatable_and_leaves_no_socket_behind() {
        // `F-PRT-02` AC3: the probe socket must be closed again, otherwise the
        // engine could not bind right after. Three probes of the same released
        // port must all report Free.
        attempt(|| {
            let (listener, port) = bind_test_listener();
            drop(listener);
            for round in 1..=3 {
                match probe_bind(port).expect("probe").status().clone() {
                    BindStatus::Free => {}
                    BindStatus::InUse { detail } => {
                        return Err(format!("{port} busy on probe {round}: {detail}"));
                    }
                }
            }
            Ok(())
        });
    }

    #[test]
    fn an_in_use_report_names_the_interface() {
        let (listener, port) = bind_test_listener();
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
        let (listener, port) = bind_test_listener();
        assert!(!probe_or_in_use(port).is_free());
        drop(listener);
        attempt(|| match probe_or_in_use(port).status() {
            BindStatus::Free => Ok(()),
            BindStatus::InUse { detail } => Err(format!("{port} still in use: {detail}")),
        });
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
        // PRD edge case: `::1` occupied while `127.0.0.1` is free. The helper
        // returns `None` when the host has no IPv6 loopback, so this is skipped
        // rather than silently vacuous.
        let Some((listener, port)) = bind_v6_test_listener() else {
            return;
        };
        let report = probe_bind(port).expect("probe");
        assert!(!report.is_free(), "a v6 listener must not read as free");
        drop(listener);
        attempt(|| match probe_bind(port).expect("probe").status() {
            BindStatus::Free => Ok(()),
            BindStatus::InUse { detail } => Err(format!("{port} still in use: {detail}")),
        });
    }

    #[test]
    fn privileged_ports_cannot_even_be_constructed() {
        assert!(Port::new(80).is_err());
        assert!(Port::new(1023).is_err());
    }
}
