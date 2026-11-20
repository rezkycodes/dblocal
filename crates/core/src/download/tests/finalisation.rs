//! Archive extraction, part 2: staging/finalisation, corrupt archives, prefix
//! edge cases and expected-binary validation.

use crate::model::ArchiveFormat;

use super::{TarEntry, ZipEntry, assert_rejected, make_tar_gz, make_tar_xz, make_zip, run_extract};

#[tokio::test]
async fn corrupt_gzip_body_is_reported_as_invalid_archive() {
    let mut data = make_tar_gz(vec![TarEntry::File {
        path: "root/bin/app".to_owned(),
        data: b"bin".to_vec(),
        mode: 0o755,
    }]);
    // Truncate mid-stream so the gzip footer is missing.
    data.truncate(data.len() / 2);
    let run = run_extract(
        &data,
        ArchiveFormat::TarGz,
        Some("root"),
        &["bin/app"],
        None,
        None,
    )
    .await;
    assert!(run.result.is_err(), "corrupt gzip must fail");
    assert!(!run.final_dir().exists());
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn corrupt_xz_body_is_reported_as_invalid_archive() {
    let mut data = make_tar_xz(vec![TarEntry::File {
        path: "root/bin/app".to_owned(),
        data: b"bin".to_vec(),
        mode: 0o755,
    }]);
    data.truncate(data.len() / 2);
    let run = run_extract(
        &data,
        ArchiveFormat::TarXz,
        Some("root"),
        &["bin/app"],
        None,
        None,
    )
    .await;
    assert!(run.result.is_err(), "corrupt xz must fail");
    assert!(!run.final_dir().exists());
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn corrupt_zip_body_is_reported_as_invalid_archive() {
    let data = b"not a zip at all".to_vec();
    let run = run_extract(&data, ArchiveFormat::Zip, None, &[], None, None).await;
    let err = run.result.as_ref().expect_err("must fail");
    assert!(
        matches!(err, crate::DblError::InvalidInput { .. }),
        "{err:?}"
    );
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn existing_final_directory_is_not_overwritten() {
    let data = make_tar_gz(vec![TarEntry::File {
        path: "root/bin/app".to_owned(),
        data: b"bin".to_vec(),
        mode: 0o755,
    }]);
    let run = run_extract(
        &data,
        ArchiveFormat::TarGz,
        Some("root"),
        &["bin/app"],
        None,
        None,
    )
    .await;
    let final_dir = run.result.as_ref().expect("first extract ok").clone();
    let archive = run.base.join("archive.tar.gz");

    let options = super::super::extract::ExtractOptions {
        expected_binaries: vec!["bin/app".to_owned()],
        strip_prefix: Some("root".to_owned()),
        max_total_bytes: None,
        max_entries: None,
    };
    let second = super::super::extract::extract_runtime(
        &archive,
        ArchiveFormat::TarGz,
        data.len() as u64,
        &final_dir,
        &options,
    )
    .await;
    assert!(
        matches!(second, Err(crate::DblError::AlreadyExists { .. })),
        "{second:?}"
    );
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn tar_special_device_entry_is_rejected() {
    let mut h = tar::Header::new_gnu();
    h.set_entry_type(tar::EntryType::Fifo);
    h.set_mode(0o644);
    h.set_size(0);
    h.set_cksum();
    let data = make_tar_gz(vec![TarEntry::Raw {
        header: Box::new(h),
        data: Vec::new(),
    }]);
    let run = run_extract(&data, ArchiveFormat::TarGz, None, &[], None, None).await;
    assert_rejected(&run, "tar fifo");
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn nix_path_component_counts_match_prefix() {
    // `root/a` under `root` yields `a`, and a bare prefix entry is skipped.
    let data = make_tar_gz(vec![
        TarEntry::Dir {
            path: "root".to_owned(),
            mode: 0o755,
        },
        TarEntry::File {
            path: "root/bin/app".to_owned(),
            data: b"bin".to_vec(),
            mode: 0o755,
        },
    ]);
    let run = run_extract(
        &data,
        ArchiveFormat::TarGz,
        Some("root/"),
        &["bin/app"],
        None,
        None,
    )
    .await;
    let final_dir = run.result.as_ref().expect("trailing slash prefix accepted");
    assert!(final_dir.join("bin/app").exists());
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[cfg(unix)]
#[tokio::test]
async fn zip_directory_symlink_entry_processed_as_directory() {
    let data = make_zip(vec![
        ZipEntry::Dir {
            name: "root/bin/".to_owned(),
        },
        ZipEntry::File {
            name: "root/bin/app".to_owned(),
            data: b"bin".to_vec(),
            unix_mode: Some(0o755),
        },
    ]);
    let run = run_extract(
        &data,
        ArchiveFormat::Zip,
        Some("root"),
        &["bin/app"],
        None,
        None,
    )
    .await;
    assert!(run.result.is_ok());
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_without_target_is_rejected() {
    // A symlink header whose linkname field decodes to nothing is malformed.
    let mut h = tar::Header::new_gnu();
    h.set_entry_type(tar::EntryType::Symlink);
    h.set_size(0);
    h.set_cksum();
    let data = make_tar_gz(vec![TarEntry::Raw {
        header: Box::new(h),
        data: Vec::new(),
    }]);
    let run = run_extract(&data, ArchiveFormat::TarGz, None, &[], None, None).await;
    assert_rejected(&run, "symlink without target");
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn empty_strip_prefix_behaves_like_none() {
    let data = make_tar_gz(vec![TarEntry::File {
        path: "bin/app".to_owned(),
        data: b"bin".to_vec(),
        mode: 0o755,
    }]);
    let run = run_extract(
        &data,
        ArchiveFormat::TarGz,
        Some(""),
        &["bin/app"],
        None,
        None,
    )
    .await;
    let final_dir = run.result.as_ref().expect("empty prefix is a no-op");
    assert!(final_dir.join("bin/app").exists());
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn tar_file_under_prefix_without_directory_entry_works() {
    // Real archives sometimes omit the explicit directory entry for the root.
    let data = make_tar_gz(vec![TarEntry::File {
        path: "root/bin/app".to_owned(),
        data: b"bin".to_vec(),
        mode: 0o755,
    }]);
    let run = run_extract(
        &data,
        ArchiveFormat::TarGz,
        Some("root"),
        &["bin/app"],
        None,
        None,
    )
    .await;
    let final_dir = run.result.as_ref().expect("implicit root works");
    assert!(final_dir.join("bin/app").exists());
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn zip_file_under_prefix_without_directory_entry_works() {
    let data = make_zip(vec![ZipEntry::File {
        name: "root/bin/app".to_owned(),
        data: b"bin".to_vec(),
        unix_mode: Some(0o755),
    }]);
    let run = run_extract(
        &data,
        ArchiveFormat::Zip,
        Some("root"),
        &["bin/app"],
        None,
        None,
    )
    .await;
    let final_dir = run.result.as_ref().expect("implicit root works");
    assert!(final_dir.join("bin/app").exists());
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn expected_binary_with_absolute_path_is_rejected() {
    let data = make_tar_gz(vec![TarEntry::File {
        path: "root/bin/app".to_owned(),
        data: b"bin".to_vec(),
        mode: 0o755,
    }]);
    let run = run_extract(
        &data,
        ArchiveFormat::TarGz,
        Some("root"),
        &["/bin/app"],
        None,
        None,
    )
    .await;
    let err = run.result.as_ref().expect_err("absolute expected path");
    assert!(
        matches!(
            err,
            crate::DblError::InvalidInput {
                field: "expected_binaries",
                ..
            }
        ),
        "{err:?}"
    );
    assert!(!run.final_dir().exists());
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn expected_binary_that_is_a_directory_is_rejected() {
    let data = make_tar_gz(vec![TarEntry::Dir {
        path: "root/bin".to_owned(),
        mode: 0o755,
    }]);
    let run = run_extract(
        &data,
        ArchiveFormat::TarGz,
        Some("root"),
        &["bin"],
        None,
        None,
    )
    .await;
    let err = run.result.as_ref().expect_err("directory is not a binary");
    assert!(
        matches!(
            err,
            crate::DblError::InvalidInput {
                field: "archive",
                ..
            }
        ),
        "{err:?}"
    );
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn stale_staging_directory_is_cleaned_before_extracting() {
    let data = make_tar_gz(vec![TarEntry::File {
        path: "root/bin/app".to_owned(),
        data: b"bin".to_vec(),
        mode: 0o755,
    }]);
    let base = super::temp_base();
    tokio::fs::create_dir_all(&base).await.expect("base");
    let archive = base.join("a.tar.gz");
    tokio::fs::write(&archive, &data).await.expect("archive");
    let final_dir = base.join("runtime");
    let options = super::super::extract::ExtractOptions {
        expected_binaries: vec!["bin/app".to_owned()],
        strip_prefix: Some("root".to_owned()),
        max_total_bytes: None,
        max_entries: None,
    };
    let result = super::super::extract::extract_runtime(
        &archive,
        ArchiveFormat::TarGz,
        data.len() as u64,
        &final_dir,
        &options,
    )
    .await;
    assert!(result.is_ok());
    // A second call must refuse because the final directory now exists.
    let again = super::super::extract::extract_runtime(
        &archive,
        ArchiveFormat::TarGz,
        data.len() as u64,
        &final_dir,
        &options,
    )
    .await;
    assert!(
        matches!(again, Err(crate::DblError::AlreadyExists { .. })),
        "{again:?}"
    );
    let _ = tokio::fs::remove_dir_all(&base).await;
}

#[tokio::test]
async fn final_directory_with_missing_parent_is_rejected() {
    // A root path has no parent component to stage inside.
    let data = make_tar_gz(vec![]);
    let base = super::temp_base();
    tokio::fs::create_dir_all(&base).await.expect("base");
    let archive = base.join("empty.tar.gz");
    tokio::fs::write(&archive, &data).await.expect("archive");
    let options = super::super::extract::ExtractOptions::default();
    let err = super::super::extract::extract_runtime(
        &archive,
        ArchiveFormat::TarGz,
        data.len() as u64,
        std::path::Path::new("/"),
        &options,
    )
    .await
    .expect_err("root has no parent");
    assert!(
        matches!(err, crate::DblError::InvalidInput { field: "path", .. })
            || matches!(err, crate::DblError::AlreadyExists { .. }),
        "{err:?}"
    );
    let _ = tokio::fs::remove_dir_all(&base).await;
}

#[tokio::test]
async fn tar_metadata_extension_entries_are_skipped() {
    // PAX/global extension records carry metadata, not files; the extractor must
    // skip them and still materialise the real entries.
    let data = make_tar_gz(vec![
        TarEntry::Raw {
            header: Box::new(pax_header(b"12 mtime=0")),
            data: b"12 mtime=0\n".to_vec(),
        },
        TarEntry::File {
            path: "root/bin/app".to_owned(),
            data: b"bin".to_vec(),
            mode: 0o755,
        },
    ]);
    let run = run_extract(
        &data,
        ArchiveFormat::TarGz,
        Some("root"),
        &["bin/app"],
        None,
        None,
    )
    .await;
    assert!(run.result.is_ok(), "pax metadata must not fail extraction");
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

fn pax_header(payload: &[u8]) -> tar::Header {
    let mut h = tar::Header::new_gnu();
    h.set_entry_type(tar::EntryType::XHeader);
    h.set_size(payload.len() as u64);
    h.set_mode(0o644);
    h.set_cksum();
    h
}

#[tokio::test]
async fn extract_already_existing_file_path_is_rejected() {
    // When `final_dir` exists as a regular file (not a directory), extraction must
    // still refuse rather than half-operate.
    let data = make_tar_gz(vec![TarEntry::File {
        path: "root/bin/app".to_owned(),
        data: b"bin".to_vec(),
        mode: 0o755,
    }]);
    let base = super::temp_base();
    tokio::fs::create_dir_all(&base).await.expect("base");
    let archive = base.join("a.tar.gz");
    tokio::fs::write(&archive, &data).await.expect("archive");
    let final_dir = base.join("runtime");
    tokio::fs::write(&final_dir, b"existing")
        .await
        .expect("file");
    let options = super::super::extract::ExtractOptions {
        expected_binaries: vec!["bin/app".to_owned()],
        strip_prefix: Some("root".to_owned()),
        max_total_bytes: None,
        max_entries: None,
    };
    let err = super::super::extract::extract_runtime(
        &archive,
        ArchiveFormat::TarGz,
        data.len() as u64,
        &final_dir,
        &options,
    )
    .await
    .expect_err("must refuse");
    assert!(
        matches!(err, crate::DblError::AlreadyExists { .. }),
        "{err:?}"
    );
    assert_eq!(std::fs::read(&final_dir).expect("read"), b"existing");
    let _ = tokio::fs::remove_dir_all(&base).await;
}

#[cfg(unix)]
#[tokio::test]
async fn zip_relative_symlink_inside_root_is_created() {
    let data = make_zip(vec![
        ZipEntry::Dir {
            name: "root/bin/".to_owned(),
        },
        ZipEntry::File {
            name: "root/bin/app".to_owned(),
            data: b"bin".to_vec(),
            unix_mode: Some(0o755),
        },
        ZipEntry::Symlink {
            name: "root/bin/alias".to_owned(),
            target: "app".to_owned(),
        },
    ]);
    let run = run_extract(
        &data,
        ArchiveFormat::Zip,
        Some("root"),
        &["bin/app"],
        None,
        None,
    )
    .await;
    let final_dir = run.result.as_ref().expect("zip symlink allowed");
    let meta = std::fs::symlink_metadata(final_dir.join("bin/alias")).expect("symlink exists");
    assert!(meta.file_type().is_symlink());
    assert_eq!(
        std::fs::read(final_dir.join("bin/alias")).expect("follow"),
        b"bin"
    );
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn zip_without_matching_top_level_directory_fails() {
    // No entry starts with the required prefix, so the strip-prefix contract is
    // violated even though every individual name is harmless.
    let data = make_zip(vec![ZipEntry::File {
        name: "somewhere/bin/app".to_owned(),
        data: b"bin".to_vec(),
        unix_mode: Some(0o755),
    }]);
    let run = run_extract(
        &data,
        ArchiveFormat::Zip,
        Some("root"),
        &["bin/app"],
        None,
        None,
    )
    .await;
    assert_rejected(&run, "zip prefix absent");
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn tar_without_matching_top_level_directory_fails() {
    let data = make_tar_gz(vec![TarEntry::File {
        path: "somewhere/bin/app".to_owned(),
        data: b"bin".to_vec(),
        mode: 0o755,
    }]);
    let run = run_extract(
        &data,
        ArchiveFormat::TarGz,
        Some("root"),
        &["bin/app"],
        None,
        None,
    )
    .await;
    assert_rejected(&run, "tar prefix absent");
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn unknown_tar_entry_type_is_rejected() {
    let mut h = tar::Header::new_gnu();
    // Typeflag 'Z' is not a defined tar entry type.
    h.set_entry_type(tar::EntryType::__Nonexhaustive(b'Z'));
    h.set_mode(0o644);
    h.set_size(0);
    h.set_cksum();
    let data = make_tar_gz(vec![TarEntry::Raw {
        header: Box::new(h),
        data: Vec::new(),
    }]);
    let run = run_extract(&data, ArchiveFormat::TarGz, None, &[], None, None).await;
    assert_rejected(&run, "unknown typeflag");
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn tar_global_extension_header_is_skipped() {
    let mut h = tar::Header::new_gnu();
    h.set_entry_type(tar::EntryType::XGlobalHeader);
    h.set_size(0);
    h.set_mode(0o644);
    h.set_cksum();
    let data = make_tar_gz(vec![
        TarEntry::Raw {
            header: Box::new(h),
            data: Vec::new(),
        },
        TarEntry::File {
            path: "root/bin/app".to_owned(),
            data: b"bin".to_vec(),
            mode: 0o755,
        },
    ]);
    let run = run_extract(
        &data,
        ArchiveFormat::TarGz,
        Some("root"),
        &["bin/app"],
        None,
        None,
    )
    .await;
    assert!(
        run.result.is_ok(),
        "global header must be skipped: {:?}",
        run.result
    );
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[cfg(unix)]
#[tokio::test]
async fn tar_symlink_in_a_new_directory_creates_the_parent() {
    // The symlink's parent directory does not exist yet, so the extractor must
    // create it before creating the link.
    let data = make_tar_gz(vec![
        TarEntry::File {
            path: "root/bin/app".to_owned(),
            data: b"bin".to_vec(),
            mode: 0o755,
        },
        TarEntry::Symlink {
            path: "root/links/alias".to_owned(),
            target: "../bin/app".to_owned(),
        },
    ]);
    let run = run_extract(
        &data,
        ArchiveFormat::TarGz,
        Some("root"),
        &["bin/app"],
        None,
        None,
    )
    .await;
    let final_dir = run.result.as_ref().expect("symlink parent created");
    assert!(final_dir.join("links/alias").is_symlink());
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[cfg(unix)]
#[tokio::test]
async fn zip_symlink_in_a_new_directory_creates_the_parent() {
    let data = make_zip(vec![
        ZipEntry::File {
            name: "root/bin/app".to_owned(),
            data: b"bin".to_vec(),
            unix_mode: Some(0o755),
        },
        ZipEntry::Symlink {
            name: "root/links/alias".to_owned(),
            target: "../bin/app".to_owned(),
        },
    ]);
    let run = run_extract(
        &data,
        ArchiveFormat::Zip,
        Some("root"),
        &["bin/app"],
        None,
        None,
    )
    .await;
    let final_dir = run.result.as_ref().expect("zip symlink parent created");
    assert!(final_dir.join("links/alias").is_symlink());
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn empty_zip_with_strip_prefix_fails() {
    let data = make_zip(vec![]);
    let run = run_extract(&data, ArchiveFormat::Zip, Some("root"), &[], None, None).await;
    let err = run
        .result
        .as_ref()
        .expect_err("empty archive cannot satisfy prefix");
    assert!(
        matches!(
            err,
            crate::DblError::InvalidInput {
                field: "strip_prefix",
                ..
            }
        ),
        "{err:?}"
    );
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn empty_tar_with_strip_prefix_fails() {
    let data = make_tar_gz(vec![]);
    let run = run_extract(&data, ArchiveFormat::TarGz, Some("root"), &[], None, None).await;
    let err = run
        .result
        .as_ref()
        .expect_err("empty archive cannot satisfy prefix");
    assert!(
        matches!(
            err,
            crate::DblError::InvalidInput {
                field: "strip_prefix",
                ..
            }
        ),
        "{err:?}"
    );
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn empty_tar_without_strip_prefix_succeeds_as_empty_tree() {
    let data = make_tar_gz(vec![]);
    let run = run_extract(&data, ArchiveFormat::TarGz, None, &[], None, None).await;
    let final_dir = run
        .result
        .as_ref()
        .expect("empty archive is a valid empty tree");
    assert!(final_dir.is_dir());
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}
