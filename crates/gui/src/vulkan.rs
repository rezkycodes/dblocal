//! Vulkan pre-flight.
//!
//! GPUI renders through Vulkan on Linux. Without a usable GPU driver/ICD the
//! process would panic or show a black window (W0-C evidence: `raw-panic` with
//! exit 101). This check fails early with actionable install instructions and a
//! dedicated exit code instead. It never panics.
//!
//! The check applies to Linux/BSD only. Windows renders through DirectX and
//! macOS through Metal; neither uses Vulkan ICD manifests, so the Linux
//! preflight must not run there.

use std::env;
use std::ffi::OsString;
use std::path::Path;

/// Exit code when Vulkan is unavailable (`DBLOCAL_E_VULKAN`).
pub const EXIT_VULKAN: u8 = 78;

/// Test-only escape hatch: set `DBLOCAL_SPIKE_SKIP_PREFLIGHT=1` to observe
/// GPUI's own behaviour. Never set in a release build.
const ENV_SKIP: &str = "DBLOCAL_SPIKE_SKIP_PREFLIGHT";

const DOCS_URL: &str = "docs/dev/linux-build-deps.md";

/// Returns `Ok(())` when Vulkan looks usable, or an actionable report when it
/// does not. On Windows and macOS the check is a no-op (DirectX / Metal).
pub fn preflight() -> Result<(), String> {
    preflight_for_os(env::consts::OS, env::var_os(ENV_SKIP).is_some())
}

/// OS-aware pre-flight, pure in its decision so every platform can be tested on
/// Linux. Only Linux/BSD use Vulkan ICD manifests.
pub fn preflight_for_os(os: &str, skip: bool) -> Result<(), String> {
    if !os_uses_vulkan_preflight(os) {
        return Ok(());
    }
    preflight_with_skip(skip)
}

/// Whether this OS needs the Linux Vulkan ICD/adapter pre-flight.
pub fn os_uses_vulkan_preflight(os: &str) -> bool {
    matches!(os, "linux" | "freebsd" | "dragonfly" | "netbsd" | "openbsd")
}

/// The pre-flight body with the escape hatch decided by the caller, so the
/// skip branch is testable without mutating the process environment (which
/// `unsafe_code = deny` forbids).
pub fn preflight_with_skip(skip: bool) -> Result<(), String> {
    if skip {
        tracing::warn!("Vulkan pre-flight skipped ({ENV_SKIP} set)");
        return Ok(());
    }
    match detect() {
        Some(reason) => Err(report(&reason)),
        None => Ok(()),
    }
}

fn detect() -> Option<String> {
    for var in ["VK_ICD_FILENAMES", "VK_DRIVER_FILES"] {
        if let Some(value) = env::var_os(var)
            && let Some(problem) = invalid_override(var, &value)
        {
            return Some(problem);
        }
    }
    if !icd_manifest_present() {
        return Some(
            "no Vulkan ICD manifest in /usr/share/vulkan/icd.d, /etc/vulkan/icd.d \
             or $XDG_DATA_HOME/vulkan/icd.d"
                .to_owned(),
        );
    }
    probe_vulkan_adapters()
}

fn invalid_override(var: &str, value: &OsString) -> Option<String> {
    let text = value.to_string_lossy();
    if text.trim().is_empty() {
        return Some(format!("{var} is set but empty"));
    }
    let separator = if cfg!(windows) { ';' } else { ':' };
    let mut saw_existing = false;
    let mut missing: Vec<String> = Vec::new();
    for entry in text.split(separator) {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        if Path::new(entry).is_file() {
            saw_existing = true;
        } else {
            missing.push(entry.to_owned());
        }
    }
    if saw_existing {
        None
    } else if missing.is_empty() {
        Some(format!("{var} does not name any ICD manifest"))
    } else {
        Some(format!(
            "{var} points to missing ICD manifest(s): {}",
            missing.join(", ")
        ))
    }
}

fn icd_manifest_present() -> bool {
    let mut dirs: Vec<std::path::PathBuf> = vec![
        "/usr/share/vulkan/icd.d".into(),
        "/etc/vulkan/icd.d".into(),
        "/usr/local/share/vulkan/icd.d".into(),
    ];
    if let Some(data_home) = env::var_os("XDG_DATA_HOME") {
        dirs.push(std::path::PathBuf::from(data_home).join("vulkan/icd.d"));
    } else if let Some(home) = env::var_os("HOME") {
        dirs.push(std::path::PathBuf::from(home).join(".local/share/vulkan/icd.d"));
    }
    dirs.iter().any(|dir| {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries.flatten().any(|entry| {
                    entry
                        .path()
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
                })
            })
            .unwrap_or(false)
    })
}

/// Enumerates wgpu Vulkan adapters; returns a reason when none is usable.
fn probe_vulkan_adapters() -> Option<String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut future = std::pin::pin!(instance.enumerate_adapters(wgpu::Backends::VULKAN));
    let waker = std::task::Waker::noop();
    let mut context = std::task::Context::from_waker(waker);
    let adapters = match std::future::Future::poll(future.as_mut(), &mut context) {
        std::task::Poll::Ready(adapters) => adapters,
        std::task::Poll::Pending => {
            return Some("wgpu adapter enumeration did not complete synchronously".to_owned());
        }
    };
    if adapters.is_empty() {
        return Some(
            "wgpu enumerated zero Vulkan adapters, so no GPU driver is loaded for this session"
                .to_owned(),
        );
    }
    None
}

