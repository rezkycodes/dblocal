use super::prelude::*;

// Cleanup
// ---------------------------------------------------------------------------

#[test]
fn cleanup_removes_a_real_socket_with_the_right_name_and_parent() {
    let dir = tempdir().expect("tmp");
    let (xdg, fb) = xdg_candidate(dir.path());
    let sock = fb.join("m42013-abc123def456.sock");
    bind_socket(&sock);
    cleanup_socket(&sock, 42013, Some(xdg), uid());
    assert!(!sock.exists(), "verified fallback socket must be removed");
}

#[test]
fn cleanup_refuses_a_regular_file_with_a_valid_name() {
    let dir = tempdir().expect("tmp");
    let (xdg, fb) = xdg_candidate(dir.path());
    let path = fb.join("m42010-abc123def456.sock");
    fs::write(&path, b"not a socket").expect("write");
    cleanup_socket(&path, 42010, Some(xdg), uid());
    assert!(path.exists(), "a regular file must never be deleted");
}

#[test]
fn cleanup_refuses_a_foreign_name_and_another_port() {
    let dir = tempdir().expect("tmp");
    let (xdg, fb) = xdg_candidate(dir.path());
    let foreign = fb.join("not-a-fallback.sock");
    bind_socket(&foreign);
    cleanup_socket(&foreign, 42011, Some(xdg.clone()), uid());
    assert!(foreign.exists(), "foreign name must never be deleted");

    let other_port = fb.join("m42012-abc123def456.sock");
    bind_socket(&other_port);
    cleanup_socket(&other_port, 42011, Some(xdg), uid());
    assert!(other_port.exists(), "another port's socket must stay");
}

#[test]
fn cleanup_refuses_a_socket_outside_every_candidate() {
    let dir = tempdir().expect("tmp");
    let (xdg, _fb) = xdg_candidate(dir.path());
    let elsewhere = private(&dir.path().join("elsewhere"));
    let stray = elsewhere.join("m42206-abc123def456.sock");
    bind_socket(&stray);
    cleanup_socket(&stray, 42206, Some(xdg), uid());
    assert!(stray.exists(), "a socket outside the candidates must stay");
}

#[test]
fn cleanup_tolerates_a_vanished_path_and_a_path_without_a_name() {
    let dir = tempdir().expect("tmp");
    let (xdg, fb) = xdg_candidate(dir.path());
    let vanished = fb.join("m42233-abc123def456.sock");
    cleanup_socket(&vanished, 42233, Some(xdg), uid());
    assert!(!vanished.exists());

    // No parent and no file name: not a fallback socket, so nothing to do.
    assert!(!is_allowed_fallback_socket(
        Path::new("/"),
        42234,
        None,
        uid()
    ));
    cleanup_socket(Path::new("/"), 42234, None, uid());
}

#[test]
fn cleanup_from_config_skips_a_foreign_socket_and_removes_a_verified_one() {
    let dir = tempdir().expect("tmp");
    let (xdg, fb) = xdg_candidate(dir.path());
    let cnf = dir.path().join("my.cnf");
    let foreign = dir.path().join("notes.txt");
    fs::write(&foreign, b"notes").expect("notes");
    fs::write(&cnf, format!("[mysqld]\nsocket={}\n", foreign.display())).expect("cnf");
    cleanup_socket_from_config(&cnf, 42215, Some(xdg.clone()), uid());
    assert!(foreign.exists(), "a regular non-fallback file must survive");

    let sock = fb.join("m42215-abc123def456.sock");
    bind_socket(&sock);
    fs::write(&cnf, format!("[mysqld]\nsocket={}\n", sock.display())).expect("cnf2");
    cleanup_socket_from_config(&cnf, 42215, Some(xdg), uid());
    assert!(
        !sock.exists(),
        "the verified fallback socket must be removed"
    );

    // No socket directive: a no-op.
    fs::write(&cnf, "[mysqld]\nport=42232\n").expect("cnf3");
    cleanup_socket_from_config(&cnf, 42232, None, uid());
}

// ---------------------------------------------------------------------------
// my.cnf parsing
// ---------------------------------------------------------------------------

#[test]
fn parse_directive_handles_absent_empty_spaces_comments_and_quotes() {
    assert_eq!(
        parse_directive("[mysqld]\nport=42000\n# socket=/tmp/x.sock\n", "socket"),
        None
    );
    assert_eq!(
        parse_directive("[mysqld]\nsocket=\nport=42000\n", "socket"),
        None
    );
    assert_eq!(
        parse_directive("[mysqld]\n;socket=/tmp/x.sock\n", "socket"),
        None
    );
    assert_eq!(
        parse_directive("[mysqld]\n socket = /tmp/dblocal-1/m1.sock \n", "socket"),
        Some("/tmp/dblocal-1/m1.sock".to_owned())
    );
    assert_eq!(
        parse_directive("[mysqld]\nsocket=   /tmp/dblocal-1/m1.sock  \n", "socket"),
        Some("/tmp/dblocal-1/m1.sock".to_owned())
    );
    assert_eq!(
        parse_directive(
            "[mysqld]\nsocket=/tmp/a b/m1.sock # trailing note\n",
            "socket"
        ),
        Some("/tmp/a b/m1.sock".to_owned())
    );
    assert_eq!(
        parse_directive("[mysqld]\nsocket=\"/tmp/a#b/m1.sock\"\n", "socket"),
        Some("/tmp/a#b/m1.sock".to_owned()),
        "a # inside a quoted value is part of the path"
    );
    assert_eq!(
        parse_directive("[mysqld]\nsocket='/tmp/a b/m1.sock'\n", "socket"),
        Some("/tmp/a b/m1.sock".to_owned())
    );
    assert_eq!(
        parse_directive("[mysqld]\n  tmpdir = /run/x/tmp  \n", "tmpdir"),
        Some("/run/x/tmp".to_owned())
    );
    // A key that is only a prefix of another key must not match.
    assert_eq!(
        parse_directive("[mysqld]\nsocket_path=/tmp/x\n", "socket"),
        None
    );
    // Lines without `=` are ignored.
    assert_eq!(parse_directive("[mysqld]\nsocket\n", "socket"), None);
    // The first declaration wins.
    assert_eq!(
        parse_directive("socket=/a\nsocket=/b\n", "socket"),
        Some("/a".to_owned())
    );
}

#[test]
fn read_directive_reads_a_file_and_treats_a_missing_file_as_no_key() {
    let dir = tempdir().expect("tmp");
    let cnf = dir.path().join("my.cnf");
    fs::write(
        &cnf,
        "[mysqld]\nsocket=/tmp/dblocal-1/m1.sock\ntmpdir=/run/tmp\n",
    )
    .expect("write");
    assert_eq!(
        read_directive(&cnf, "socket"),
        Some("/tmp/dblocal-1/m1.sock".to_owned())
    );
    assert_eq!(read_directive(&cnf, "tmpdir"), Some("/run/tmp".to_owned()));
    assert_eq!(read_directive(&cnf, "port"), None);
    assert_eq!(
        read_directive(&dir.path().join("missing.cnf"), "socket"),
        None
    );
}
