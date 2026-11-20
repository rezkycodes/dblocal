//! Test-only port reservation for `ports/**` and any future test that needs a
//! real socket.
//!
//! Why not `bind(:0)`: the kernel hands out ephemeral ports (32768-60999 here)
//! from one shared pool and recycles a just-released number immediately. A test
//! that binds `:0`, drops the listener and then asserts the port is free is
//! racing every other test in the binary and every `bind(:0)` caller on the
//! machine; the number can be handed out again between the drop and the
//! assertion. That is exactly how the first full-suite runs of W1-D failed.
//!
//! [`reserve_test_port`] hands out numbers sequentially from 20000-29999 —
//! below the kernel's ephemeral range, so the allocator never gives them to
//! anyone else, and above the privileged range, so every test process can bind
//! them. Each call takes its own slot from an `AtomicU16`, so parallel tests in
//! one binary cannot collide.
//!
//! Two things can still occupy a number in that range: another process that
//! binds it explicitly (this machine has two such listeners) or a second copy of
//! this suite running from another worktree. Neither can be ruled out from a
//! test helper, so [`reserve_test_port`] verifies the slot at hand-out, and
//! [`attempt`] retries an acquire-then-act cycle whose assertion depends on the
//! port still being free. A test therefore asserts the property, never the luck
//! of one particular number.

use std::io::ErrorKind;
use std::net::{Ipv4Addr, Ipv6Addr, TcpListener};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU16, Ordering};

use crate::model::Port;

/// First port the counter hands out.
pub const FIRST_TEST_PORT: u16 = 20_000;
/// Last port the counter hands out.
pub const LAST_TEST_PORT: u16 = 29_999;
/// How many candidates one helper may try before giving up.
const ATTEMPTS: u32 = 64;

static NEXT_PORT: AtomicU16 = AtomicU16::new(FIRST_TEST_PORT);

/// Per-process starting slot, so concurrent copies of this suite walk the range
/// from different points instead of colliding at 20000.
fn start_offset() -> u16 {
    static OFFSET: OnceLock<u16> = OnceLock::new();
    *OFFSET.get_or_init(|| {
        let pid = u64::from(std::process::id());
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| u64::from(elapsed.subsec_micros()))
            .unwrap_or(0);
        let span = u64::from(LAST_TEST_PORT - FIRST_TEST_PORT) + 1;
        u16::try_from((pid.wrapping_mul(7919) ^ nanos) % span).unwrap_or(0)
    })
}

fn next_candidate() -> Port {
    let raw = NEXT_PORT.fetch_add(1, Ordering::Relaxed);
    let span = LAST_TEST_PORT - FIRST_TEST_PORT + 1;
    let slot = (raw - FIRST_TEST_PORT).wrapping_add(start_offset()) % span;
    Port::new(FIRST_TEST_PORT + slot).expect("test range is a valid port range")
}

fn v4_free(port: Port) -> bool {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port.get())).is_ok()
}

/// `true` when `port` can be bound on `::1`, or when this host has no IPv6
/// loopback at all (in which case no IPv6 listener can exist either).
fn v6_free_or_absent(port: Port) -> bool {
    match TcpListener::bind((Ipv6Addr::LOCALHOST, port.get())) {
        Ok(listener) => {
            drop(listener);
            true
        }
        Err(err) => matches!(
            err.kind(),
            ErrorKind::AddrNotAvailable | ErrorKind::Unsupported
        ),
    }
}

/// Reserves a port in 20000-29999 that is free on both loopback families.
///
/// Skips any slot that is already bound, so neither an explicit listener in the
/// range nor a concurrent copy of the suite can make this return a busy port.
pub fn reserve_test_port() -> Port {
    for _ in 0..ATTEMPTS {
        let port = next_candidate();
        if v4_free(port) && v6_free_or_absent(port) {
            return port;
        }
    }
    panic!("no free port in {FIRST_TEST_PORT}-{LAST_TEST_PORT}");
}

