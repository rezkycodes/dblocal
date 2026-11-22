//! R9: instance paths are derived from the real DBLocal root.

#![cfg(test)]

use super::*;

/// R9: every instance path the UI prints comes from the real home root, never a
/// literal `~/.dblocal`, is abbreviated against `$HOME`, and a Windows path
/// keeps its separators.
#[test]
fn instance_paths_come_from_the_real_home_root() {
    let user_home = std::path::Path::new("/tmp");
    let root = std::path::Path::new("/tmp/dblocal-home-w2b");
    let data = data_dir(root, Some(user_home), "app");
    let log = engine_log_path(root, Some(user_home), "app");
    assert_eq!(data, "~/dblocal-home-w2b/instances/app/data");
    assert_eq!(log, "~/dblocal-home-w2b/instances/app/logs/engine.log");
    assert!(!data.contains("~/.dblocal") && !log.contains("~/.dblocal"));

    // A home outside `$HOME` is printed in full, never rewritten.
    let outside = data_dir(std::path::Path::new("/opt/dblocal"), None, "app");
    assert_eq!(outside, "/opt/dblocal/instances/app/data");

    // A Windows-style path is shown as is, not rewritten with `/`.
    let windows = std::path::Path::new(r"C:\Users\x\AppData\Local\DBLocal");
    let printed = abbreviate_home(windows, None);
    assert!(printed.starts_with(r"C:\Users"), "{printed}");
    assert!(!printed.contains("~/"), "{printed}");
    let data = data_dir(windows, None, "app");
    assert!(data.contains("DBLocal"), "{data}");
    assert!(data.contains('\\'), "{data}");
}
