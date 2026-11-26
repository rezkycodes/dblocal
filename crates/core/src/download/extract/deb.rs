//! Pure-Rust Debian `.deb` (`ar`) extractor.
//!
//! Parses the `ar` container with zero `unsafe` code, validates the expected
//! Debian member layout (`debian-binary`, `control.tar.*`, `data.tar.{zst,xz,gz}`),
//! and streams the `data.tar.*` payload directly into [`super::tarball::extract_tar`]
//! so every existing zip-slip, symlink-escape, hard-link and bomb-cap guard
//! applies unchanged.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::{DblError, Result};

use super::ExtractCaps;
use super::tarball;

/// Global `ar` archive magic header (8 bytes).
const AR_MAGIC: &[u8; 8] = b"!<arch>\n";
/// Size of one `ar` member header in bytes.
const AR_HEADER_LEN: usize = 60;
/// Trailer bytes at the end of every `ar` member header.
const AR_HEADER_END: &[u8; 2] = b"`\n";
/// Maximum allowed size of `debian-binary` (expected `2.0\n`, 4 bytes).
const MAX_DEBIAN_BINARY_BYTES: u64 = 64;
/// Maximum allowed size of `control.tar.*` metadata (skipped, never extracted).
const MAX_CONTROL_MEMBER_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DataCompression {
    Zst,
    Xz,
    Gz,
}

#[derive(Debug, Clone, Copy)]
struct DataMember {
    offset: u64,
    size: u64,
    compression: DataCompression,
}

pub(super) fn extract_deb(
    archive_path: &Path,
    staging: &Path,
    strip_prefix: Option<&str>,
    caps: &ExtractCaps,
) -> Result<()> {
    let mut file = std::fs::File::open(archive_path).map_err(|source| DblError::Io { source })?;
    let file_len = file
        .metadata()
        .map_err(|source| DblError::Io { source })?
        .len();

    let data_member = locate_data_member(&mut file, file_len, caps)?;
    file.seek(SeekFrom::Start(data_member.offset))
        .map_err(|source| DblError::Io { source })?;
    let limited = file.take(data_member.size);

    match data_member.compression {
        DataCompression::Zst => {
            let dec = ruzstd::decoding::StreamingDecoder::new(limited).map_err(|err| {
                DblError::InvalidInput {
                    field: "archive",
                    reason: format!("invalid zstd stream in .deb data.tar.zst: {err}"),
                }
            })?;
            tarball::extract_tar(dec, staging, strip_prefix, caps)
        }
        DataCompression::Xz => {
            let dec = liblzma::read::XzDecoder::new(limited);
            tarball::extract_tar(dec, staging, strip_prefix, caps)
        }
        DataCompression::Gz => {
            let dec = flate2::read::GzDecoder::new(limited);
            tarball::extract_tar(dec, staging, strip_prefix, caps)
        }
    }
}

