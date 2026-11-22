//! Tests for the shared socket-fallback and private-directory module.
//!
//! Environment is never mutated: every function takes `XDG_RUNTIME_DIR` and the
//! uid as arguments, so the tests drive the pure functions and inject candidate
//! lists directly.

use super::*;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use tempfile::tempdir;

fn uid() -> u32 {
    current_uid()
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path)
        .expect("meta")
        .permissions()
        .mode()
        & 0o777
}

fn private(dir: &Path) -> PathBuf {
    fs::create_dir_all(dir).expect("mkdir");
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).expect("chmod 0700");
    dir.to_path_buf()
}

fn long_path(dir: &Path) -> PathBuf {
    dir.join("r".repeat(140)).join("run")
}

fn bind_socket(path: &Path) {
    use std::os::unix::fs::FileTypeExt as _;
    let _ = fs::remove_file(path);
    let listener = std::os::unix::net::UnixListener::bind(path).expect("bind socket");
    drop(listener);
    assert!(
        fs::symlink_metadata(path)
            .expect("meta")
            .file_type()
            .is_socket()
    );
}

// ---------------------------------------------------------------------------
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

// ---------------------------------------------------------------------------
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

#[test]
fn resolve_skips_a_candidate_that_cannot_be_verified() {
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

// ---------------------------------------------------------------------------
// Directory verification
// ---------------------------------------------------------------------------

#[test]
fn verify_secure_dir_refuses_symlink_regular_file_and_wrong_mode() {
    let dir = tempdir().expect("tmp");
    let real_path = dir.path().join("real");
    let real = private(&real_path);
    verify_secure_dir(&real, uid()).expect("real 0700 dir is accepted");

    let link = dir.path().join("link");
    symlink(&real, &link).expect("symlink");
    assert!(
        verify_secure_dir(&link, uid())
            .expect_err("symlink")
            .to_string()
            .contains("not a real directory")
    );

    let file = dir.path().join("file");
    fs::write(&file, b"x").expect("file");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o700)).expect("chmod");
    assert!(
        verify_secure_dir(&file, uid())
            .expect_err("regular file")
            .to_string()
            .contains("not a real directory")
    );

    for mode_bits in [0o755, 0o770] {
        let loose = dir.path().join(format!("loose-{mode_bits:o}"));
        fs::create_dir(&loose).expect("dir");
        fs::set_permissions(&loose, fs::Permissions::from_mode(mode_bits)).expect("chmod");
        let err = verify_secure_dir(&loose, uid()).expect_err("wrong mode");
        let msg = err.to_string();
        assert!(msg.contains("0700"), "{msg}");
        assert!(msg.contains(&format!("{mode_bits:04o}")), "{msg}");
        assert_eq!(mode(&loose), mode_bits, "verify must not chmod");
    }
}

#[test]
fn verify_secure_dir_refuses_a_foreign_uid() {
    let dir = tempdir().expect("tmp");
    let real_path = dir.path().join("real");
    let real = private(&real_path);
    assert_eq!(
        fs::symlink_metadata(&real).expect("meta").uid(),
        uid(),
        "test assumption: the directory is ours"
    );
    let err = verify_secure_dir(&real, uid() + 1).expect_err("foreign uid");
    assert!(err.to_string().contains("wrong owner"), "{err}");
}

#[test]
fn verify_secure_dir_reports_a_missing_directory() {
    let dir = tempdir().expect("tmp");
    let err = verify_secure_dir(&dir.path().join("nope"), uid()).expect_err("missing");
    assert!(
        err.to_string().contains("cannot inspect private dir"),
        "{err}"
    );
}

#[test]
fn ensure_secure_dir_creates_0700_and_never_repairs_an_existing_mode() {
    let dir = tempdir().expect("tmp");
    let target = dir.path().join("new");
    ensure_secure_dir(&target, uid()).expect("create");
    assert!(target.is_dir());
    assert_eq!(mode(&target), 0o700);
    assert_eq!(fs::symlink_metadata(&target).expect("meta").uid(), uid());

    // A pre-existing 0770 directory is refused and left at 0770.
    let loose = dir.path().join("loose");
    fs::create_dir(&loose).expect("dir");
    fs::set_permissions(&loose, fs::Permissions::from_mode(0o770)).expect("chmod");
    ensure_secure_dir(&loose, uid()).expect_err("wrong mode");
    assert_eq!(mode(&loose), 0o770, "must not silently chmod");
}

