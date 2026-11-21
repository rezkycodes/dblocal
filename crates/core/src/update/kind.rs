//! Detect how DBLocal was installed, which determines whether self-update is
//! allowed and what apply strategy to use.

use std::path::{Path, PathBuf};

use crate::platform::Os;

use super::error::{UpdateError, UpdateResult};

/// How the running binary was installed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum InstallKind {
    /// Linux AppImage (`$APPIMAGE` points at the mounted file).
    AppImage { appimage_path: PathBuf },
    /// Portable tarball or generic binary on Linux.
    PortableTarball { exe_path: PathBuf },
    /// Windows zip install; binary can be swapped via helper.
    WindowsZip { exe_path: PathBuf },
    /// Windows MSI install; managed by Windows Installer.
    WindowsMsi { exe_path: PathBuf },
    /// macOS .app bundle.
    MacApp { bundle_path: PathBuf },
    /// Installed by a distro package manager.
    DistPackage {
        exe_path: PathBuf,
        hint: PackageManagerHint,
    },
    /// Installed by Homebrew.
    Homebrew { exe_path: PathBuf },
    /// Could not determine; self-update refused.
    Unknown { exe_path: PathBuf, reason: String },
}

impl InstallKind {
    /// Returns true when this install can be self-updated by DBLocal.
    pub fn can_self_update(&self) -> bool {
        matches!(
            self,
            Self::AppImage { .. }
                | Self::PortableTarball { .. }
                | Self::WindowsZip { .. }
                | Self::MacApp { .. }
        )
    }

    /// Returns the package-manager hint if self-update is disabled by manager.
    pub fn package_manager_hint(&self) -> Option<&PackageManagerHint> {
        match self {
            Self::DistPackage { hint, .. } => Some(hint),
            Self::Homebrew { .. } => Some(&PackageManagerHint::Homebrew),
            _ => None,
        }
    }

    /// Returns the executable or bundle path relevant to this kind.
    pub fn path(&self) -> &Path {
        match self {
            Self::AppImage { appimage_path } => appimage_path,
            Self::PortableTarball { exe_path }
            | Self::WindowsZip { exe_path }
            | Self::WindowsMsi { exe_path }
            | Self::DistPackage { exe_path, .. }
            | Self::Homebrew { exe_path }
            | Self::Unknown { exe_path, .. } => exe_path,
            Self::MacApp { bundle_path } => bundle_path,
        }
    }

    /// Human-readable refusal reason for unsupported kinds.
    pub fn refusal_reason(&self) -> Option<String> {
        match self {
            Self::DistPackage { hint, .. } => Some(hint.message()),
            Self::Homebrew { .. } => Some(PackageManagerHint::Homebrew.message()),
            Self::Unknown { reason, .. } => Some(format!(
                "Jenis pemasangan tidak dikenali; pembaruan otomatis ditolak demi keamanan. {reason}"
            )),
            _ => None,
        }
    }
}

/// User-facing hint for package-manager-managed installs.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PackageManagerHint {
    /// Debian/Ubuntu apt/dpkg.
    Dpkg,
    /// Fedora/RHEL rpm.
    Rpm,
    /// Homebrew.
    Homebrew,
    /// Generic package manager.
    Other { manager: String },
}

impl PackageManagerHint {
    /// Indonesian hint telling the user where to update.
    pub fn message(&self) -> String {
        match self {
            Self::Dpkg => {
                "Pembaruan dikelola oleh package manager sistem (dpkg/apt). Perbarui DBLocal lewat package manager Anda.".to_owned()
            }
            Self::Rpm => {
                "Pembaruan dikelola oleh package manager sistem (rpm/dnf/yum). Perbarui DBLocal lewat package manager Anda.".to_owned()
            }
            Self::Homebrew => {
                "Pembaruan dikelola oleh Homebrew. Jalankan pembaruan lewat Homebrew.".to_owned()
            }
            Self::Other { manager } => format!(
                "Pembaruan dikelola oleh package manager sistem ({manager}). Perbarui DBLocal lewat package manager Anda."
            ),
        }
    }
}