fn locate_data_member<R: Read + Seek>(
    reader: &mut R,
    file_len: u64,
    caps: &ExtractCaps,
) -> Result<DataMember> {
    let mut magic = [0u8; 8];
    reader
        .read_exact(&mut magic)
        .map_err(|_| invalid_archive("not a valid .deb archive: truncated or missing ar magic"))?;
    if &magic != AR_MAGIC {
        return Err(invalid_archive(
            "not a valid .deb archive: wrong ar magic header",
        ));
    }

    let mut pos: u64 = AR_MAGIC.len() as u64;
    let mut saw_debian_binary = false;
    let mut data_member: Option<DataMember> = None;

    while pos < file_len {
        if file_len - pos < AR_HEADER_LEN as u64 {
            return Err(invalid_archive("truncated ar member header in .deb"));
        }
        let mut header = [0u8; AR_HEADER_LEN];
        reader
            .read_exact(&mut header)
            .map_err(|_| invalid_archive("truncated ar member header in .deb"))?;
        pos += AR_HEADER_LEN as u64;

        let (name, size) = parse_ar_header(&header)?;
        let member_end = pos
            .checked_add(size)
            .ok_or_else(|| invalid_archive("ar member size overflow in .deb"))?;
        if member_end > file_len {
            return Err(invalid_archive(format!(
                "truncated .deb archive: member '{name}' claims {size} bytes past end of file"
            )));
        }

        if !saw_debian_binary {
            if name != "debian-binary" {
                return Err(DblError::UnsafeArchive {
                    entry: format!("first .deb member must be debian-binary, got '{name}'"),
                });
            }
            if size > MAX_DEBIAN_BINARY_BYTES {
                return Err(DblError::UnsafeArchive {
                    entry: format!("debian-binary member is oversized ({size} bytes)"),
                });
            }
            let mut version_buf = vec![0u8; size as usize];
            reader
                .read_exact(&mut version_buf)
                .map_err(|_| invalid_archive("truncated debian-binary member"))?;
            if !version_buf.starts_with(b"2.") {
                return Err(invalid_archive("unsupported .deb debian-binary version"));
            }
            saw_debian_binary = true;
        } else if let Some(compression) = classify_data_member(&name) {
            if size > caps.max_total_bytes {
                return Err(DblError::UnsafeArchive {
                    entry: format!(
                        "{name}: member size ({size} bytes) exceeds maximum ({} bytes)",
                        caps.max_total_bytes
                    ),
                });
            }
            if data_member.is_some() {
                return Err(DblError::UnsafeArchive {
                    entry: format!("duplicate data.tar member '{name}' in .deb"),
                });
            }
            data_member = Some(DataMember {
                offset: pos,
                size,
                compression,
            });
            reader
                .seek(SeekFrom::Start(member_end))
                .map_err(|source| DblError::Io { source })?;
        } else if is_control_member(&name) {
            if size > MAX_CONTROL_MEMBER_BYTES {
                return Err(DblError::UnsafeArchive {
                    entry: format!("{name}: control member is oversized ({size} bytes)"),
                });
            }
            reader
                .seek(SeekFrom::Start(member_end))
                .map_err(|source| DblError::Io { source })?;
        } else {
            return Err(DblError::UnsafeArchive {
                entry: format!("unexpected .deb member '{name}'"),
            });
        }

        pos = member_end;
        // ar members are 2-byte aligned; odd-sized members are followed by a single '\n'.
        if size % 2 == 1 && pos < file_len {
            let mut pad = [0u8; 1];
            reader
                .read_exact(&mut pad)
                .map_err(|_| invalid_archive("truncated ar padding byte in .deb"))?;
            pos += 1;
        }
    }

    data_member.ok_or_else(|| {
        invalid_archive(".deb archive is missing a supported data.tar.{zst,xz,gz} member")
    })
}

fn parse_ar_header(header: &[u8; AR_HEADER_LEN]) -> Result<(String, u64)> {
    if &header[58..60] != AR_HEADER_END {
        return Err(invalid_archive(
            "malformed ar member header trailer in .deb",
        ));
    }
    let raw_name = std::str::from_utf8(&header[0..16])
        .map_err(|_| invalid_archive("non-UTF-8 ar member name in .deb"))?;
    let trimmed = raw_name.trim_end_matches(' ');
    let name = trimmed.strip_suffix('/').unwrap_or(trimmed);
    if name.is_empty() {
        return Err(DblError::UnsafeArchive {
            entry: "empty ar member name in .deb".to_owned(),
        });
    }
    let raw_size = std::str::from_utf8(&header[48..58])
        .map_err(|_| invalid_archive("non-UTF-8 ar member size in .deb"))?
        .trim();
    let size = raw_size
        .parse::<u64>()
        .map_err(|_| invalid_archive(format!("invalid ar member size '{raw_size}' in .deb")))?;
    Ok((name.to_owned(), size))
}

fn classify_data_member(name: &str) -> Option<DataCompression> {
    match name {
        "data.tar.zst" => Some(DataCompression::Zst),
        "data.tar.xz" => Some(DataCompression::Xz),
        "data.tar.gz" => Some(DataCompression::Gz),
        _ => None,
    }
}

fn is_control_member(name: &str) -> bool {
    matches!(
        name,
        "control.tar" | "control.tar.gz" | "control.tar.xz" | "control.tar.zst"
    )
}

