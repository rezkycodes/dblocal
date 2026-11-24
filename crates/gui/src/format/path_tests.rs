//! R9: instance paths are derived from the real DBLocal root.

#![cfg(test)]

use super::*;

/// R9: every instance path the UI prints comes from the real home root, never a
/// literal `~/.dblocal`, is abbreviated against `$HOME`, and a Windows path
/// keeps its separators.
///
/// Expected values are built with `Path::join` rather than hardcoded strings, so
/// the assertion holds where the separator differs (`/` vs `\`).
#[test]
fn instance_paths_come_from_the_real_home_root() {
    let user_home = std::path::Path::new("/tmp");
    let root = std::path::Path::new("/tmp/dblocal-home-w2b");
    let data = data_dir(root, Some(user_home), "app");
    let log = engine_log_path(root, Some(user_home), "app");
    let expected_data = format!(
        "~/{}",
        std::path::Path::new("dblocal-home-w2b")
            .join("instances")
            .join("app")
            .join("data")
            .to_string_lossy()
    );
    let expected_log = format!(
        "~/{}",
        std::path::Path::new("dblocal-home-w2b")
            .join("instances")
            .join("app")
            .join("logs")
            .join("engine.log")
            .to_string_lossy()
    );
    assert_eq!(data, expected_data);
    assert_eq!(log, expected_log);
    assert!(!data.contains("~/.dblocal") && !log.contains("~/.dblocal"));

    // A home outside `$HOME` is printed in full, never rewritten.
    let outside = data_dir(std::path::Path::new("/opt/dblocal"), None, "app");
    let expected_outside = std::path::Path::new("/opt/dblocal")
        .join("instances")
        .join("app")
        .join("data");
    assert_eq!(outside, expected_outside.to_string_lossy());

    // A Windows-style path is shown as is, not rewritten with `/`.
    let windows = std::path::Path::new(r"C:\Users\x\AppData\Local\DBLocal");
    let printed = abbreviate_home(windows, None);
    assert!(printed.starts_with(r"C:\Users"), "{printed}");
    assert!(!printed.contains("~/"), "{printed}");
    let data = data_dir(windows, None, "app");
    assert!(data.contains("DBLocal"), "{data}");
    assert!(data.contains('\\'), "{data}");
}