/// Abstraction over package-database lookups for testability.
pub trait PackageOwnership {
    /// Returns the package manager owning `path`, if any.
    fn owner_of(&self, path: &Path) -> Option<PackageManagerHint>;
}

/// Production ownership probe: checks env and platform package databases.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemPackageOwnership;

impl PackageOwnership for SystemPackageOwnership {
    fn owner_of(&self, path: &Path) -> Option<PackageManagerHint> {
        detect_package_manager(path)
    }
}

/// Detect install kind for the current process.
pub fn detect_install_kind() -> UpdateResult<InstallKind> {
    let ownership = SystemPackageOwnership;
    detect_install_kind_with(current_exe_path()?, &ownership)
}

/// Detect install kind using an injected executable path and ownership probe.
pub fn detect_install_kind_with(
    exe_path: PathBuf,
    ownership: &dyn PackageOwnership,
) -> UpdateResult<InstallKind> {
    if let Some(kind) = detect_from_env(&exe_path) {
        return Ok(kind);
    }
    if let Some(hint) = ownership.owner_of(&exe_path) {
        return Ok(InstallKind::DistPackage { exe_path, hint });
    }
    Ok(detect_from_path(&exe_path))
}

fn current_exe_path() -> UpdateResult<PathBuf> {
    std::env::current_exe().map_err(|source| UpdateError::ApplyFailed {
        reason: format!("gagal menentukan path executable saat ini: {source}"),
    })
}

fn detect_from_env(exe_path: &Path) -> Option<InstallKind> {
    detect_from_env_with(exe_path, |k| std::env::var(k).ok())
}

fn detect_from_env_with(
    exe_path: &Path,
    lookup: impl Fn(&str) -> Option<String>,
) -> Option<InstallKind> {
    if let Some(appimage) = lookup("APPIMAGE") {
        let path = PathBuf::from(appimage);
        if !path.as_os_str().is_empty()
            && lookup("APPDIR").is_some()
            && exe_path_inside_appdir(exe_path, lookup("APPDIR").as_deref())
            && path.is_file()
        {
            return Some(InstallKind::AppImage { appimage_path: path });
        }
    }
    if is_homebrew_path(exe_path) {
        return Some(InstallKind::Homebrew {
            exe_path: exe_path.to_path_buf(),
        });
    }
    None
}

fn exe_path_inside_appdir(exe_path: &Path, appdir: Option<&str>) -> bool {
    let Some(appdir) = appdir else {
        return false;
    };
    let appdir = PathBuf::from(appdir);
    if appdir.as_os_str().is_empty() {
        return false;
    }
    let exe = canonical_or_original(exe_path);
    let dir = canonical_or_original(&appdir);
    exe.starts_with(&dir)
}

fn detect_from_path(exe_path: &Path) -> InstallKind {
    match Os::current() {
        Os::Windows => detect_windows(exe_path),
        Os::Macos => detect_macos(exe_path),
        Os::Linux => detect_linux(exe_path),
    }
}

fn detect_linux(exe_path: &Path) -> InstallKind {
    if let Some(reason) = linux_deny_reason(exe_path) {
        return InstallKind::Unknown {
            exe_path: exe_path.to_path_buf(),
            reason,
        };
    }
    if !looks_like_portable_linux(exe_path) {
        return InstallKind::Unknown {
            exe_path: exe_path.to_path_buf(),
            reason: "lokasi executable Linux tidak dikenali sebagai instalasi portabel".to_owned(),
        };
    }
    InstallKind::PortableTarball {
        exe_path: exe_path.to_path_buf(),
    }
}

