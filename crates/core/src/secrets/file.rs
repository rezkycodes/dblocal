//! `0600` file fallback used when the OS keychain is unavailable.
//!
//! `F-CRD-01` AC2 requires this path to work on a headless Linux box, so it must
//! not depend on D-Bus, libsecret or any privilege. The rules are:
//!
//! - the directory is created `0700`, and is refused if it is a symlink;
//! - every file is created `0600` before any byte is written, so it is never
//!   briefly world-readable;
//! - writes are atomic: a unique temporary file in the same directory is
//!   fsynced and then renamed over the destination, so a concurrent reader sees
//!   either the old value or the new one, never a torn one;
//! - a destination that is a symlink is refused instead of followed, so the
//!   file can never be redirected outside `~/.dblocal/secrets`.

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use zeroize::Zeroize;

use crate::DblError;
use crate::error::Result;
use crate::model::{CredentialBackend, CredentialRef};
use crate::paths::DblPaths;
use crate::secret::SecretString;

use super::backend::{BackendKind, SecretBackend, not_found, validate_key_ref, validate_key_str};

/// Monotonic suffix so two writers in the same process never pick the same
/// temporary name.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Credentials stored as one `0600` file per reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileBackend {
    dir: PathBuf,
}

impl FileBackend {
    /// Builds a backend rooted at `dir`, normally [`DblPaths::secrets_dir`].
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Builds the backend for the configured DBLocal home.
    pub fn from_paths(paths: &DblPaths) -> Self {
        Self::new(paths.secrets_dir())
    }

    /// The directory this backend owns.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Path of the file holding `reference`.
    fn path_for(&self, reference: &CredentialRef) -> Result<PathBuf> {
        validate_key_ref(reference)?;
        Ok(self.dir.join(file_name_for(reference.key_ref())?))
    }

    /// Creates the directory (and its parents) `0700`, refusing a symlink.
    ///
    /// An existing real directory is *tightened* to `0700` on Unix rather than
    /// left alone: if it was created earlier with the default umask (by an
    /// older build, or by hand), other users could list the reference names.
    fn ensure_dir(&self) -> Result<()> {
        match fs::symlink_metadata(&self.dir) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(DblError::Secret {
                        reason: "the secrets directory is a symlink".to_owned(),
                    });
                }
                if !meta.is_dir() {
                    return Err(DblError::Secret {
                        reason: "the secrets path exists but is not a directory".to_owned(),
                    });
                }
                #[cfg(unix)]
                tighten_dir(&self.dir)?;
                Ok(())
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {
                let mut builder = fs::DirBuilder::new();
                builder.recursive(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    builder.mode(0o700);
                }
                builder.create(&self.dir).map_err(DblError::from)?;

                // `recursive(true)` honours a pre-existing mode on some
                // platforms, so set it explicitly once the directory exists.
                #[cfg(unix)]
                tighten_dir(&self.dir)?;
                Ok(())
            }
            Err(err) => Err(DblError::from(err)),
        }
    }
}

/// Forces `0700` on a directory, ignoring a mode that is already correct.
#[cfg(unix)]
fn tighten_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let current = fs::metadata(dir).map_err(DblError::from)?.permissions();
    if current.mode() & 0o777 == 0o700 {
        return Ok(());
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(DblError::from)
}

impl SecretBackend for FileBackend {
    fn kind(&self) -> BackendKind {
        CredentialBackend::File
    }

    fn get(&self, reference: &CredentialRef) -> Result<SecretString> {
        let path = self.path_for(reference)?;
        refuse_symlink(&path)?;
        let mut file = open_readonly(&path).map_err(|err| match err.kind() {
            ErrorKind::NotFound => not_found(reference),
            _ => DblError::from(err),
        })?;

        let mut buffer = Vec::new();
        if let Err(err) = file.read_to_end(&mut buffer) {
            buffer.zeroize();
            return Err(DblError::from(err));
        }
        let text = match String::from_utf8(buffer) {
            Ok(value) => value,
            Err(err) => {
                let mut bytes = err.into_bytes();
                bytes.zeroize();
                return Err(DblError::Secret {
                    reason: "the stored credential is not valid UTF-8".to_owned(),
                });
            }
        };
        Ok(SecretString::new(text))
    }

