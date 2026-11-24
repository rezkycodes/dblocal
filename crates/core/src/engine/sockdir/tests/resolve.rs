use super::prelude::*;

// Resolution
// ---------------------------------------------------------------------------

#[test]
fn resolve_returns_the_natural_path_when_it_fits() {
    let dir = tempdir().expect("tmp");
    let run = dir.path().join("run");
    fs::create_dir_all(&run).expect("run");
    let natural = run.join("mysqld-42000.sock");
    let identity = dir.path().join("inst");
    let chosen = resolve_socket_path(&natural, 42000, &identity, None, uid()).expect("natural");
    assert_eq!(chosen, natural);
}

#[test]
fn resolve_falls_back_to_the_first_verifiable_candidate_that_fits() {
    let dir = tempdir().expect("tmp");
    let first_path = dir.path().join("first");
    let first = private(&first_path);
    let second_path = dir.path().join("second");
    let second = private(&second_path);
    let natural = long_path(dir.path()).join("mysqld-42001.sock");
    let identity = dir.path().join("instance");
    let chosen = resolve_socket_path_with_candidates(
        &natural,
        42001,
        &identity,
        vec![first.to_path_buf(), second.to_path_buf()],
        uid(),
    )
    .expect("fallback");
    assert!(chosen.starts_with(first), "{chosen:?}");
    assert_eq!(
        chosen.file_name().unwrap().to_string_lossy(),
        fallback_socket_name(42001, &identity)
    );
    assert!(socket_path_fits(&chosen));
}

#[cfg(unix)]
#[test]
fn resolve_skips_a_candidate_that_cannot_be_verified() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempdir().expect("tmp");
    let bad = dir.path().join("bad");
    fs::create_dir_all(&bad).expect("bad");
    fs::set_permissions(&bad, fs::Permissions::from_mode(0o755)).expect("chmod");
    let good_path = dir.path().join("good");
    let good = private(&good_path);
    let natural = long_path(dir.path()).join("mysqld-42002.sock");
    let chosen = resolve_socket_path_with_candidates(
        &natural,
        42002,
        &dir.path().join("instance"),
        vec![bad.clone(), good.to_path_buf()],
        uid(),
    )
    .expect("second candidate wins");
    assert!(chosen.starts_with(&good), "{chosen:?}");
    assert!(!chosen.starts_with(&bad));
}

#[test]
fn resolve_skips_a_verified_candidate_whose_socket_would_not_fit() {
    let dir = tempdir().expect("tmp");
    // 88 chars + "/dblocal" + "/m42003-<12 hex>.sock" exceeds the 107 byte limit
    // on Linux while the candidate itself is a valid 0700 directory.
    let long_but_path = dir.path().join("n".repeat(88)).join("dblocal");
    let long_but_valid = private(&long_but_path);
    let short_path = dir.path().join("short");
    let short = private(&short_path);
    let natural = long_path(dir.path()).join("mysqld-42003.sock");
    let chosen = resolve_socket_path_with_candidates(
        &natural,
        42003,
        &dir.path().join("instance"),
        vec![long_but_valid.clone(), short.to_path_buf()],
        uid(),
    )
    .expect("short candidate wins");
    assert!(chosen.starts_with(&short), "{chosen:?}");
    assert!(!chosen.starts_with(&long_but_valid));
}

/// The "wrong owner" wording only exists in the POSIX verification arm.
#[cfg(unix)]
#[test]
fn resolve_error_names_every_candidate_and_both_lengths() {
    let dir = tempdir().expect("tmp");
    let first_path = dir.path().join("first");
    let first = private(&first_path);
    let second = dir.path().join("second");
    let natural = long_path(dir.path()).join("mysqld-42004.sock");
    // A foreign uid: every candidate fails verification with "wrong owner".
    let err = resolve_socket_path_with_candidates(
        &natural,
        42004,
        &dir.path().join("instance"),
        vec![first.clone(), second.clone()],
        uid() + 1,
    )
    .expect_err("no candidate verifies");
    match err {
        DblError::InvalidInput { field, reason } => {
            assert_eq!(field, "socket");
            assert!(reason.contains(&first.display().to_string()), "{reason}");
            assert!(reason.contains("wrong owner"), "{reason}");
            assert!(reason.contains("maximum is"), "{reason}");
            assert!(
                reason.contains(&format!("{} bytes", path_len(&natural))),
                "{reason}"
            );
            assert!(
                reason.contains(&socket_path_limit().to_string()),
                "{reason}"
            );
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn resolve_error_for_a_fitting_candidate_reports_the_fitting_attempts() {
    let dir = tempdir().expect("tmp");
    let too_long_path = dir.path().join("f".repeat(140));
    let too_long = private(&too_long_path);
    let natural = long_path(dir.path()).join("mysqld-42005.sock");
    let err = resolve_socket_path_with_candidates(
        &natural,
        42005,
        &dir.path().join("instance"),
        vec![too_long.clone()],
        uid(),
    )
    .expect_err("socket would not fit");
    match err {
        DblError::InvalidInput { field, reason } => {
            assert_eq!(field, "socket");
            assert!(reason.contains(&too_long.display().to_string()), "{reason}");
            assert!(reason.contains("maximum is"), "{reason}");
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn resolve_with_no_candidates_says_so() {
    let dir = tempdir().expect("tmp");
    let natural = long_path(dir.path()).join("mysqld-42006.sock");
    let err = resolve_socket_path_with_candidates(
        &natural,
        42006,
        &dir.path().join("instance"),
        Vec::new(),
        uid(),
    )
    .expect_err("no candidates");
    assert!(err.to_string().contains("no candidates"), "{err}");
}