/// Linux self-update deny-list: paths where in-place binary replacement is not
/// safe or not the right delivery mechanism. Anything not recognised becomes
/// Unknown.
fn linux_deny_reason(exe_path: &Path) -> Option<String> {
    let lower = path_to_lower(exe_path);
    let denied_prefixes = ["/nix/store", "/snap/", "/app/", "/usr/", "/opt/"];
    for prefix in denied_prefixes {
        if lower.starts_with(prefix) || lower.contains(prefix) {
            return Some(format!("path berada di lokasi terkelola: {prefix}"));
        }
    }
    let denied_suffixes = ["/.cargo/bin/", "/target/debug/", "/target/release/"];
    for suffix in denied_suffixes {
        if lower.contains(suffix) {
            return Some(format!("path berada di lokasi build/tooling: {suffix}"));
        }
    }
    if !path_is_writable_by_current_user(exe_path) {
        return Some("direktori executable tidak dapat ditulis oleh pengguna saat ini".to_owned());
    }
    None
}

fn looks_like_portable_linux(exe_path: &Path) -> bool {
    let Some(name) = exe_path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if name != "dblocal" {
        return false;
    }
    exe_path.is_file()
}

fn path_is_writable_by_current_user(exe_path: &Path) -> bool {
    let Some(dir) = exe_path.parent() else {
        return false;
    };
    let meta = match std::fs::metadata(dir) {
        Ok(m) => m,
        Err(_) => return false,
    };
    if !meta.is_dir() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = meta.permissions().mode();
        let probe = dir.join(format!(".dblocal-update-probe-{}", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)
        {
            Ok(_) => {
                let _ = std::fs::remove_file(&probe);
                true
            }
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        !meta.permissions().readonly()
    }
}

fn canonical_or_original(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn detect_windows(exe_path: &Path) -> InstallKind {
    let lower = path_to_lower(exe_path);
    if lower.contains("\\program files")
        || lower.contains("\\program files (x86)")
        || lower.contains("\\appdata\\local\\dblocal")
    {
        return InstallKind::WindowsMsi {
            exe_path: exe_path.to_path_buf(),
        };
    }
    InstallKind::WindowsZip {
        exe_path: exe_path.to_path_buf(),
    }
}

fn detect_macos(exe_path: &Path) -> InstallKind {
    if let Some(bundle) = mac_app_bundle_path(exe_path) {
        return InstallKind::MacApp {
            bundle_path: bundle,
        };
    }
    InstallKind::Unknown {
        exe_path: exe_path.to_path_buf(),
        reason: "path executable macOS tidak berada di dalam bundle .app yang dikenali".to_owned(),
    }
}

fn mac_app_bundle_path(exe_path: &Path) -> Option<PathBuf> {
    let mut current = exe_path;
    while let Some(parent) = current.parent() {
        if parent.extension().and_then(|e| e.to_str()) == Some("app") {
            return Some(parent.to_path_buf());
        }
        current = parent;
    }
    None
}

fn is_homebrew_path(path: &Path) -> bool {
    let lower = path_to_lower(path);
    lower.contains("/homebrew/")
        || lower.contains("/cellar/")
        || lower.contains("/opt/homebrew/")
        || lower.contains("/linuxbrew/")
}

fn path_to_lower(path: &Path) -> String {
    path.to_string_lossy().to_ascii_lowercase()
}

fn detect_package_manager(path: &Path) -> Option<PackageManagerHint> {
    match Os::current() {
        Os::Linux => detect_linux_package_manager(path),
        Os::Macos => None,
        Os::Windows => None,
    }
}

fn detect_linux_package_manager(path: &Path) -> Option<PackageManagerHint> {
    if command_reports_ownership("dpkg-query", &["-S", &path.to_string_lossy()]) {
        return Some(PackageManagerHint::Dpkg);
    }
    if command_reports_ownership("rpm", &["-qf", &path.to_string_lossy()]) {
        return Some(PackageManagerHint::Rpm);
    }
    None
}

fn command_reports_ownership(cmd: &str, args: &[&str]) -> bool {
    let Ok(output) = std::process::Command::new(cmd).args(args).output() else {
        return false;
    };
    output.status.success()
}

#[cfg(test)]
#[path = "kind/tests.rs"]
mod tests;