/// Binds `127.0.0.1` on a reserved port and keeps the listener alive.
pub fn bind_test_listener() -> (TcpListener, Port) {
    for _ in 0..ATTEMPTS {
        let port = next_candidate();
        if let Ok(listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, port.get())) {
            return (listener, port);
        }
    }
    panic!("no bindable port in {FIRST_TEST_PORT}-{LAST_TEST_PORT}");
}

/// Binds `[::1]` on a reserved port whose `127.0.0.1` side stays free.
///
/// The v4 side is checked *after* the v6 bind as well, because another process
/// could take it in the gap; if that happened the listener is released and the
/// next candidate is tried, so the caller always gets a port with a genuinely
/// free IPv4 side.
///
/// Returns `None` when the host has no IPv6 loopback, so the caller can skip.
pub fn bind_v6_test_listener() -> Option<(TcpListener, Port)> {
    for _ in 0..ATTEMPTS {
        let port = next_candidate();
        if !v4_free(port) {
            continue;
        }
        let Ok(listener) = TcpListener::bind((Ipv6Addr::LOCALHOST, port.get())) else {
            continue;
        };
        if v4_free(port) {
            return Some((listener, port));
        }
        drop(listener);
    }
    None
}

/// Runs `action` until it succeeds, or panics with the last reason it gave.
///
/// Use this for a "release it, then expect it free" check: the closure acquires a
/// reserved port, releases it and performs the assertion. If another process
/// grabs the number in that gap the closure returns `Err` and the next candidate
/// is tried, so the test still asserts the real property. A genuine regression
/// also returns `Err` every time, so the panic message names the last real
/// failure rather than passing silently.
pub fn attempt<T>(mut action: impl FnMut() -> Result<T, String>) -> T {
    let mut last = String::from("no attempt was made");
    for _ in 0..ATTEMPTS {
        match action() {
            Ok(value) => return value,
            Err(reason) => last = reason,
        }
    }
    panic!("gave up after {ATTEMPTS} attempts in {FIRST_TEST_PORT}-{LAST_TEST_PORT}: {last}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_ports_are_in_the_test_range_and_unique() {
        let first = reserve_test_port();
        let second = reserve_test_port();
        for port in [first, second] {
            assert!(
                (FIRST_TEST_PORT..=LAST_TEST_PORT).contains(&port.get()),
                "{port} is outside the reserved test range"
            );
        }
        assert_ne!(first, second, "each call must hand out its own slot");
    }

    #[test]
    fn a_reserved_port_is_bindable() {
        let listener = attempt(|| {
            let port = reserve_test_port();
            TcpListener::bind((Ipv4Addr::LOCALHOST, port.get()))
                .map_err(|err| format!("{port} was taken before we bound it: {err}"))
        });
        drop(listener);
    }

    #[test]
    fn a_bound_listener_holds_the_port() {
        let (listener, port) = bind_test_listener();
        assert!(
            TcpListener::bind((Ipv4Addr::LOCALHOST, port.get())).is_err(),
            "the returned listener must actually hold the port"
        );
        drop(listener);
    }

    #[test]
    fn a_released_port_can_be_taken_again() {
        let listener = attempt(|| {
            let (held, port) = bind_test_listener();
            drop(held);
            TcpListener::bind((Ipv4Addr::LOCALHOST, port.get()))
                .map_err(|err| format!("{port} not bindable after release: {err}"))
        });
        drop(listener);
    }

    #[test]
    fn a_v6_listener_leaves_the_v4_side_free() {
        // Retried: another process on the machine can take the v4 side of this
        // number in the gap between the helper's own check and the assertion.
        attempt(|| {
            let Some((listener, port)) = bind_v6_test_listener() else {
                return Ok(());
            };
            let v4 = TcpListener::bind((Ipv4Addr::LOCALHOST, port.get()));
            drop(listener);
            v4.map(drop)
                .map_err(|err| format!("v4 side of {port} was taken while `::1` held it: {err}"))
        });
    }
}
