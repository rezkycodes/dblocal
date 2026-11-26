//! Defence-in-depth Linux listening-socket loopback guard for RustFS.
//!
//! After RustFS reports ready, DBLocal inspects `/proc/<pid>/fd` socket inodes
//! against `/proc/net/tcp` and `/proc/net/tcp6` (state `0A` = `TCP_LISTEN`).
//! If any listening socket owned by the RustFS process binds anything other
//! than `127.0.0.1` or `::1`, start fails and the server is stopped.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::DblError;
use crate::error::Result;

/// One listening TCP socket decoded from `/proc/net/tcp` or `/proc/net/tcp6`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListeningSocket {
    /// Bound local IP address.
    pub addr: IpAddr,
    /// Bound local TCP port.
    pub port: u16,
}

impl ListeningSocket {
    /// Formats `addr:port` (bracketed for IPv6) for diagnostics.
    pub fn display_addr(&self) -> String {
        match self.addr {
            IpAddr::V4(v4) => format!("{v4}:{}", self.port),
            IpAddr::V6(v6) => format!("[{v6}]:{}", self.port),
        }
    }
}

/// Extracts the socket inode number from a `/proc/<pid>/fd/*` symlink target
/// such as `socket:[12345]`.
pub fn parse_socket_inode(link_target: &str) -> Option<u64> {
    let rest = link_target.strip_prefix("socket:[")?.strip_suffix(']')?;
    rest.parse::<u64>().ok()
}

/// Parses `/proc/net/tcp` (`is_v6 = false`) or `/proc/net/tcp6` (`is_v6 = true`)
/// and returns every `TCP_LISTEN` (`0A`) socket whose inode is in `owned_inodes`.
pub fn parse_proc_net_tcp(
    contents: &str,
    is_v6: bool,
    owned_inodes: &HashSet<u64>,
) -> Vec<ListeningSocket> {
    let mut out = Vec::new();
    for line in contents.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 10 {
            continue;
        }
        // State 0A is TCP_LISTEN.
        if !fields[3].eq_ignore_ascii_case("0A") {
            continue;
        }
        let Ok(inode) = fields[9].parse::<u64>() else {
            continue;
        };
        if !owned_inodes.contains(&inode) {
            continue;
        }
        let Some((ip_hex, port_hex)) = fields[1].split_once(':') else {
            continue;
        };
        let Ok(port) = u16::from_str_radix(port_hex, 16) else {
            continue;
        };
        let addr = if is_v6 {
            decode_ipv6_hex(ip_hex).map(IpAddr::V6)
        } else {
            decode_ipv4_hex(ip_hex).map(IpAddr::V4)
        };
        if let Some(addr) = addr {
            out.push(ListeningSocket { addr, port });
        }
    }
    out
}

/// Decodes an 8-hex-digit little-endian IPv4 address from `/proc/net/tcp`.
fn decode_ipv4_hex(hex: &str) -> Option<Ipv4Addr> {
    if hex.len() != 8 {
        return None;
    }
    let raw = u32::from_str_radix(hex, 16).ok()?;
    let [a, b, c, d] = raw.to_le_bytes();
    Some(Ipv4Addr::new(a, b, c, d))
}

/// Decodes a 32-hex-digit IPv6 address (four little-endian 32-bit words) from `/proc/net/tcp6`.
fn decode_ipv6_hex(hex: &str) -> Option<Ipv6Addr> {
    if hex.len() != 32 {
        return None;
    }
    let mut octets = [0u8; 16];
    for i in 0..4 {
        let chunk = &hex[i * 8..(i + 1) * 8];
        let word = u32::from_str_radix(chunk, 16).ok()?;
        let bytes = word.to_le_bytes();
        octets[i * 4..i * 4 + 4].copy_from_slice(&bytes);
    }
    Some(Ipv6Addr::from(octets))
}

/// Verifies that every listening socket is strictly on `127.0.0.1` or `::1`.
pub fn check_sockets_loopback(sockets: &[ListeningSocket]) -> Result<()> {
    for sock in sockets {
        let is_loopback = matches!(
            sock.addr,
            IpAddr::V4(v4) if v4 == Ipv4Addr::LOCALHOST
        ) || matches!(
            sock.addr,
            IpAddr::V6(v6) if v6 == Ipv6Addr::LOCALHOST
        );
        if !is_loopback {
            return Err(DblError::Engine {
                reason: format!(
                    "RustFS opened a non-loopback listening socket on {} (only 127.0.0.1 or ::1 is permitted)",
                    sock.display_addr()
                ),
            });
        }
    }
    Ok(())
}

