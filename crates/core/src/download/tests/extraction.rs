//! Archive extraction, part 1: zip-slip, path policy and bomb caps.
//!
//! Every archive is built in memory and extracted into a temp directory; the
//! assertions check both the typed error and the absence of any half-installed
//! runtime.

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::model::ArchiveFormat;

use super::{TarEntry, ZipEntry, assert_rejected, make_tar_gz, make_tar_xz, make_zip, run_extract};

#[tokio::test]
async fn zip_slip_parent_dir_rejected_tar_gz() {
    let mut h = tar::Header::new_gnu();
    h.as_old_mut().name[..11].copy_from_slice(b"../evil.txt");
    h.set_entry_type(tar::EntryType::Regular);
    h.set_mode(0o644);
    h.set_size(1);
    h.set_cksum();
    let data = make_tar_gz(vec![TarEntry::Raw {
        header: Box::new(h),
        data: b"x".to_vec(),
    }]);
    let run = run_extract(&data, ArchiveFormat::TarGz, None, &[], None, None).await;
    assert_rejected(&run, "tar.gz ../zip-slip");
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn zip_slip_parent_dir_rejected_tar_xz() {
    let mut h = tar::Header::new_gnu();
    h.as_old_mut().name[..11].copy_from_slice(b"../evil.txt");
    h.set_entry_type(tar::EntryType::Regular);
    h.set_mode(0o644);
    h.set_size(1);
    h.set_cksum();
    let data = make_tar_xz(vec![TarEntry::Raw {
        header: Box::new(h),
        data: b"x".to_vec(),
    }]);
    let run = run_extract(&data, ArchiveFormat::TarXz, None, &[], None, None).await;
    assert_rejected(&run, "tar.xz ../zip-slip");
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn zip_slip_parent_dir_rejected_zip() {
    let data = make_zip(vec![ZipEntry::File {
        name: "../evil.txt".to_owned(),
        data: b"x".to_vec(),
        unix_mode: Some(0o644),
    }]);
    let run = run_extract(&data, ArchiveFormat::Zip, None, &[], None, None).await;
    assert_rejected(&run, "zip ../zip-slip");
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn absolute_path_rejected_tar_gz() {
    let mut h = tar::Header::new_gnu();
    h.as_old_mut().name[..9].copy_from_slice(b"/etc/pass");
    h.set_entry_type(tar::EntryType::Regular);
    h.set_mode(0o644);
    h.set_size(1);
    h.set_cksum();
    let data = make_tar_gz(vec![TarEntry::Raw {
        header: Box::new(h),
        data: b"x".to_vec(),
    }]);
    let run = run_extract(&data, ArchiveFormat::TarGz, None, &[], None, None).await;
    assert_rejected(&run, "tar.gz absolute path");
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn absolute_path_rejected_zip() {
    let data = make_zip(vec![ZipEntry::File {
        name: "/abs/path.txt".to_owned(),
        data: b"x".to_vec(),
        unix_mode: Some(0o644),
    }]);
    let run = run_extract(&data, ArchiveFormat::Zip, None, &[], None, None).await;
    assert_rejected(&run, "zip absolute path");
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn windows_drive_prefix_rejected_zip() {
    let data = make_zip(vec![ZipEntry::File {
        name: "C:/Windows/system32/evil.dll".to_owned(),
        data: b"x".to_vec(),
        unix_mode: Some(0o644),
    }]);
    let run = run_extract(&data, ArchiveFormat::Zip, None, &[], None, None).await;
    assert_rejected(&run, "zip windows drive");
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn unc_prefix_rejected_zip() {
    let data = make_zip(vec![ZipEntry::File {
        name: "//server/share/evil".to_owned(),
        data: b"x".to_vec(),
        unix_mode: Some(0o644),
    }]);
    let run = run_extract(&data, ArchiveFormat::Zip, None, &[], None, None).await;
    assert_rejected(&run, "zip UNC");
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn symlink_escape_rejected_tar_gz() {
    let data = make_tar_gz(vec![
        TarEntry::Dir {
            path: "root".to_owned(),
            mode: 0o755,
        },
        TarEntry::Symlink {
            path: "root/escape".to_owned(),
            target: "../../outside".to_owned(),
        },
    ]);
    let run = run_extract(&data, ArchiveFormat::TarGz, Some("root"), &[], None, None).await;
    assert_rejected(&run, "tar.gz symlink escape");
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn symlink_escape_rejected_zip() {
    let data = make_zip(vec![
        ZipEntry::Dir {
            name: "root/".to_owned(),
        },
        ZipEntry::Symlink {
            name: "root/escape".to_owned(),
            target: "../../outside".to_owned(),
        },
    ]);
    let run = run_extract(&data, ArchiveFormat::Zip, Some("root"), &[], None, None).await;
    assert_rejected(&run, "zip symlink escape");
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn absolute_symlink_target_rejected_tar_gz() {
    let data = make_tar_gz(vec![
        TarEntry::Dir {
            path: "root".to_owned(),
            mode: 0o755,
        },
        TarEntry::Symlink {
            path: "root/abs".to_owned(),
            target: "/etc/passwd".to_owned(),
        },
    ]);
    let run = run_extract(&data, ArchiveFormat::TarGz, Some("root"), &[], None, None).await;
    assert_rejected(&run, "tar.gz absolute symlink");
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn hard_link_rejected_tar_gz() {
    let data = make_tar_gz(vec![
        TarEntry::File {
            path: "root/a".to_owned(),
            data: b"x".to_vec(),
            mode: 0o644,
        },
        TarEntry::Hardlink {
            path: "root/b".to_owned(),
            target: "root/a".to_owned(),
        },
    ]);
    let run = run_extract(&data, ArchiveFormat::TarGz, Some("root"), &[], None, None).await;
    assert_rejected(&run, "tar.gz hard link");
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn hard_link_rejected_tar_xz() {
    let data = make_tar_xz(vec![
        TarEntry::File {
            path: "root/a".to_owned(),
            data: b"x".to_vec(),
            mode: 0o644,
        },
        TarEntry::Hardlink {
            path: "root/b".to_owned(),
            target: "root/a".to_owned(),
        },
    ]);
    let run = run_extract(&data, ArchiveFormat::TarXz, Some("root"), &[], None, None).await;
    assert_rejected(&run, "tar.xz hard link");
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn entry_count_bomb_rejected() {
    let entries = (0..25)
        .map(|i| TarEntry::File {
            path: format!("root/f{i}"),
            data: vec![0u8; 1],
            mode: 0o644,
        })
        .collect();
    let data = make_tar_gz(entries);
    let run = run_extract(
        &data,
        ArchiveFormat::TarGz,
        Some("root"),
        &[],
        None,
        Some(10),
    )
    .await;
    assert_rejected(&run, "entry-count bomb");
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn total_bytes_bomb_rejected() {
    let data = make_tar_gz(vec![TarEntry::File {
        path: "root/big".to_owned(),
        data: vec![b'a'; 4096],
        mode: 0o644,
    }]);
    let run = run_extract(
        &data,
        ArchiveFormat::TarGz,
        Some("root"),
        &[],
        Some(100),
        None,
    )
    .await;
    assert_rejected(&run, "total-bytes bomb");
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn strip_prefix_mismatch_rejected() {
    let data = make_tar_gz(vec![TarEntry::File {
        path: "otherroot/bin/app".to_owned(),
        data: b"x".to_vec(),
        mode: 0o755,
    }]);
    let run = run_extract(
        &data,
        ArchiveFormat::TarGz,
        Some("expectedroot"),
        &["bin/app"],
        None,
        None,
    )
    .await;
    assert_rejected(&run, "strip_prefix mismatch");
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn strip_prefix_sibling_name_not_stripped() {
    // `rootx` must not be treated as being under `root`.
    let data = make_tar_gz(vec![
        TarEntry::File {
            path: "root/bin/app".to_owned(),
            data: b"bin".to_vec(),
            mode: 0o755,
        },
        TarEntry::File {
            path: "rootx/bin/other".to_owned(),
            data: b"other".to_vec(),
            mode: 0o644,
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
    assert_rejected(&run, "strip_prefix sibling");
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[cfg(unix)]
#[tokio::test]
async fn executable_bit_preserved_tar_gz() {
    let data = make_tar_gz(vec![
        TarEntry::Dir {
            path: "root/bin".to_owned(),
            mode: 0o755,
        },
        TarEntry::File {
            path: "root/bin/app".to_owned(),
            data: b"#!/bin/sh\n".to_vec(),
            mode: 0o755,
        },
        TarEntry::File {
            path: "root/bin/data".to_owned(),
            data: b"readme".to_vec(),
            mode: 0o644,
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
    let final_dir = run.result.as_ref().expect("extract ok");
    let app_mode = std::fs::metadata(final_dir.join("bin/app"))
        .expect("meta")
        .permissions()
        .mode();
    let data_mode = std::fs::metadata(final_dir.join("bin/data"))
        .expect("meta")
        .permissions()
        .mode();
    assert_eq!(app_mode & 0o111, 0o111, "binary must stay executable");
    assert_eq!(data_mode & 0o111, 0, "plain file must not gain +x");
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[cfg(unix)]
#[tokio::test]
async fn executable_bit_preserved_zip() {
    let data = make_zip(vec![
        ZipEntry::Dir {
            name: "root/bin/".to_owned(),
        },
        ZipEntry::File {
            name: "root/bin/app".to_owned(),
            data: b"#!/bin/sh\n".to_vec(),
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
    let final_dir = run.result.as_ref().expect("extract ok");
    let mode = std::fs::metadata(final_dir.join("bin/app"))
        .expect("meta")
        .permissions()
        .mode();
    assert_eq!(mode & 0o111, 0o111);
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[cfg(unix)]
#[tokio::test]
async fn relative_symlink_inside_root_allowed() {
    let data = make_tar_gz(vec![
        TarEntry::Dir {
            path: "root".to_owned(),
            mode: 0o755,
        },
        TarEntry::Dir {
            path: "root/bin".to_owned(),
            mode: 0o755,
        },
        TarEntry::File {
            path: "root/bin/app".to_owned(),
            data: b"bin".to_vec(),
            mode: 0o755,
        },
        TarEntry::Symlink {
            path: "root/bin/alias".to_owned(),
            target: "app".to_owned(),
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
    let final_dir = run.result.as_ref().expect("extract ok");
    let meta = std::fs::symlink_metadata(final_dir.join("bin/alias")).expect("symlink");
    assert!(meta.file_type().is_symlink());
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[tokio::test]
async fn final_directory_absent_after_failed_extract() {
    let data = make_tar_gz(vec![TarEntry::File {
        path: "root/README".to_owned(),
        data: b"hi".to_vec(),
        mode: 0o644,
    }]);
    let run = run_extract(
        &data,
        ArchiveFormat::TarGz,
        Some("root"),
        &["bin/missing"],
        None,
        None,
    )
    .await;
    assert!(run.result.is_err());
    assert!(!run.final_dir().exists());
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[cfg(unix)]
#[tokio::test]
async fn file_through_a_previously_extracted_symlink_dir_is_rejected() {
    // `root/lib` is a symlink to another in-tree directory. A later entry that
    // writes `root/lib/payload` must be refused: writing through an extracted
    // symlink is exactly how a benign-looking archive escapes its root once an
    // attacker can also control the link target.
    let data = make_tar_gz(vec![
        TarEntry::Dir {
            path: "root/sub".to_owned(),
            mode: 0o755,
        },
        TarEntry::Symlink {
            path: "root/lib".to_owned(),
            target: "sub".to_owned(),
        },
        TarEntry::File {
            path: "root/lib/payload".to_owned(),
            data: b"pwn".to_vec(),
            mode: 0o644,
        },
    ]);
    let run = run_extract(&data, ArchiveFormat::TarGz, Some("root"), &[], None, None).await;
    assert_rejected(&run, "file through symlink dir");
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}

#[cfg(unix)]
#[tokio::test]
async fn walking_symlinks_cannot_reach_outside_the_root() {
    // Two links chained so that a naive resolver would land outside; the second
    // link's target resolves through the first and must be caught.
    let data = make_tar_gz(vec![
        TarEntry::Dir {
            path: "root/inner".to_owned(),
            mode: 0o755,
        },
        TarEntry::Symlink {
            path: "root/inner/up".to_owned(),
            target: "../..".to_owned(),
        },
        TarEntry::File {
            path: "root/inner/file".to_owned(),
            data: b"ok".to_vec(),
            mode: 0o644,
        },
    ]);
    let run = run_extract(&data, ArchiveFormat::TarGz, Some("root"), &[], None, None).await;
    // `../..` from `root/inner` is the staging root's parent: outside.
    assert_rejected(&run, "chained symlink escape");
    let _ = tokio::fs::remove_dir_all(&run.base).await;
}