#[test]
fn ensure_secure_dir_creates_missing_parents_and_accepts_a_racing_creator() {
    let dir = tempdir().expect("tmp");
    let target = dir.path().join("a").join("b").join("c");
    ensure_secure_dir(&target, uid()).expect("deep create");
    assert_eq!(mode(&target), 0o700);
    // Second call is the "another process created it first" path and re-verifies.
    ensure_secure_dir(&target, uid()).expect("already exists");
}

#[test]
fn ensure_secure_dir_reports_a_parent_that_is_a_file() {
    let dir = tempdir().expect("tmp");
    let not_a_dir = dir.path().join("run");
    fs::write(&not_a_dir, b"x").expect("write");
    // Rust resolves the parent before creating anything, so the failure is an
    // inspect error (`Not a directory`), not a create error: either is a typed
    // `Engine` error naming the path, which is what matters.
    let err = ensure_secure_dir(&not_a_dir.join("child"), uid()).expect_err("parent is a file");
    let msg = err.to_string();
    assert!(
        msg.contains("cannot inspect private dir")
            || msg.contains("cannot create parent")
            || msg.contains("cannot create private dir"),
        "{msg}"
    );
    assert!(matches!(err, DblError::Engine { .. }));
}

#[test]
fn ensure_owned_dir_tightens_an_existing_directory_of_ours() {
    let dir = tempdir().expect("tmp");
    let loose = dir.path().join("tmp");
    fs::create_dir(&loose).expect("dir");
    fs::set_permissions(&loose, fs::Permissions::from_mode(0o755)).expect("chmod");
    ensure_owned_dir(&loose, uid()).expect("tighten");
    assert_eq!(mode(&loose), 0o700, "a directory we own is tightened");

    let missing = dir.path().join("created");
    ensure_owned_dir(&missing, uid()).expect("create");
    assert_eq!(mode(&missing), 0o700);

    let err = ensure_owned_dir(&loose, uid() + 1).expect_err("foreign uid");
    assert!(err.to_string().contains("wrong owner"), "{err}");
}

#[test]
fn ensure_owned_dir_rejects_symlink_regular_file_and_foreign_owner() {
    let dir = tempdir().expect("tmp");
    let real_path = dir.path().join("real");
    let real = private(&real_path);
    let link = dir.path().join("link");
    symlink(&real, &link).expect("symlink");
    let err = ensure_owned_dir(&link, uid()).expect_err("symlink");
    assert!(err.to_string().contains("not a real directory"), "{err}");

    let file = dir.path().join("file");
    fs::write(&file, b"x").expect("file");
    let err = ensure_owned_dir(&file, uid()).expect_err("regular file");
    assert!(err.to_string().contains("not a real directory"), "{err}");
}

// ---------------------------------------------------------------------------
// Cleanup
// ---------------------------------------------------------------------------

fn xdg_candidate(dir: &Path) -> (OsString, PathBuf) {
    let xdg = dir.join("xdg");
    fs::create_dir_all(&xdg).expect("xdg");
    let fb = private(&xdg.join("dblocal"));
    (xdg.as_os_str().to_os_string(), fb)
}

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

// ---------------------------------------------------------------------------
// prepare_run
// ---------------------------------------------------------------------------

struct Layout {
    _dir: tempfile::TempDir,
    run: PathBuf,
    config: PathBuf,
    natural: PathBuf,
    tmpdir: PathBuf,
}

fn layout() -> Layout {
    let dir = tempdir().expect("tmp");
    let inst = dir.path().join("inst");
    let data = inst.join("data");
    let run = inst.join("run");
    fs::create_dir_all(&data).expect("data");
    fs::create_dir_all(&run).expect("run");
    Layout {
        run: run.clone(),
        config: run.join("my.cnf"),
        natural: run.join("mysqld-42200.sock"),
        tmpdir: run.join("tmp"),
        _dir: dir,
    }
}

fn spec<'a>(layout: &'a Layout, xdg: Option<OsString>) -> PrepareSpec<'a> {
    PrepareSpec {
        config: &layout.config,
        natural_socket: &layout.natural,
        port: 42200,
        tmpdir: &layout.tmpdir,
        xdg_runtime_dir: xdg,
        uid: uid(),
    }
}