/// Reads all listening TCP sockets owned by `pid` on Linux and rejects any non-loopback bind.
#[cfg(target_os = "linux")]
pub fn verify_process_loopback_only(pid: u32) -> Result<()> {
    let fd_dir = format!("/proc/{pid}/fd");
    let entries = std::fs::read_dir(&fd_dir).map_err(|source| DblError::Io { source })?;
    let mut inodes = HashSet::new();
    for entry in entries.flatten() {
        if let Ok(target) = std::fs::read_link(entry.path())
            && let Some(text) = target.to_str()
            && let Some(inode) = parse_socket_inode(text)
        {
            inodes.insert(inode);
        }
    }

    let mut sockets = Vec::new();
    if let Ok(tcp) = std::fs::read_to_string("/proc/net/tcp") {
        sockets.extend(parse_proc_net_tcp(&tcp, false, &inodes));
    }
    if let Ok(tcp6) = std::fs::read_to_string("/proc/net/tcp6") {
        sockets.extend(parse_proc_net_tcp(&tcp6, true, &inodes));
    }

    if let Err(err) = check_sockets_loopback(&sockets) {
        tracing::error!(
            pid,
            error = %err,
            "RustFS defence-in-depth check failed: non-loopback listening socket detected"
        );
        return Err(err);
    }
    Ok(())
}

/// No-op on non-Linux platforms (Linux `/proc/net/tcp{,6}` guard only).
#[cfg(not(target_os = "linux"))]
pub fn verify_process_loopback_only(_pid: u32) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    const TCP_HEADER: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n";

    #[test]
    fn parses_socket_inodes_and_ignores_non_sockets() {
        assert_eq!(parse_socket_inode("socket:[123456]"), Some(123456));
        assert_eq!(parse_socket_inode("pipe:[999]"), None);
        assert_eq!(parse_socket_inode("/dev/null"), None);
    }

    #[test]
    fn accepts_loopback_only_console_off_and_console_on() {
        // Console OFF: only 127.0.0.1:9000 (0x2328)
        let tcp_off = format!(
            "{TCP_HEADER}   0: 0100007F:2328 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 50001 1 0000000000000000 100 0 0 10 0\n"
        );
        let inodes: HashSet<u64> = [50001].into_iter().collect();
        let socks = parse_proc_net_tcp(&tcp_off, false, &inodes);
        assert_eq!(
            socks,
            vec![ListeningSocket {
                addr: IpAddr::V4(Ipv4Addr::LOCALHOST),
                port: 9000,
            }]
        );
        assert!(check_sockets_loopback(&socks).is_ok());

        // Console ON: 127.0.0.1:9000 (0x2328) and 127.0.0.1:9001 (0x2329)
        let tcp_on = format!(
            "{tcp_off}   1: 0100007F:2329 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 50002 1 0000000000000000 100 0 0 10 0\n"
        );
        let inodes_on: HashSet<u64> = [50001, 50002].into_iter().collect();
        let socks_on = parse_proc_net_tcp(&tcp_on, false, &inodes_on);
        assert_eq!(socks_on.len(), 2);
        assert!(check_sockets_loopback(&socks_on).is_ok());
    }

    #[test]
    fn rejects_wildcard_ipv4_listener() {
        let tcp = format!(
            "{TCP_HEADER}   0: 00000000:2329 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 60001 1 0000000000000000 100 0 0 10 0\n"
        );
        let inodes: HashSet<u64> = [60001].into_iter().collect();
        let socks = parse_proc_net_tcp(&tcp, false, &inodes);
        let err = check_sockets_loopback(&socks).expect_err("0.0.0.0:9001 must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("0.0.0.0:9001"), "{msg}");
    }

    #[test]
    fn rejects_wildcard_ipv6_listener_and_accepts_ipv6_loopback() {
        let tcp6 = format!(
            "{TCP_HEADER}\
             0: 00000000000000000000000000000000:2329 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 70001 1 0000000000000000 100 0 0 10 0\n\
             1: 00000000000000000000000001000000:2328 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 70002 1 0000000000000000 100 0 0 10 0\n"
        );
        let ok_inodes: HashSet<u64> = [70002].into_iter().collect();
        let ok_socks = parse_proc_net_tcp(&tcp6, true, &ok_inodes);
        assert_eq!(
            ok_socks,
            vec![ListeningSocket {
                addr: IpAddr::V6(Ipv6Addr::LOCALHOST),
                port: 9000,
            }]
        );
        assert!(check_sockets_loopback(&ok_socks).is_ok());

        let bad_inodes: HashSet<u64> = [70001, 70002].into_iter().collect();
        let bad_socks = parse_proc_net_tcp(&tcp6, true, &bad_inodes);
        let err = check_sockets_loopback(&bad_socks).expect_err("[::]:9001 must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("[::]:9001"), "{msg}");
    }

    #[test]
    fn ignores_non_listening_and_unowned_sockets() {
        let tcp = format!(
            "{TCP_HEADER}\
             0: 00000000:2329 0100007F:9999 01 00000000:00000000 00:00000000 00000000  1000        0 80001 1 0000000000000000 100 0 0 10 0\n\
             1: 00000000:2329 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 99999 1 0000000000000000 100 0 0 10 0\n"
        );
        let inodes: HashSet<u64> = [80001].into_iter().collect();
        let socks = parse_proc_net_tcp(&tcp, false, &inodes);
        assert!(socks.is_empty());
    }
}