fn report(reason: &str) -> String {
    use dblocal_core::strings as s;
    let mut out = String::new();
    out.push_str(s::VULKAN_HEADLINE);
    out.push_str("\n\n");
    out.push_str(&format!("{}: {reason}\n\n", s::VULKAN_DETECTED));
    out.push_str(s::VULKAN_WHY);
    out.push_str("\n\n");
    out.push_str(s::VULKAN_FIX_HEADING);
    out.push('\n');
    out.push_str("  Fedora:  sudo dnf install mesa-vulkan-drivers vulkan-loader\n");
    out.push_str("  Ubuntu:  sudo apt install mesa-vulkan-drivers vulkan-tools\n");
    out.push_str("  Arch:    sudo pacman -S vulkan-icd-loader mesa-vulkan-drivers\n\n");
    out.push_str(&format!(
        "{}:\n  vulkaninfo --summary\n\n",
        s::VULKAN_VERIFY
    ));
    out.push_str(&format!("{}: {DOCS_URL}\n\n", s::VULKAN_MORE_HELP));
    out.push_str(s::VULKAN_HINT);
    out.push_str("\n\n");
    out.push_str(&format!(
        "{}: {EXIT_VULKAN} (DBLOCAL_E_VULKAN).\n",
        s::VULKAN_EXIT_CODE
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_missing_override_file() {
        let value = OsString::from("/nonexistent/icd.json");
        let reason = invalid_override("VK_ICD_FILENAMES", &value).expect("reason");
        assert!(reason.contains("VK_ICD_FILENAMES"));
        assert!(reason.contains("/nonexistent/icd.json"));
    }

    #[test]
    fn detects_empty_override() {
        let reason = invalid_override("VK_DRIVER_FILES", &OsString::from("")).expect("reason");
        assert!(reason.contains("empty"));
    }

    #[test]
    fn accepts_an_existing_override_file() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(
            invalid_override("VK_ICD_FILENAMES", &OsString::from(manifest.as_os_str())).is_none()
        );
    }

    #[test]
    fn manifest_presence_is_stable_within_a_run() {
        let first = icd_manifest_present();
        let second = icd_manifest_present();
        assert_eq!(first, second);
    }

    #[test]
    fn report_is_actionable_and_names_each_distro() {
        let text = report("no Vulkan ICD manifest found");
        assert!(text.contains("Terdeteksi: no Vulkan ICD manifest found"));
        for needle in ["Fedora", "Ubuntu", "Arch", "vulkaninfo", "vulkan-loader"] {
            assert!(text.contains(needle), "missing {needle}");
        }
        assert!(text.contains(DOCS_URL));
        assert!(text.ends_with(&format!("Kode keluar: {EXIT_VULKAN} (DBLOCAL_E_VULKAN).\n")));
    }

    #[test]
    fn multiple_override_entries_accept_any_existing_manifest() {
        let existing = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let value = OsString::from(format!("/nonexistent/a.json:{}", existing.display()));
        assert!(invalid_override("VK_ICD_FILENAMES", &value).is_none());
    }

    #[test]
    fn override_with_only_missing_entries_lists_them_all() {
        let value = OsString::from("/missing/one.json:/missing/two.json");
        let reason = invalid_override("VK_DRIVER_FILES", &value).expect("reason");
        assert!(reason.contains("/missing/one.json"));
        assert!(reason.contains("/missing/two.json"));
    }

    #[test]
    fn override_with_no_entries_is_reported_as_empty() {
        // Just separators: nothing to resolve either way.
        let reason = invalid_override("VK_ICD_FILENAMES", &OsString::from("::")).expect("reason");
        assert!(reason.contains("does not name any ICD manifest"));
    }

    #[test]
    fn manifest_probe_does_not_panic_on_a_missing_directory() {
        // On this host the directories exist; on a bare container they may not.
        // Either answer is fine, but the call must return rather than panic.
        assert!(matches!(icd_manifest_present(), true | false));
    }

    #[test]
    fn full_preflight_matches_the_skip_branch_when_skipped() {
        assert_eq!(preflight_with_skip(true), Ok(()));
    }

    #[test]
    fn adapter_probe_returns_cleanly_either_way() {
        // This host has a working RADV Vulkan driver, a bare CI container may
        // not. Both outcomes are acceptable; the call must not panic or hang.
        let probe = probe_vulkan_adapters();
        if let Some(reason) = probe {
            assert!(!reason.is_empty());
        }
    }

    #[test]
    fn real_preflight_runs_end_to_end() {
        // No override and no skip: exercises detect, the manifest probe and the
        // adapter enumeration. On this host Vulkan is present, so it passes.
        let result = preflight_with_skip(false);
        match result {
            Ok(()) => {}
            Err(report) => assert!(report.contains("DBLocal tidak bisa dijalankan")),
        }
    }

    #[test]
    fn skip_flag_disables_the_check() {
        assert!(preflight_with_skip(true).is_ok());
    }

    #[test]
    fn preflight_is_os_aware() {
        assert!(os_uses_vulkan_preflight("linux"));
        assert!(os_uses_vulkan_preflight("freebsd"));
        assert!(!os_uses_vulkan_preflight("macos"));
        assert!(!os_uses_vulkan_preflight("windows"));

        assert_eq!(preflight_for_os("macos", false), Ok(()));
        assert_eq!(preflight_for_os("windows", false), Ok(()));
        // Linux/BSD take the Linux path: skip makes the decision deterministic.
        assert_eq!(preflight_for_os("linux", true), Ok(()));
    }
}