#[test]
fn prepare_run_recreates_the_tmpdir_and_a_fallback_dir() {
    let l = layout();
    let xdg = l._dir.path().join("xdg");
    fs::create_dir_all(&xdg).expect("xdg");
    let fb = xdg.join("dblocal");
    let sock = fb.join("m42200-abc123def456.sock");
    fs::write(
        &l.config,
        format!("[mysqld]\nsocket={}\nport=42200\n", sock.display()),
    )
    .expect("cnf");

    prepare_run(&spec(&l, Some(xdg.as_os_str().to_os_string()))).expect("prepare");

    let meta = fs::symlink_metadata(&l.tmpdir).expect("tmpdir meta");
    assert!(meta.is_dir());
    assert_eq!(meta.permissions().mode() & 0o777, 0o700);
    assert_eq!(meta.uid(), uid());
    let fb_meta = fs::symlink_metadata(&fb).expect("fb meta");
    assert_eq!(fb_meta.permissions().mode() & 0o777, 0o700);
    assert_eq!(fb_meta.uid(), uid());
}

#[test]
fn prepare_run_creates_the_natural_run_dir() {
    let l = layout();
    fs::remove_dir_all(&l.run).expect("remove run");
    fs::create_dir_all(&l.run).expect("recreate run");
    fs::write(
        &l.config,
        format!("[mysqld]\nsocket={}\n", l.natural.display()),
    )
    .expect("cnf");
    prepare_run(&spec(&l, None)).expect("prepare");
    assert!(l.natural.parent().unwrap().is_dir());
    assert!(l.tmpdir.is_dir());
}

#[test]
fn prepare_run_removes_a_stale_socket_and_refuses_a_regular_file() {
    let l = layout();
    let xdg = l._dir.path().join("xdg");
    fs::create_dir_all(&xdg).expect("xdg");
    let fb = private(&xdg.join("dblocal"));
    let sock = fb.join("m42200-abc123def456.sock");
    bind_socket(&sock);
    fs::write(&l.config, format!("[mysqld]\nsocket={}\n", sock.display())).expect("cnf");

    prepare_run(&spec(&l, Some(xdg.as_os_str().to_os_string()))).expect("prepare");
    assert!(!sock.exists(), "a stale socket left by a crash is removed");

    fs::write(&sock, b"not socket").expect("write");
    let err = prepare_run(&spec(&l, Some(xdg.as_os_str().to_os_string())))
        .expect_err("a regular file at the socket path must fail the start");
    assert!(err.to_string().contains("not a socket"), "{err}");
}

#[test]
fn prepare_run_refuses_a_symlinked_or_loose_fallback_dir() {
    let l = layout();
    let xdg = l._dir.path().join("xdg");
    fs::create_dir_all(&xdg).expect("xdg");
    let target = private(&l._dir.path().join("target"));
    let link = xdg.join("dblocal");
    symlink(&target, &link).expect("symlink");
    let sock = link.join("m42200-abc123def456.sock");
    fs::write(&l.config, format!("[mysqld]\nsocket={}\n", sock.display())).expect("cnf");
    let err = prepare_run(&spec(&l, Some(xdg.as_os_str().to_os_string()))).expect_err("symlink");
    assert!(err.to_string().contains("not a real directory"), "{err}");

    let loose = l._dir.path().join("loose-xdg").join("dblocal");
    fs::create_dir_all(&loose).expect("loose");
    fs::set_permissions(&loose, fs::Permissions::from_mode(0o755)).expect("chmod");
    let sock = loose.join("m42200-abc123def456.sock");
    fs::write(&l.config, format!("[mysqld]\nsocket={}\n", sock.display())).expect("cnf");
    let err = prepare_run(&spec(
        &l,
        Some(l._dir.path().join("loose-xdg").as_os_str().to_os_string()),
    ))
    .expect_err("wrong mode");
    assert!(err.to_string().contains("0700"), "{err}");
    assert_eq!(mode(&loose), 0o755, "prepare must not chmod a foreign dir");
}

