use super::*;

#[test]
fn validate_socket_path_len_rejects_long_path() {
    let long = PathBuf::from(format!("/tmp/{}/mysqld-3306.sock", "x".repeat(120)));
    let err = validate_socket_path_len(&long).unwrap_err();
    assert!(matches!(
        err,
        DblError::InvalidInput {
            field: "socket",
            ..
        }
    ));
}

#[test]
fn validate_socket_path_len_boundary_at_limit_minus_one_limit_plus_one() {
    let limit = socket_path_limit();
    let base = "/tmp/s/";
    let suffix = ".sock";
    let fixed = base.len() + suffix.len();
    let at_limit = format!("{base}{}{suffix}", "a".repeat(limit - fixed));
    assert_eq!(at_limit.len(), limit);
    validate_socket_path_len(Path::new(&at_limit)).expect("at limit fits");

    let at_limit_minus_one = format!("{base}{}{suffix}", "a".repeat(limit - fixed - 1));
    assert_eq!(at_limit_minus_one.len(), limit - 1);
    validate_socket_path_len(Path::new(&at_limit_minus_one)).expect("limit-1 fits");

    let at_limit_plus_one = format!("{base}{}{suffix}", "a".repeat(limit - fixed + 1));
    assert_eq!(at_limit_plus_one.len(), limit + 1);
    let err = validate_socket_path_len(Path::new(&at_limit_plus_one)).unwrap_err();
    match err {
        DblError::InvalidInput { field, reason } => {
            assert_eq!(field, "socket");
            assert!(reason.contains(&(limit + 1).to_string()));
            assert!(reason.contains(&limit.to_string()));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn socket_path_limit_is_platform_specific() {
    let limit = socket_path_limit();
    #[cfg(target_os = "linux")]
    assert_eq!(limit, 107);
    #[cfg(not(target_os = "linux"))]
    assert_eq!(limit, 103);
    assert!(limit <= 107);
}

#[test]
fn socket_path_from_my_cnf_missing_empty_and_spaces() {
    let dir = tempfile::tempdir().expect("tmp");
    let cnf = dir.path().join("my.cnf");
    fs::write(&cnf, "[mysqld]\nport=42000\n# socket=/tmp/x.sock\n").expect("write");
    assert!(socket_path_from_my_cnf(&cnf).is_none());

    fs::write(&cnf, "[mysqld]\nsocket=\nport=42000\n").expect("write");
    assert!(socket_path_from_my_cnf(&cnf).is_none());

    fs::write(&cnf, "[mysqld]\n socket=/tmp/dblocal-1/m1.sock \n").expect("write");
    assert_eq!(
        socket_path_from_my_cnf(&cnf),
        Some(PathBuf::from("/tmp/dblocal-1/m1.sock")),
        "leading spaces before key should be ignored by trim"
    );

    fs::write(&cnf, "[mysqld]\nsocket=   /tmp/dblocal-1/m1.sock  \n").expect("write");
    assert_eq!(
        socket_path_from_my_cnf(&cnf),
        Some(PathBuf::from("/tmp/dblocal-1/m1.sock"))
    );
}

#[test]
fn fallback_socket_name_validation_for_port() {
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
    // uppercase hex is accepted by is_ascii_hexdigit; that is harmless here
    assert!(is_fallback_socket_name_for_port(
        "m42000-ABCDEF012345.sock",
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
}

#[test]
fn socket_name_validation_rejects_short_hash_and_wrong_suffix() {
    assert!(!is_fallback_socket_name_for_port("m42205-abc.sock", 42205));
    assert!(!is_fallback_socket_name_for_port(
        "m42205-abcdef012345.sockx",
        42205
    ));
    assert!(!is_fallback_socket_name_for_port("", 42205));
    assert!(is_fallback_socket_name_for_port(
        "m42205-abcdef012345.sock",
        42205
    ));
}
