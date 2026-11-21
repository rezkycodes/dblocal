use std::path::PathBuf;

use crate::clients::{HostProbe, OsKind, TerminalKind, resolve_terminal};

#[derive(Default)]
struct FakeHost {
    bins: Vec<(String, PathBuf)>,
}
impl FakeHost {
    fn with(mut self, n: &str, p: &str) -> Self {
        self.bins.push((n.into(), p.into()));
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
    fn path_exists(&self, _: &std::path::Path) -> bool {
        false
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
fn resolve_macos_terminal() {
    let t = resolve_terminal(&FakeHost::default(), OsKind::Macos).unwrap();
    assert_eq!(t.kind(), TerminalKind::MacOsTerminalApp);
}

#[test]
fn resolve_windows_terminal_wt() {
    let h = FakeHost::default().with("wt.exe", "C:/wt.exe");
    let t = resolve_terminal(&h, OsKind::Windows).unwrap();
    assert_eq!(t.kind(), TerminalKind::WindowsTerminal);
}

#[test]
fn resolve_windows_terminal_fallback_cmd() {
    let t = resolve_terminal(&FakeHost::default(), OsKind::Windows).unwrap();
    assert_eq!(t.kind(), TerminalKind::WindowsCmd);
}

#[test]
fn resolve_linux_prefers_gnome() {
    let h = FakeHost::default().with("gnome-terminal", "/usr/bin/gnome-terminal");
    let t = resolve_terminal(&h, OsKind::Linux).unwrap();
    assert!(matches!(
        t.kind(),
        TerminalKind::GnomeTerminal | TerminalKind::GenericArgv
    ));
}