#[test]
fn prepare_run_rejects_a_socket_name_that_is_not_for_this_port() {
    let l = layout();
    let dir = l._dir.path().join("private");
    let private_dir = private(&dir);
    let sock = private_dir.join("mariadb-42200-aabb.sock");
    fs::write(&l.config, format!("[mysqld]\nsocket={}\n", sock.display())).expect("cnf");
    let err = prepare_run(&spec(&l, None)).expect_err("foreign socket name");
    assert!(
        err.to_string()
            .contains("not an allowed fallback socket path"),
        "{err}"
    );
    assert!(!sock.exists());
}

#[test]
fn prepare_run_clears_stale_tmpdir_contents() {
    let l = layout();
    fs::create_dir_all(&l.tmpdir).expect("tmpdir");
    fs::write(l.tmpdir.join("#sql-temptable-stale"), b"old").expect("stale");
    fs::create_dir_all(l.tmpdir.join("nested")).expect("nested");
    fs::write(l.tmpdir.join("nested/file"), b"x").expect("nested file");
    fs::write(
        &l.config,
        format!("[mysqld]\nsocket={}\n", l.natural.display()),
    )
    .expect("cnf");
    prepare_run(&spec(&l, None)).expect("prepare");
    assert_eq!(
        fs::read_dir(&l.tmpdir).expect("read").count(),
        0,
        "stale tmpdir contents must be cleared on every start"
    );
}

#[test]
fn prepare_run_skips_a_missing_config_and_rejects_one_without_socket() {
    let l = layout();
    prepare_run(&spec(&l, None)).expect("no config yet is not an error");
    assert!(l.tmpdir.is_dir(), "the tmpdir is still prepared");

    fs::write(&l.config, "[mysqld]\nport=42200\n").expect("cnf");
    let err = prepare_run(&spec(&l, None)).expect_err("no socket directive");
    assert!(
        err.to_string().contains("has no `socket` directive"),
        "{err}"
    );
}

#[test]
fn prepare_run_errors_when_the_tmpdir_cannot_be_created() {
    let l = layout();
    fs::write(&l.tmpdir, b"x").expect("a regular file where the tmpdir must be");
    let err = prepare_run(&spec(&l, None)).expect_err("tmpdir is a file");
    assert!(
        err.to_string().contains("cannot prepare private tmpdir"),
        "{err}"
    );
}

#[test]
fn prepare_run_errors_on_an_unreadable_config_that_exists() {
    if uid() == 0 {
        eprintln!("skipping as root: mode 000 does not block reads");
        return;
    }
    let l = layout();
    fs::write(&l.config, "[mysqld]\nsocket=/tmp/x.sock\n").expect("cnf");
    fs::set_permissions(&l.config, fs::Permissions::from_mode(0o000)).expect("chmod");
    let err = prepare_run(&spec(&l, None)).expect_err("unreadable config");
    fs::set_permissions(&l.config, fs::Permissions::from_mode(0o600)).expect("restore");
    assert!(err.to_string().contains("cannot read"), "{err}");
}

#[test]
fn prepare_run_errors_when_the_tmpdir_cannot_be_listed() {
    let l = layout();
    // A regular file at the tmpdir path makes the listing fail after the
    // ownership/tighten step would have refused it; either way it is an Engine
    // error naming the tmpdir.
    fs::write(&l.tmpdir, b"not a dir").expect("file");
    fs::write(
        &l.config,
        format!("[mysqld]\nsocket={}\n", l.natural.display()),
    )
    .expect("cnf");
    let err = prepare_run(&spec(&l, None)).expect_err("tmpdir is a file");
    assert!(
        err.to_string().contains("cannot prepare private tmpdir"),
        "{err}"
    );
}

#[test]
fn clear_dir_contents_removes_files_and_nested_directories() {
    let dir = tempdir().expect("tmp");
    let root = dir.path().join("root");
    fs::create_dir_all(root.join("sub")).expect("sub");
    fs::write(root.join("f"), b"x").expect("f");
    fs::write(root.join("sub/g"), b"x").expect("g");
    clear_dir_contents(&root).expect("clear");
    assert_eq!(fs::read_dir(&root).expect("read").count(), 0);
    assert!(root.is_dir(), "the directory itself stays");

    let err = clear_dir_contents(&dir.path().join("missing")).expect_err("missing dir");
    assert!(err.to_string().contains("cannot read private dir"), "{err}");
}
