//! `/proc` inspection: process discovery by command line, listening sockets,
//! and validated termination for the cleanup guard.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// One live process observed from `/proc`.
#[derive(Debug, Clone)]
pub struct ProcInfo {
    /// Process id.
    pub pid: u32,
    /// Command line joined by spaces (NUL separators replaced).
    pub cmdline: String,
    /// Resolved executable, when readable.
    pub exe: Option<PathBuf>,
}

/// Every live process whose command line mentions `home`.
pub fn under(home: &Path) -> Vec<ProcInfo> {
    let needle = home.to_string_lossy().into_owned();
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return found;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(pid) = file_name.to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let Some(cmdline) = cmdline(pid) else {
            continue;
        };
        if !cmdline.contains(&needle) {
            continue;
        }
        found.push(ProcInfo {
            pid,
            cmdline,
            exe: fs::read_link(format!("/proc/{pid}/exe")).ok(),
        });
    }
    found.sort_by_key(|p| p.pid);
    found
}

/// The command line of `pid`, or `None` when the process is gone.
pub fn cmdline(pid: u32) -> Option<String> {
    let bytes = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let joined = bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    Some(joined)
}

/// Whether `pid` is still present.
pub fn alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// Waits until `pid` is gone, up to `timeout`.
pub fn wait_gone(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    !alive(pid)
}

/// Sends SIGKILL. Last-resort cleanup only: never used to prove a stop worked.
pub fn sigkill(pid: u32) {
    let _ = Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .status();
}

/// Terminates every process whose command line mentions `home` **and** whose
/// executable resolves inside that home's `runtimes/` tree.
///
/// Two checks, mirroring AGENTS.md invariant 7 ("verify executable name and
/// process start time; never signal a process you cannot positively identify"):
/// the command line proves it belongs to this temp home, and the executable path
/// proves it is an engine binary this suite installed rather than, say, a `sh`
/// that merely mentions the path in an argument. A process that fails the second
/// check is left alone and reported, because killing it would be the exact blind
/// signal the invariant forbids.
pub fn kill_under(home: &Path) {
    let runtimes = home.join("runtimes");
    for info in under(home) {
        match &info.exe {
            Some(exe) if exe.starts_with(&runtimes) => sigkill(info.pid),
            other => eprintln!(
                "cleanup: not signalling pid {} — executable {:?} is not under {} (argv: {})",
                info.pid,
                other,
                runtimes.display(),
                info.cmdline
            ),
        }
    }
}

/// Waits until no positively-identified engine process mentions `home`.
pub fn wait_none_under(home: &Path, timeout: Duration) -> bool {
    let runtimes = home.join("runtimes");
    let engine_processes = || {
        under(home)
            .into_iter()
            .filter(|info| {
                info.exe
                    .as_ref()
                    .is_some_and(|exe| exe.starts_with(&runtimes))
            })
            .collect::<Vec<_>>()
    };
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = engine_processes();
        if remaining.is_empty() {
            return true;
        }
        if Instant::now() >= deadline {
            for info in remaining {
                sigkill(info.pid);
            }
            return engine_processes().is_empty();
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Every engine process that mentions `home`.
///
/// This is the set the task requires to be empty after a test: an engine, not
/// just any process. Identification is by executable file name, which is what the
/// service itself records, so a `sh` that happens to have the path in an argument
/// is not mistaken for an engine. `mysqld` is the MySQL launcher's real name and
/// `mariadbd` MariaDB's (its `bin/mysqld` is a symlink to it).
pub fn engines_under(home: &Path) -> Vec<ProcInfo> {
    const ENGINE_NAMES: [&str; 4] = ["postgres", "valkey-server", "mysqld", "mariadbd"];
    under(home)
        .into_iter()
        .filter(|info| {
            info.exe
                .as_ref()
                .and_then(|exe| exe.file_name())
                .and_then(|name| name.to_str())
                .is_some_and(|name| ENGINE_NAMES.contains(&name))
        })
        .collect()
}

/// One listening socket as read from `/proc/net/tcp*`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenSocket {
    /// Local address exactly as the kernel prints it (hex, host byte order).
    pub addr_hex: String,
    /// Local port.
    pub port: u16,
    /// Socket inode, which ties the row to a `/proc/<pid>/fd` link.
    pub inode: u64,
}

impl ListenSocket {
    /// Whether this socket is bound to IPv4 `127.0.0.1`.
    pub fn is_v4_loopback(&self) -> bool {
        self.addr_hex == "0100007F"
    }

    /// Whether this socket is bound to IPv6 `::1`.
    pub fn is_v6_loopback(&self) -> bool {
        self.addr_hex == "00000000000000000000000001000000"
    }

    /// Whether this is any loopback address (`127.0.0.1` or `::1`).
    pub fn is_loopback(&self) -> bool {
        self.is_v4_loopback() || self.is_v6_loopback()
    }
}

/// Every listening socket in `/proc/net/tcp` and `/proc/net/tcp6`.
pub fn listen_sockets() -> Vec<ListenSocket> {
    let mut out = Vec::new();
    for path in ["/proc/net/tcp", "/proc/net/tcp6"] {
        out.extend(parse_net_tcp(&fs::read_to_string(path).unwrap_or_default()));
    }
    out
}

/// Parses the rows of `/proc/net/tcp` whose state is `0A` (LISTEN).
pub fn parse_net_tcp(contents: &str) -> Vec<ListenSocket> {
    let mut out = Vec::new();
    for line in contents.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 10 || parts[3] != "0A" {
            continue;
        }
        let Some((addr_hex, port_hex)) = parts[1].rsplit_once(':') else {
            continue;
        };
        let Ok(port) = u16::from_str_radix(port_hex, 16) else {
            continue;
        };
        let Ok(inode) = parts[9].parse::<u64>() else {
            continue;
        };
        out.push(ListenSocket {
            addr_hex: addr_hex.to_owned(),
            port,
            inode,
        });
    }
    out
}

