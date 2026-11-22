//! Ports in 20000-29999 only, each verified free on both loopback families.
//!
//! Networks rule (AGENTS.md invariant 9): never bind 5432/3306/3307/6379 or an
//! ephemeral port. This module hands out slots from the documented range.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::{Ipv4Addr, Ipv6Addr, TcpListener};
use std::sync::atomic::{AtomicU16, Ordering};

/// First port handed out.
pub const FIRST: u16 = 20_000;
/// Last port handed out.
pub const LAST: u16 = 29_999;

const ATTEMPTS: u32 = 512;

static NEXT: AtomicU16 = AtomicU16::new(FIRST);

/// Reserves a port that is free on IPv4 and IPv6 loopback.
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
