//! MySQL builds, whose checksums must be computed (no upstream SHA-256).

use sha2::{Digest, Sha256};

use super::helpers::{hex, parse_md5_sidecar};
use super::{Arch, CatalogArtifact, Os, Source, SourceError};
use dblocal_core::model::ArchiveFormat;

/// One MySQL build the builder knows how to describe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MysqlBuild {
    /// File name on the CDN.
    pub file_name: String,
    /// Target OS.
    pub os: Os,
    /// Target architecture.
    pub arch: Arch,
    /// Archive format.
    pub format: ArchiveFormat,
    /// Base URL without the file name.
    pub base_url: String,
}

/// Base URL of a MySQL CDN line (`8.4`, `9.7`, `26.10`, `8.0`).
///
/// Oracle names the directory after the release line, which is exactly
/// [`dblocal_core::model::version_line`] for MySQL.
pub fn mysql_base_url(line: &str) -> String {
    format!("https://cdn.mysql.com/Downloads/MySQL-{line}")
}

/// The published builds for one tracked MySQL version.
///
/// Oracle publishes a `-minimal` Linux `x86_64` tarball, a `macos15` arm64
/// tarball and a `winx64` zip for every line. macOS `x86_64` is the one target
/// Oracle drops on some 8.4/26.10 releases (only arm64 exists), so it is
/// optional: the picker then shows macOS x86_64 as unavailable rather than
/// pointing at a 404. No line publishes a Linux `aarch64` minimal tarball, so
/// that target is not claimed either, instead of a ~900 MB full tarball.
pub fn mysql_builds_for(tracked: &super::tracked::TrackedVersion) -> Vec<MysqlBuild> {
    let version = tracked.version;
    let base_url = mysql_base_url(tracked.line);
    let file = |name: String, os: Os, arch: Arch, format: ArchiveFormat| MysqlBuild {
        file_name: name,
        os,
        arch,
        format,
        base_url: base_url.clone(),
    };

    // 8.0 minimal archives are linked against glibc 2.17; 8.4 and newer 2.28.
    let glibc = if tracked.line == "8.0" {
        "2.17"
    } else {
        "2.28"
    };

    vec![
        file(
            format!("mysql-{version}-linux-glibc{glibc}-x86_64-minimal.tar.xz"),
            Os::Linux,
            Arch::X86_64,
            ArchiveFormat::TarXz,
        ),
        file(
            format!("mysql-{version}-macos15-arm64.tar.gz"),
            Os::Macos,
            Arch::Aarch64,
            ArchiveFormat::TarGz,
        ),
        file(
            format!("mysql-{version}-macos15-x86_64.tar.gz"),
            Os::Macos,
            Arch::X86_64,
            ArchiveFormat::TarGz,
        ),
        file(
            format!("mysql-{version}-winx64.zip"),
            Os::Windows,
            Arch::X86_64,
            ArchiveFormat::Zip,
        ),
    ]
}

/// Whether a MySQL build's 404 means "this target is not published".
///
/// Only macOS `x86_64` is dropped per release; every other target must exist.
pub fn mysql_target_optional(build: &MysqlBuild) -> bool {
    build.os == Os::Macos && build.arch == Arch::X86_64
}

/// Builds a MySQL artifact by downloading the archive once and hashing it.
///
/// Oracle publishes no SHA-256 for MySQL, only `.md5` and a GPG signature. The
/// hash is computed here and the `.md5` sidecar must agree before it is
/// recorded.
pub fn mysql_artifact(
    build: &MysqlBuild,
    version: &str,
    source: &dyn Source,
) -> Result<CatalogArtifact, SourceError> {
    let url = format!("{}/{}", build.base_url, build.file_name);
    let md5_url = format!("{url}.md5");
    let md5_text = source.get_text(&md5_url)?;
    let expected_md5 = parse_md5_sidecar(&md5_text, &build.file_name)?;

    let bytes = source.get_bytes(&url)?;
    if bytes.is_empty() {
        return Err(SourceError::new(format!(
            "MySQL archive {} downloaded as an empty body",
            build.file_name
        )));
    }
    let actual_md5 = hex(&md5_compute(&bytes));
    if actual_md5 != expected_md5 {
        return Err(SourceError::new(format!(
            "MySQL archive {} failed the .md5 cross-check ({actual_md5} != {expected_md5})",
            build.file_name
        )));
    }

    let sha256 = hex(&Sha256::digest(&bytes));
    Ok(CatalogArtifact {
        id: format!(
            "mysql-{version}-{}-{}",
            build.os.as_str(),
            build.arch.as_str()
        ),
        engine: "mysql".to_owned(),
        version: version.to_owned(),
        os: build.os.as_str().to_owned(),
        arch: build.arch.as_str().to_owned(),
        url,
        sha256,
        size_bytes: bytes.len() as u64,
        format: build.format.as_str().to_owned(),
        // Every MySQL archive has exactly one top-level directory named after
        // the archive stem (`mysql-8.4.11-linux-glibc2.28-x86_64-minimal/`,
        // `mysql-8.4.11-macos15-arm64/`, `mysql-8.4.11-winx64/`), not
        // `mysql-{version}-minimal`: the old constant matched no real archive,
        // so `bin/` would land one level too deep and `expected_binaries` would
        // fail after a real download.
        strip_prefix: Some(archive_root(&build.file_name)?),
        min_glibc: None,
        variant: None,
    })
}

/// The single top-level directory a MySQL archive unpacks into.
///
/// Derived from the archive name by dropping its extension; MySQL 8.4 archives
/// are `.tar.xz`, `.tar.gz` or `.zip` and every one names its root after the
/// stem, so the strip prefix must too.
pub fn archive_root(file_name: &str) -> Result<String, SourceError> {
    super::helpers::archive_root(file_name)
        .ok_or_else(|| SourceError::new(format!("MySQL archive {file_name} has no known format")))
}

/// The `.md5` of `bytes`, as the sidecar would publish it.
///
/// Used by the offline builder tests to serve a genuine `.md5` sidecar for the
/// tiny fixture archive.
#[cfg(test)]
pub fn md5_hex(bytes: &[u8]) -> String {
    hex(&md5_compute(bytes))
}

/// Small, dependency-free MD5 used only to cross-check MySQL's `.md5` sidecar.
///
/// MD5 is broken for collision resistance; it is used here purely as a
/// consistency check. The catalog always stores the SHA-256.
pub fn md5_compute(input: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];

    let mut message = input.to_vec();
    let bit_len = (input.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_le_bytes());

    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;

    for chunk in message.chunks_exact(64) {
        let mut words = [0u32; 16];
        for (index, word) in words.iter_mut().enumerate() {
            let start = index * 4;
            *word = u32::from_le_bytes([
                chunk[start],
                chunk[start + 1],
                chunk[start + 2],
                chunk[start + 3],
            ]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | (!b & d), i),
                16..=31 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(K[i]).wrapping_add(words[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}