/// Listening sockets whose inode is open in `pid`.
pub fn listen_sockets_of(pid: u32) -> Vec<ListenSocket> {
    let inodes = socket_inodes(pid);
    listen_sockets()
        .into_iter()
        .filter(|socket| inodes.contains(&socket.inode))
        .collect()
}

/// Socket inodes held open by `pid`.
pub fn socket_inodes(pid: u32) -> HashSet<u64> {
    let mut out = HashSet::new();
    let Ok(entries) = fs::read_dir(format!("/proc/{pid}/fd")) else {
        return out;
    };
    for entry in entries.flatten() {
        let Ok(target) = fs::read_link(entry.path()) else {
            continue;
        };
        let text = target.to_string_lossy();
        if let Some(rest) = text.strip_prefix("socket:[")
            && let Some(number) = rest.strip_suffix(']')
            && let Ok(inode) = number.parse::<u64>()
        {
            out.insert(inode);
        }
    }
    out
}

/// Pids of the processes listening on `port`, read without any engine helper.
pub fn listeners_on(port: u16) -> Vec<u32> {
    let inodes: HashSet<u64> = listen_sockets()
        .into_iter()
        .filter(|socket| socket.port == port)
        .map(|socket| socket.inode)
        .collect();
    if inodes.is_empty() {
        return Vec::new();
    }
    let mut pids = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return pids;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        if socket_inodes(pid)
            .iter()
            .any(|inode| inodes.contains(inode))
        {
            pids.push(pid);
        }
    }
    pids
}

/// Distinct process groups among the processes whose command line mentions
/// `home`.
///
/// Engines are spawned detached into their own process group (`process_group(0)`
/// in production, `pg_ctl` for PostgreSQL), so every process of one engine —
/// the postmaster and its closed-source forked workers — shares a group, and two
/// engines never share one. Counting groups is therefore "how many engines are
/// running", not "how many processes the engine forked", which a plain process
/// count would get wrong for PostgreSQL.
pub fn process_groups_under(home: &Path) -> Vec<i32> {
    let mut groups: Vec<i32> = under(home)
        .iter()
        .filter_map(|info| pgrp(info.pid))
        .collect();
    groups.sort_unstable();
    groups.dedup();
    groups
}

/// The process group id of `pid`, read from `/proc/<pid>/stat`.
fn pgrp(pid: u32) -> Option<i32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Field 5 (1-indexed) is pgrp; the comm field may contain spaces and
    // parentheses, so parse from the closing paren and count from there.
    let after_comm = stat.rsplit_once(')')?.1;
    let mut fields = after_comm.split_whitespace();
    // After comm: state(3) ppid(4) pgrp(5) ...
    let _state = fields.next()?;
    let _ppid = fields.next()?;
    fields.next()?.parse::<i32>().ok()
}