    fn set(&self, reference: &CredentialRef, secret: &SecretString) -> Result<()> {
        validate_key_ref(reference)?;
        self.ensure_dir()?;
        let path = self.path_for(reference)?;
        refuse_symlink(&path)?;
        write_atomically(&path, secret.expose().as_bytes())
    }

    fn delete(&self, reference: &CredentialRef) -> Result<()> {
        let path = self.path_for(reference)?;
        refuse_symlink(&path)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == ErrorKind::NotFound => Err(not_found(reference)),
            Err(err) => Err(DblError::from(err)),
        }
    }
}

/// Prefix on every credential file name; see [`file_name_for`].
const KEY_FILE_PREFIX: &str = "cred-";

/// A Windows-safe, injective file name for a key reference.
///
/// The key may contain `:` (`dblocal:<instance-id>`), which NTFS would read as
/// an alternate data stream of a file named `dblocal`; a rename onto it then
/// fails, so the fallback would not work on Windows at all. The encoding is
/// therefore:
///
/// - every `:` becomes `%3A`. `%` is not an allowed key character (see
///   [`validate_key_str`]), so the escape can never appear literally and two
///   distinct keys can never map to the same name;
/// - the result is prefixed with `cred-`, which makes a key that looks like a
///   reserved device name (`CON`, `NUL`, `COM1`, ...) an ordinary file name;
/// - every other allowed character (`A-Za-z0-9`, `.`, `-`, `_`) is kept, so the
///   name stays readable and `get`/`set`/`delete` agree by construction.
pub(crate) fn file_name_for(key: &str) -> Result<String> {
    validate_key_str(key)?;
    let mut name = String::with_capacity(KEY_FILE_PREFIX.len() + key.len() + 8);
    name.push_str(KEY_FILE_PREFIX);
    for ch in key.chars() {
        if ch == ':' {
            name.push_str("%3A");
        } else {
            name.push(ch);
        }
    }
    Ok(name)
}

/// Opens an existing credential file without following a symlink.
fn open_readonly(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // O_NOFOLLOW turns a symlink into an error instead of a redirect.
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    options.open(path)
}

/// Rejects a destination that is already a symlink.
fn refuse_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(DblError::Secret {
            reason: "refusing to write through a symlinked credential file".to_owned(),
        }),
        Ok(meta) if !meta.is_file() => Err(DblError::Secret {
            reason: "the credential path exists but is not a regular file".to_owned(),
        }),
        Ok(_) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(DblError::from(err)),
    }
}

/// Writes `bytes` to a fresh `0600` file and renames it over `path`.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| DblError::Secret {
        reason: "the credential path has no parent directory".to_owned(),
    })?;
    let temp = temp_path(parent);

    let result = (|| -> Result<()> {
        let mut file = create_private(&temp)?;
        file.write_all(bytes).map_err(DblError::from)?;
        file.sync_all().map_err(DblError::from)?;
        fs::rename(&temp, path).map_err(DblError::from)?;
        sync_dir(parent);
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Creates a new file that is `0600` from the very first open call.
fn create_private(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        // Never climb over a symlink planted between the check and this call.
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let file = options.open(path).map_err(DblError::from)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // `mode` is filtered by the process umask; force the exact mode.
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(DblError::from)?;
    }
    Ok(file)
}

/// Best-effort directory fsync so the rename survives a crash.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(handle) = File::open(dir) {
        let _ = handle.sync_all();
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
}

/// A unique, hidden temporary name in the destination directory.
fn temp_path(dir: &Path) -> PathBuf {
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    dir.join(format!(".tmp-{pid}-{counter}"))
}

#[cfg(test)]
#[path = "file/tests.rs"]
mod tests;
