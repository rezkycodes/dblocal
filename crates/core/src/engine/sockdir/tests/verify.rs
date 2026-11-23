use super::prelude::*;

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