fn invalid_archive(reason: impl Into<String>) -> DblError {
    DblError::InvalidInput {
        field: "archive",
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::io::Write;

    use tempfile::tempdir;

    use super::*;

    fn push_ar_member(out: &mut Vec<u8>, name: &str, data: &[u8]) {
        assert!(name.len() <= 16);
        let header = format!(
            "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
            name,
            "0",
            "0",
            "0",
            "100644",
            data.len()
        );
        assert_eq!(header.len(), AR_HEADER_LEN);
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(data);
        if data.len() % 2 == 1 {
            out.push(b'\n');
        }
    }

    fn build_deb(members: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(AR_MAGIC);
        for (name, data) in members {
            push_ar_member(&mut out, name, data);
        }
        out
    }

    fn build_tar_bytes(entries: &[(&str, &[u8], Option<&str>)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, data, symlink_target) in entries {
            let mut header = tar::Header::new_gnu();
            if let Some(target) = symlink_target {
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_mode(0o777);
                header.set_size(0);
                header.set_cksum();
                builder
                    .append_link(&mut header, path, target)
                    .expect("append symlink");
            } else if path.ends_with('/') {
                header.set_entry_type(tar::EntryType::Directory);
                header.set_mode(0o755);
                header.set_size(0);
                header.set_cksum();
                builder
                    .append_data(&mut header, path, std::io::empty())
                    .expect("append dir");
            } else {
                header.set_entry_type(tar::EntryType::Regular);
                header.set_mode(0o755);
                header.set_size(data.len() as u64);
                header.set_cksum();
                builder
                    .append_data(&mut header, path, *data)
                    .expect("append file");
            }
        }
        builder.into_inner().expect("finish tar")
    }

    fn compress_gz(raw: &[u8]) -> Vec<u8> {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(raw).expect("gz write");
        enc.finish().expect("gz finish")
    }

    fn compress_xz(raw: &[u8]) -> Vec<u8> {
        let mut enc = liblzma::write::XzEncoder::new(Vec::new(), 1);
        enc.write_all(raw).expect("xz write");
        enc.finish().expect("xz finish")
    }

    fn compress_zst(raw: &[u8]) -> Vec<u8> {
        ruzstd::encoding::compress_to_vec(raw, ruzstd::encoding::CompressionLevel::Fastest)
    }

    fn default_caps() -> ExtractCaps {
        ExtractCaps {
            max_total_bytes: 10 * 1024 * 1024,
            max_entries: 1_000,
        }
    }

    #[test]
    fn extracts_deb_with_zst_xz_and_gz_data_members() {
        let tar_bytes = build_tar_bytes(&[
            ("./", b"", None),
            ("./usr/", b"", None),
            ("./usr/bin/", b"", None),
            ("./usr/bin/redis-check-rdb", b"ELF-fake-server", None),
            ("./usr/bin/redis-cli", b"ELF-fake-cli", None),
        ]);

        for (member_name, payload) in [
            ("data.tar.zst", compress_zst(&tar_bytes)),
            ("data.tar.xz", compress_xz(&tar_bytes)),
            ("data.tar.gz", compress_gz(&tar_bytes)),
        ] {
            let deb = build_deb(&[
                ("debian-binary", b"2.0\n"),
                ("control.tar.gz", &compress_gz(b"")),
                (member_name, &payload),
            ]);
            let dir = tempdir().expect("tempdir");
            let deb_path = dir.path().join("pkg.deb");
            let staging = dir.path().join("staging");
            std::fs::create_dir_all(&staging).expect("staging");
            std::fs::write(&deb_path, &deb).expect("write deb");

            extract_deb(&deb_path, &staging, Some("usr"), &default_caps())
                .unwrap_or_else(|e| panic!("{member_name}: {e}"));
            assert_eq!(
                std::fs::read(staging.join("bin/redis-check-rdb")).expect("rdb"),
                b"ELF-fake-server"
            );
            assert_eq!(
                std::fs::read(staging.join("bin/redis-cli")).expect("cli"),
                b"ELF-fake-cli"
            );
        }
    }

    #[test]
    fn rejects_wrong_magic() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("bad.deb");
        std::fs::write(&path, b"not-an-ar-archive").expect("write");
        let err = extract_deb(&path, dir.path(), None, &default_caps()).unwrap_err();
        assert!(
            matches!(
                err,
                DblError::InvalidInput {
                    field: "archive",
                    ..
                }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn rejects_truncated_archive() {
        let tar_bytes = build_tar_bytes(&[("./usr/bin/a", b"hello", None)]);
        let gz = compress_gz(&tar_bytes);
        let mut deb = build_deb(&[("debian-binary", b"2.0\n"), ("data.tar.gz", &gz)]);
        deb.truncate(deb.len() - 5);

        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("trunc.deb");
        std::fs::write(&path, &deb).expect("write");
        let err = extract_deb(&path, dir.path(), Some("usr"), &default_caps()).unwrap_err();
        assert!(
            matches!(
                err,
                DblError::InvalidInput {
                    field: "archive",
                    ..
                }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn rejects_unknown_member() {
        let tar_bytes = build_tar_bytes(&[("./usr/bin/a", b"hello", None)]);
        let gz = compress_gz(&tar_bytes);
        let deb = build_deb(&[
            ("debian-binary", b"2.0\n"),
            ("evil.sh", b"echo pwn"),
            ("data.tar.gz", &gz),
        ]);
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("unknown.deb");
        std::fs::write(&path, &deb).expect("write");
        let err = extract_deb(&path, dir.path(), Some("usr"), &default_caps()).unwrap_err();
        assert!(matches!(err, DblError::UnsafeArchive { .. }), "{err:?}");
    }

    #[test]
    fn rejects_oversized_member() {
        let tar_bytes = build_tar_bytes(&[("./usr/bin/a", &vec![b'A'; 4096], None)]);
        let gz = compress_gz(&tar_bytes);
        let deb = build_deb(&[("debian-binary", b"2.0\n"), ("data.tar.gz", &gz)]);
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("big.deb");
        std::fs::write(&path, &deb).expect("write");
        let tight = ExtractCaps {
            max_total_bytes: 64,
            max_entries: 100,
        };
        let err = extract_deb(&path, dir.path(), Some("usr"), &tight).unwrap_err();
        assert!(matches!(err, DblError::UnsafeArchive { .. }), "{err:?}");
    }

    #[test]
    fn rejects_traversal_and_absolute_paths_inside_deb() {
        let mut h = tar::Header::new_gnu();
        h.as_old_mut().name[..11].copy_from_slice(b"../evil.txt");
        h.set_entry_type(tar::EntryType::Regular);
        h.set_mode(0o644);
        h.set_size(1);
        h.set_cksum();
        let mut builder = tar::Builder::new(Vec::new());
        builder.append(&h, &b"x"[..]).expect("append");
        let tar_bytes = builder.into_inner().expect("finish");

        let deb = build_deb(&[
            ("debian-binary", b"2.0\n"),
            ("data.tar.zst", &compress_zst(&tar_bytes)),
        ]);
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("traversal.deb");
        std::fs::write(&path, &deb).expect("write");
        let err = extract_deb(&path, dir.path(), None, &default_caps()).unwrap_err();
        assert!(matches!(err, DblError::UnsafeArchive { .. }), "{err:?}");

        // Absolute path inside data.tar.gz
        let mut h2 = tar::Header::new_gnu();
        h2.as_old_mut().name[..9].copy_from_slice(b"/etc/pass");
        h2.set_entry_type(tar::EntryType::Regular);
        h2.set_mode(0o644);
        h2.set_size(1);
        h2.set_cksum();
        let mut builder2 = tar::Builder::new(Vec::new());
        builder2.append(&h2, &b"x"[..]).expect("append");
        let tar_abs = builder2.into_inner().expect("finish");

        let deb_abs = build_deb(&[
            ("debian-binary", b"2.0\n"),
            ("data.tar.gz", &compress_gz(&tar_abs)),
        ]);
        let path_abs = dir.path().join("abs.deb");
        std::fs::write(&path_abs, &deb_abs).expect("write");
        let err_abs = extract_deb(&path_abs, dir.path(), None, &default_caps()).unwrap_err();
        assert!(
            matches!(err_abs, DblError::UnsafeArchive { .. }),
            "{err_abs:?}"
        );
    }
}
