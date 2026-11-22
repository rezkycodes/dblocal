//! Process identity verification and SIGTERM shutdown for MariaDB.

use std::fs;
use std::path::{Path, PathBuf};
#[cfg(not(target_os = "linux"))]
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::consts;
use crate::engine::RunContext;
use crate::engine::uri::exe_name;
use crate::error::{DblError, Result};

use super::{config_path, pidfile_path};

pub(super) fn shutdown_by_sigterm(ctx: &RunContext) -> Result<()> {
    let pid_path = pidfile_path(&ctx.data_dir);
    let pid = match read_pidfile(&pid_path) {
        Ok(pid) => pid,
        Err(e) => return Err(e),
    };
    let identity = expected_identity(ctx);
    match verify_pid_identity(pid, &identity) {
        PidIdentity::Match => {}
        PidIdentity::NoSuchProcess => {
            tracing::info!(pid, "mariadb pid already gone");
            return Ok(());
        }
        PidIdentity::NotOurs(reason) => {
            tracing::warn!(
                pid,
                reason = %reason,
                "mariadb pidfile pid is not our mariadbd; refusing to signal"
            );
            return Ok(());
        }
        PidIdentity::Unverifiable(reason) => {
            tracing::warn!(
                pid,
                reason = %reason,
                "mariadb pid identity unverifiable; refusing to signal"
            );
            return Ok(());
        }
    }

    let start = Instant::now();
    let grace = Duration::from_secs(consts::STOP_GRACE_SECS);

    send_term(pid)?;

    while start.elapsed() < grace {
        if !pid_alive(pid) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    if !pid_alive(pid) {
        return Ok(());
    }
    Err(DblError::Engine {
        reason: format!(
            "mariadbd pid {pid} did not exit within {}s after SIGTERM",
            consts::STOP_GRACE_SECS
        ),
    })
}

pub(super) fn read_pidfile(path: &Path) -> Result<u32> {
    let contents = fs::read_to_string(path).map_err(|e| DblError::Engine {
        reason: format!("cannot read pidfile {}: {e}", path.display()),
    })?;
    let pid = contents
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .ok_or_else(|| DblError::Engine {
            reason: format!("pidfile {} is empty", path.display()),
        })?
        .parse::<u32>()
        .map_err(|_| DblError::Engine {
            reason: format!("pidfile {} does not contain a pid", path.display()),
        })?;
    Ok(pid)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExpectedIdentity {
    pub(crate) exe: PathBuf,
    pub(crate) data_dir: PathBuf,
    pub(crate) config_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PidIdentity {
    Match,
    NoSuchProcess,
    NotOurs(String),
    Unverifiable(String),
}

pub(crate) fn expected_identity(ctx: &RunContext) -> ExpectedIdentity {
    ExpectedIdentity {
        exe: ctx.runtime_dir.join("bin").join(exe_name("mariadbd")),
        data_dir: ctx.data_dir.clone(),
        config_path: config_path(&ctx.data_dir),
    }
}

pub(crate) fn verify_pid_identity(pid: u32, expected: &ExpectedIdentity) -> PidIdentity {
    #[cfg(target_os = "linux")]
    {
        verify_pid_identity_linux(pid, expected)
    }
    #[cfg(not(target_os = "linux"))]
    {
        verify_pid_identity_fallback(pid, expected)
    }
}

#[cfg(target_os = "linux")]
fn verify_pid_identity_linux(pid: u32, expected: &ExpectedIdentity) -> PidIdentity {
    let proc_dir = PathBuf::from(format!("/proc/{pid}"));
    if !proc_dir.exists() {
        return PidIdentity::NoSuchProcess;
    }
    if is_zombie(pid) {
        return PidIdentity::NoSuchProcess;
    }
    let exe_link = proc_dir.join("exe");
    let observed_exe = match fs::read_link(&exe_link) {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return PidIdentity::NoSuchProcess,
        Err(e) => return PidIdentity::Unverifiable(format!("cannot read exe: {e}")),
    };
    if !same_executable(&observed_exe, &expected.exe) {
        return PidIdentity::NotOurs(format!(
            "exe {} != {}",
            observed_exe.display(),
            expected.exe.display()
        ));
    }
    let cmdline_path = proc_dir.join("cmdline");
    let cmdline_bytes = match fs::read(&cmdline_path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return PidIdentity::NoSuchProcess,
        Err(e) => return PidIdentity::Unverifiable(format!("cannot read cmdline: {e}")),
    };
    let args = parse_cmdline(&cmdline_bytes);
    if !cmdline_targets_instance(&args, expected) {
        return PidIdentity::NotOurs("cmdline does not target this instance".to_owned());
    }
    PidIdentity::Match
}

#[cfg(not(target_os = "linux"))]
fn verify_pid_identity_fallback(pid: u32, expected: &ExpectedIdentity) -> PidIdentity {
    if !pid_alive(pid) {
        return PidIdentity::NoSuchProcess;
    }
    // On macOS/BSD, /proc is not available. Use ps -p <pid> -o command= as a
    // best-effort identity check; this is weaker than the Linux /proc check and
    // is documented as unverified on macOS in the architecture notes.
    let out = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    let Ok(out) = out else {
        return PidIdentity::Unverifiable("ps failed".to_owned());
    };
    if !out.status.success() {
        return PidIdentity::NoSuchProcess;
    }
    let cmd = String::from_utf8_lossy(&out.stdout);
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return PidIdentity::NoSuchProcess;
    }
    let args: Vec<String> = cmd.split_whitespace().map(|s| s.to_owned()).collect();
    let Some(exe_arg) = args.first() else {
        return PidIdentity::Unverifiable("empty ps output".to_owned());
    };
    let observed_exe = PathBuf::from(exe_arg);
    if !same_executable(&observed_exe, &expected.exe) {
        return PidIdentity::NotOurs(format!(
            "exe {} != {}",
            observed_exe.display(),
            expected.exe.display()
        ));
    }
    if !cmdline_targets_instance(&args, expected) {
        return PidIdentity::NotOurs("cmdline does not target this instance".to_owned());
    }
    PidIdentity::Match
}

pub(super) fn same_executable(observed: &Path, expected: &Path) -> bool {
    let observed = strip_deleted(observed);
    let expected = strip_deleted(expected);
    if observed == expected {
        return true;
    }
    let obs_name = observed.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let exp_name = expected.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if obs_name != exp_name || obs_name.is_empty() {
        return false;
    }
    // Canonicalize when possible; if either fails, fall back to name match only
    // when the observed path ends with the expected suffix.
    match (observed.canonicalize(), expected.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => observed.ends_with(expected.as_ref()) || expected.ends_with(observed.as_ref()),
    }
}

fn strip_deleted(path: &Path) -> std::borrow::Cow<'_, Path> {
    #[cfg(unix)]
    {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        const SUFFIX: &[u8] = b" (deleted)";
        let bytes = path.as_os_str().as_bytes();
        if let Some(stripped) = bytes.strip_suffix(SUFFIX) {
            return std::borrow::Cow::Owned(PathBuf::from(OsStr::from_bytes(stripped)));
        }
    }
    std::borrow::Cow::Borrowed(path)
}

pub(crate) fn parse_cmdline(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

pub(crate) fn cmdline_targets_instance(args: &[String], expected: &ExpectedIdentity) -> bool {
    let datadir = expected.data_dir.to_string_lossy();
    let conf = expected.config_path.to_string_lossy();
    let mut saw_datadir = false;
    let mut saw_conf = false;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if let Some(v) = a.strip_prefix("--datadir=") {
            saw_datadir = path_arg_matches(v, &datadir);
        } else if a == "--datadir" {
            if let Some(v) = args.get(i + 1) {
                saw_datadir = path_arg_matches(v, &datadir);
            }
        } else if let Some(v) = a.strip_prefix("--defaults-file=") {
            saw_conf = path_arg_matches(v, &conf);
        } else if a == "--defaults-file" {
            if let Some(v) = args.get(i + 1) {
                saw_conf = path_arg_matches(v, &conf);
            }
        }
        i += 1;
    }
    saw_datadir || saw_conf
}

fn path_arg_matches(arg: &str, expected: &str) -> bool {
    if arg == expected {
        return true;
    }
    let a = Path::new(arg);
    let b = Path::new(expected);
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

pub(crate) fn is_zombie_stat(stat: &str) -> bool {
    let Some(close) = stat.rfind(')') else {
        return false;
    };
    let rest = &stat[close + 1..];
    let mut parts = rest.split_whitespace();
    matches!(parts.next(), Some("Z"))
}

#[cfg(target_os = "linux")]
pub(super) fn is_zombie(pid: u32) -> bool {
    let stat_path = format!("/proc/{pid}/stat");
    let Ok(stat) = fs::read_to_string(stat_path) else {
        return false;
    };
    is_zombie_stat(&stat)
}

#[cfg(unix)]
fn send_term(pid: u32) -> Result<()> {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    match kill(Pid::from_raw(pid as i32), Signal::SIGTERM) {
        Ok(()) => Ok(()),
        Err(nix::errno::Errno::ESRCH) => Ok(()),
        Err(e) => Err(DblError::Engine {
            reason: format!("failed to send SIGTERM to mariadbd pid {pid}: {e}"),
        }),
    }
}

#[cfg(not(unix))]
fn send_term(pid: u32) -> Result<()> {
    let _ = pid;
    Err(DblError::Unsupported {
        capability: "mariadb SIGTERM shutdown",
        reason: "not implemented on this platform".to_owned(),
    })
}

#[cfg(unix)]
pub(super) fn pid_alive(pid: u32) -> bool {
    use nix::sys::signal::kill;
    use nix::unistd::Pid;
    #[cfg(target_os = "linux")]
    if is_zombie(pid) {
        return false;
    }
    match kill(Pid::from_raw(pid as i32), None) {
        Ok(()) => true,
        Err(nix::errno::Errno::EPERM) => true, // exists but no permission; treat as alive
        Err(_) => false,
    }
}

#[cfg(not(unix))]
pub(super) fn pid_alive(pid: u32) -> bool {
    let _ = pid;
    false
}
