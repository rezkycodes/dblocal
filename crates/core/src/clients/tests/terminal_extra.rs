use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::clients::{HostProbe, OsKind, TerminalKind, resolve_terminal};

use super::super::terminal::{
    current_uid, linux_launcher_dir_from, macos_launcher_dir_from, resolve_linux_terminal_with,
    terminal_exists, windows_launcher_dir_from,
};
use crate::clients::ClientError;

#[derive(Default)]
struct FakeHost {
    bins: Vec<(String, PathBuf)>,
    paths: Vec<PathBuf>,
}

impl FakeHost {
    fn with_bin(mut self, n: &str, p: &str) -> Self {
        self.bins.push((n.into(), PathBuf::from(p)));
        self
    }
    fn with_path(mut self, p: &str) -> Self {
        self.paths.push(PathBuf::from(p));
        self
    }
}
impl HostProbe for FakeHost {
    fn which(&self, name: &str) -> Option<PathBuf> {
        self.bins
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, p)| p.clone())
    }
    fn path_exists(&self, path: &Path) -> bool {
        self.paths.iter().any(|p| p == path)
    }
    fn desktop_entries(&self) -> Vec<String> {
        vec![]
    }
    fn macos_applications(&self) -> Vec<String> {
        vec![]
    }
    fn windows_app_paths(&self) -> Vec<String> {
        vec![]
    }
}

#[test]
fn macos_terminal_is_terminal_app() {
    let t = resolve_terminal(&FakeHost::default(), OsKind::Macos).unwrap();
    assert_eq!(t.kind(), TerminalKind::MacOsTerminalApp);
}

#[test]
fn windows_terminal_wt_else_cmd() {
    let wt = resolve_terminal(
        &FakeHost::default().with_bin("wt.exe", "C:/wt.exe"),
        OsKind::Windows,
    )
    .unwrap();
    assert_eq!(wt.kind(), TerminalKind::WindowsTerminal);
    let cmd = resolve_terminal(&FakeHost::default(), OsKind::Windows).unwrap();
    assert_eq!(cmd.kind(), TerminalKind::WindowsCmd);
}

#[test]
fn terminal_env_with_absolute_path_is_used_when_it_exists() {
    let host = FakeHost::default().with_path("/opt/term/bin/t");
    let choice =
        resolve_linux_terminal_with(&host, Some(OsString::from("/opt/term/bin/t"))).unwrap();
    assert_eq!(choice.kind(), TerminalKind::GenericArgv);
    assert_eq!(choice.program(), &PathBuf::from("/opt/term/bin/t"));
}

#[test]
fn terminal_env_bare_name_is_resolved_through_which() {
    let host = FakeHost::default().with_bin("myterm", "/usr/bin/myterm");
    let choice = resolve_linux_terminal_with(&host, Some(OsString::from("myterm"))).unwrap();
    // A bare name is trusted only after `which` finds it; the program stays the
    // bare name and the child resolves it through PATH at exec time.
    assert_eq!(choice.program(), &PathBuf::from("myterm"));
    assert_eq!(choice.kind(), TerminalKind::GenericArgv);
}

#[test]
fn empty_or_missing_terminal_env_falls_through_to_candidates() {
    let host = FakeHost::default().with_bin("ptyxis", "/usr/bin/ptyxis");
    let choice = resolve_linux_terminal_with(&host, Some(OsString::new())).unwrap();
    assert_eq!(choice.kind(), TerminalKind::Ptyxis);
    let choice = resolve_linux_terminal_with(&host, None).unwrap();
    assert_eq!(choice.kind(), TerminalKind::Ptyxis);
}

#[test]
fn no_terminal_at_all_is_typed_error() {
    let err = resolve_linux_terminal_with(&FakeHost::default(), None).unwrap_err();
    assert!(matches!(err, ClientError::NoTerminal));
}

#[test]
fn terminal_exists_branch_reflects_components() {
    let host = FakeHost::default()
        .with_path("/opt/x")
        .with_bin("x", "/bin/x");
    assert!(terminal_exists(&host, Path::new("/opt/x")));
    assert!(!terminal_exists(&host, Path::new("/opt/missing")));
    assert!(terminal_exists(&host, Path::new("x")));
}

#[test]
fn linux_launcher_dir_uses_xdg_then_home() {
    let xdg = linux_launcher_dir_from(
        Some(OsString::from("/run/user/1")),
        Some(OsString::from("/home/u")),
    )
    .unwrap();
    assert_eq!(xdg, PathBuf::from("/run/user/1/dblocal"));
    let home = linux_launcher_dir_from(None, Some(OsString::from("/home/u"))).unwrap();
    assert_eq!(home, PathBuf::from("/home/u/.dblocal/run"));
    let empty_xdg =
        linux_launcher_dir_from(Some(OsString::new()), Some(OsString::from("/h"))).unwrap();
    assert_eq!(empty_xdg, PathBuf::from("/h/.dblocal/run"));
    let missing = linux_launcher_dir_from(None, None);
    assert!(matches!(missing, Err(ClientError::ScriptError { .. })));
}

#[test]
fn macos_launcher_dir_defaults_to_tmp() {
    let with = macos_launcher_dir_from(Some(OsString::from("/var/t"))).unwrap();
    assert_eq!(with, PathBuf::from("/var/t/dblocal"));
    let default = macos_launcher_dir_from(None).unwrap();
    assert_eq!(default, PathBuf::from("/tmp/dblocal"));
}

#[test]
fn windows_launcher_dir_prefers_local_app_data() {
    let local = windows_launcher_dir_from(
        Some(OsString::from("C:/LAD")),
        Some(OsString::from("C:/UP")),
    )
    .unwrap();
    assert_eq!(local, PathBuf::from("C:/LAD/dblocal/run"));
    let profile = windows_launcher_dir_from(None, Some(OsString::from("C:/UP"))).unwrap();
    assert_eq!(profile, PathBuf::from("C:/UP/dblocal/run"));
    let missing = windows_launcher_dir_from(None, None);
    assert!(matches!(missing, Err(ClientError::ScriptError { .. })));
}

#[test]
fn current_uid_is_a_real_uid() {
    assert_ne!(current_uid(), u32::MAX);
}
