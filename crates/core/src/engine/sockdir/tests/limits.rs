use super::prelude::*;

// Candidates and limits (pure)
// ---------------------------------------------------------------------------

#[test]
fn socket_path_limit_is_platform_specific() {
    let limit = socket_path_limit();
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
    assert_eq!(limit, 103);
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "freebsd")))]
    assert_eq!(limit, 107);
}

#[test]
fn fallback_candidates_are_a_pure_function_of_xdg_and_uid() {
    let unset = fallback_dir_candidates(None, 4242);
    assert_eq!(unset, vec![PathBuf::from("/tmp/dblocal-4242")]);

    let empty = fallback_dir_candidates(Some(OsString::from("")), 4242);
    assert_eq!(empty, vec![PathBuf::from("/tmp/dblocal-4242")]);

    let set = fallback_dir_candidates(Some(OsString::from("/run/user/4242")), 4242);
    assert_eq!(
        set,
        vec![
            PathBuf::from("/run/user/4242/dblocal"),
            PathBuf::from("/tmp/dblocal-4242")
        ]
    );

    // Same inputs, same answer; a different uid changes only the /tmp leaf.
    assert_eq!(
        set,
        fallback_dir_candidates(Some(OsString::from("/run/user/4242")), 4242)
    );
    assert_eq!(
        fallback_dir_candidates(None, 7),
        vec![PathBuf::from("/tmp/dblocal-7")]
    );
}

#[test]
fn validate_socket_path_len_accepts_at_limit_and_rejects_one_over() {
    let limit = socket_path_limit();
    let base = "/tmp/s/";
    let suffix = ".sock";
    let fixed = base.len() + suffix.len();
    let at_limit = format!("{base}{}{suffix}", "a".repeat(limit - fixed));
    assert_eq!(path_len(Path::new(&at_limit)), limit);
    validate_socket_path_len(Path::new(&at_limit)).expect("at limit fits");

    let over = format!("{base}{}{suffix}", "a".repeat(limit - fixed + 1));
    let err = validate_socket_path_len(Path::new(&over)).expect_err("one over");
    match err {
        DblError::InvalidInput { field, reason } => {
            assert_eq!(field, "socket");
            assert!(reason.contains(&(limit + 1).to_string()), "{reason}");
            assert!(reason.contains(&limit.to_string()), "{reason}");
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn fallback_socket_name_is_m_port_dash_twelve_hex() {
    let data = Path::new("/home/u/.dblocal/instances/app/data");
    let name = fallback_socket_name(3307, data);
    assert!(name.starts_with("m3307-"), "{name}");
    assert!(name.ends_with(".sock"), "{name}");
    assert_eq!(name.len(), "m3307-".len() + 12 + ".sock".len());
    // Deterministic, and different instances never collide.
    assert_eq!(name, fallback_socket_name(3307, data));
    assert_ne!(name, fallback_socket_name(3307, Path::new("/other/data")));
    assert_ne!(name, fallback_socket_name(3308, data));
}

#[test]
fn fallback_socket_name_validation_is_strict() {
    assert!(is_fallback_socket_name_for_port(
        "m42000-abcdef012345.sock",
        42000
    ));
    assert!(!is_fallback_socket_name_for_port(
        "m42001-abcdef012345.sock",
        42000
    ));
    assert!(!is_fallback_socket_name_for_port(
        "m42000-abcdef01234.sock",
        42000
    ));
    assert!(!is_fallback_socket_name_for_port(
        "m42000-abcdef0123456.sock",
        42000
    ));
    assert!(!is_fallback_socket_name_for_port(
        "other-abcdef012345.sock",
        42000
    ));
    assert!(!is_fallback_socket_name_for_port(
        "m42000-abcdef012345.socket",
        42000
    ));
    assert!(!is_fallback_socket_name_for_port(
        "m42000-abcdef01234z.sock",
        42000
    ));
    assert!(!is_fallback_socket_name_for_port("", 42000));
}
