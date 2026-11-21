//! Ports in 20000-29999 only, allocated sequentially and verified free.
//!
//! The tests never bind 5432/3306/3307/6379, never use an ephemeral port and
//! never rely on `bind(:0)` for an engine's port.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::{Ipv4Addr, Ipv6Addr, TcpListener};
use std::sync::atomic::{AtomicU16, Ordering};

/// First port handed out.
pub const FIRST: u16 = 20_000;
/// Last port handed out.
pub const LAST: u16 = 29_999;

const ATTEMPTS: u32 = 256;

static NEXT: AtomicU16 = AtomicU16::new(FIRST);

/// Reserves a port in the test range that is free on both loopback families.
pub fn reserve() -> u16 {
    for _ in 0..ATTEMPTS {
        let raw = NEXT.fetch_add(1, Ordering::Relaxed);
        let span = LAST - FIRST + 1;
        let port = FIRST + ((raw - FIRST) % span);
        if v4_free(port) && v6_free_or_absent(port) {
            return port;
        }
    }
    panic!("no free port in {FIRST}-{LAST}");
}

fn v4_free(port: u16) -> bool {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok()
}

fn v6_free_or_absent(port: u16) -> bool {
    match TcpListener::bind((Ipv6Addr::LOCALHOST, port)) {
        Ok(listener) => {
            drop(listener);
            true
        }
        Err(err) => matches!(
            err.kind(),
            std::io::ErrorKind::AddrNotAvailable | std::io::ErrorKind::Unsupported
        ),
    }
}

/// A real listener held on `127.0.0.1:<port>` for the lifetime of the value.
pub struct HeldPort {
    _listener: TcpListener,
    port: u16,
}

impl HeldPort {
    /// Port the listener occupies.
    pub fn port(&self) -> u16 {
        self.port
    }
}

/// Reserves a free port and immediately holds a listener on it.
pub fn hold_reserved() -> HeldPort {
    loop {
        let port = reserve();
        if let Ok(listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
            return HeldPort {
                _listener: listener,
                port,
            };
        }
    }
}

/// Holds `127.0.0.1:<port>`, panicking with the reason on failure.
pub fn hold(port: u16) -> HeldPort {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .unwrap_or_else(|err| panic!("could not hold port {port}: {err}"));
    HeldPort {
        _listener: listener,
        port,
    }
}
