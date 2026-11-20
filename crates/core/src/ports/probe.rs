//! TCP bind probing for port availability (`F-PRT-02`).
//!
//! The probe answers one question: **would the engine's own bind succeed?** That
//! is not the same as "is the port unused", because every engine we manage sets
//! `SO_REUSEADDR` before `listen`:
//!
//! - Measured on this machine: once a server closed a connection first, its port
//!   sits in `TIME_WAIT` for ~60 s. A bind **without** the option fails with
//!   `EADDRINUSE`; a bind **with** it succeeds. PostgreSQL, MySQL and Valkey all
//!   set it, so an engine starts fine on such a port. Reporting `InUse` there
//!   would block Stop→Start for a minute and break `F-INS-03` / `F-PRT-02`.
//! - An *active* listener still refuses a reuse-enabled bind on Linux and macOS
//!   (only `TIME_WAIT` is let through), so the probe stays honest about real
//!   conflicts.
//!
//! The socket is still closed before returning (`F-PRT-02` AC3), so the engine
//! can bind immediately afterwards. On Windows the option means something else —
//! it lets a bind win over an *active* listener, which would make the probe lie
//! in the opposite direction — so it is deliberately not set there.

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

/// Creates a socket, configures it like the engines do, binds it, then closes it.
///
/// `SO_REUSEADDR` is set on Unix so a `TIME_WAIT` port reads `Free`, which is
/// what an engine needs (see the module docs). It is deliberately absent on
/// Windows, where the same flag would let the probe bind over an *active*
/// listener.
fn bind_addr(addr: SocketAddr) -> std::io::Result<()> {
    let socket = match addr {
        SocketAddr::V4(_) => tokio::net::TcpSocket::new_v4()?,
        SocketAddr::V6(_) => tokio::net::TcpSocket::new_v6()?,
    };
    #[cfg(unix)]
    socket.set_reuseaddr(true)?;
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

    /// Outcome of trying to reproduce the `TIME_WAIT` state.
    #[cfg(unix)]
    enum TimeWait {
        /// The kernel created the state; the port is ours to assert on.
        Created(Port),
        /// This kernel did not leave a `TIME_WAIT` entry, so the test proves
        /// nothing here and must say so out loud.
        NotCreated,
    }

    /// Binds the way an engine does and produces a `TIME_WAIT` port.
    ///
    /// The sequence is the one that bites in production: the server accepts a
    /// connection and closes it first, so the server side carries `TIME_WAIT`.
    /// The precondition is explicit — a plain bind *without* `SO_REUSEADDR` must
    /// fail on that port — which proves the kernel really created the state.
    #[cfg(unix)]
    fn port_in_time_wait() -> TimeWait {
        use crate::ports::test_support::{bind_engine_v4, bind_plain_v4};

        let (listener, port) = bind_engine_v4().expect("engine-style listener");
        let addr = std::net::SocketAddr::from((Ipv4Addr::LOCALHOST, port.get()));

        // Blocking sockets on purpose: the scenario is a plain accept/close, and
        // it must not need a reactor that the assertion itself does not use.
        let Ok(client) = std::net::TcpStream::connect(addr) else {
            return TimeWait::NotCreated;
        };
        let Ok((accepted, _)) = listener.accept() else {
            return TimeWait::NotCreated;
        };
        // The server closes first. That is what leaves `TIME_WAIT` on the server
        // side, the state a restarted engine has to reuse.
        drop(accepted);
        drop(client);
        drop(listener);

        match bind_plain_v4(port) {
            Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => TimeWait::Created(port),
            // Either a plain bind succeeded (no TIME_WAIT) or it failed for an
            // unrelated reason; in both cases this run proves nothing.
            _ => TimeWait::NotCreated,
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_port_in_time_wait_after_the_server_closed_first_is_free() {
        attempt(|| {
            let port = match port_in_time_wait() {
                TimeWait::Created(port) => port,
                TimeWait::NotCreated => {
                    eprintln!(
                        "skipping: this kernel left no TIME_WAIT entry for the closed \
                         connection, so the probe has nothing to be wrong about here"
                    );
                    return Ok(());
                }
            };
            match probe_bind(port).expect("probe").status().clone() {
                BindStatus::Free => Ok(()),
                BindStatus::InUse { detail } => Err(format!(
                    "{port} is in TIME_WAIT and an engine (SO_REUSEADDR) can bind it, \
                     but the probe reported InUse: {detail}"
                )),
            }
        });
    }

    #[cfg(unix)]
    #[test]
    fn an_active_listener_that_set_reuseaddr_is_still_in_use() {
        // Models an engine that is currently listening: `SO_REUSEADDR` does not
        // let a bind win over an active listener, so the probe must say InUse.
        let (listener, port) =
            crate::ports::test_support::bind_engine_v4().expect("engine-style listener");
        let report = probe_bind(port).expect("probe");
        assert!(
            !report.is_free(),
            "an actively listening engine must read InUse: {report:?}"
        );

        if let Some((v6, v6_port)) = crate::ports::test_support::bind_engine_v6() {
            let report = probe_bind(v6_port).expect("probe");
            assert!(
                !report.is_free(),
                "an actively listening engine on ::1 must read InUse: {report:?}"
            );
            drop(v6);
        }
        drop(listener);
    }
}
