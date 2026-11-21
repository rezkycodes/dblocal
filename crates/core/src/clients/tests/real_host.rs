use std::path::PathBuf;

use crate::clients::{HostProbe, detect_clients};

struct SystemHost;

impl HostProbe for SystemHost {
    fn which(&self, name: &str) -> Option<PathBuf> {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths).find_map(|dir| {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    Some(candidate)
                } else {
                    None
                }
            })
        })
    }
    fn path_exists(&self, path: &std::path::Path) -> bool {
        path.exists()
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
fn print_real_detection_on_linux_host() {
    let clients = detect_clients(&SystemHost);
    eprintln!("REAL_HOST_CLIENTS:");
    for c in &clients {
        eprintln!("  {} -> {}", c.kind().as_str(), c.path().display());
    }
    // no assertion: host state varies
}
