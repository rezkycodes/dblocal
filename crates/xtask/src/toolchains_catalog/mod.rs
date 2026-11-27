//! `cargo xtask toolchains-catalog` command.
//!
//! Generates and checks `catalog/toolchains-v1.json` for PHP, Node.js, and
//! Composer.

pub mod composer;
pub mod node;
pub mod php_linux;
pub mod php_windows;
pub mod schema;
pub mod smoke;
pub mod transport;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::SystemTime;

use clap::Args;

use schema::{
    SkippedItem, ToolArtifact, ToolEntry, ToolchainsDocument, assemble, same_content,
    validate_document,
};
use smoke::{HostSmokeRunner, SmokeRunner};
use transport::{LiveHttpSource, MemorySource, ToolchainSource};

const DEFAULT_OUTPUT: &str = "catalog/toolchains-v1.json";

/// CLI flags for `cargo xtask toolchains-catalog`.
#[derive(Debug, Args)]
pub struct ToolchainsCatalogArgs {
    /// Path to write (or check against) the generated JSON catalog.
    #[arg(long, default_value = DEFAULT_OUTPUT)]
    pub output: PathBuf,
    /// Regenerate in memory and fail if `--output` is out of date (ignoring
    /// `generated_at`).
    #[arg(long)]
    pub check: bool,
    /// Read upstream index/checksum fixtures from a local directory instead of
    /// contacting the network (mainly for offline testing).
    #[arg(long)]
    pub offline_fixtures: Option<PathBuf>,
}

/// Output of one generator run.
#[derive(Debug)]
pub struct BuildReport {
    pub document: ToolchainsDocument,
    pub skipped: Vec<SkippedItem>,
    pub total_bytes_downloaded: u64,
}

/// Entry point dispatched from `xtask::main`.
pub fn run(args: ToolchainsCatalogArgs) -> ExitCode {
    let result = match &args.offline_fixtures {
        Some(dir) => match MemorySource::from_fixture_dir(dir) {
            Ok(src) => run_with(&args, &src, &HostSmokeRunner, SystemTime::now()),
            Err(err) => Err(err),
        },
        None => match LiveHttpSource::new() {
            Ok(src) => run_with(&args, &src, &HostSmokeRunner, SystemTime::now()),
            Err(err) => Err(err),
        },
    };

    match result {
        Ok(summary) => {
            print!("{summary}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("xtask toolchains-catalog: {err}");
            crate::EXIT_FAILED_CODE
        }
    }
}

/// Executes the build or `--check` flow with injectable transport, smoke runner,
/// and clock so every code path is testable offline.
pub fn run_with(
    args: &ToolchainsCatalogArgs,
    source: &dyn ToolchainSource,
    smoke: &dyn SmokeRunner,
    now: SystemTime,
) -> Result<String, String> {
    let existing = load_existing(&args.output);
    let existing_pinned = existing
        .as_ref()
        .map(extract_pinned_hashes)
        .unwrap_or_default();

    let report = generate(source, smoke, now, &existing_pinned)?;
    validate_document(&report.document)
        .map_err(|err| format!("generated catalog failed validation: {err}"))?;

    if args.check {
        let committed = existing.ok_or_else(|| {
            format!(
                "--check failed: {} does not exist or is not valid JSON",
                args.output.display()
            )
        })?;
        if !same_content(&committed, &report.document) {
            return Err(format!(
                "--check failed: {} is out of date with upstream (ignoring generated_at)",
                args.output.display()
            ));
        }
        let mut out = render_summary(&report);
        out.push_str(&format!("check: {} is up to date\n", args.output.display()));
        return Ok(out);
    }

    write_atomic(&args.output, &report.document)
        .map_err(|err| format!("failed to write {}: {err}", args.output.display()))?;
    Ok(render_summary(&report))
}

/// Runs all four upstream adapters, assembles the deterministic document, and
/// fails if any tool ends up with zero versions.
pub fn generate(
    source: &dyn ToolchainSource,
    smoke: &dyn SmokeRunner,
    now: SystemTime,
    existing_pinned: &BTreeMap<String, String>,
) -> Result<BuildReport, String> {
    let work_dir =
        tempfile::tempdir().map_err(|err| format!("failed to create generator temp dir: {err}"))?;

    let mut skipped = Vec::new();

    let php_linux = php_linux::collect(source, smoke, work_dir.path(), existing_pinned)?;
    skipped.extend(php_linux.skipped);

    let (php_win_versions, php_win_skips) = php_windows::collect(source);
    skipped.extend(php_win_skips);

    let mut php_versions = php_linux.versions;
    php_versions.extend(php_win_versions);

    let (node_versions, node_skips) = node::collect(source, smoke, work_dir.path());
    skipped.extend(node_skips);

    let (composer_versions, composer_skips) = composer::collect(
        source,
        smoke,
        work_dir.path(),
        php_linux.saved_php_binary.as_deref(),
    );
    skipped.extend(composer_skips);

    let tools = vec![
        ToolEntry {
            id: "php".to_owned(),
            versions: php_versions,
        },
        ToolEntry {
            id: "node".to_owned(),
            versions: node_versions,
        },
        ToolEntry {
            id: "composer".to_owned(),
            versions: composer_versions,
        },
    ];

    for tool in &tools {
        if tool.versions.is_empty() {
            let details = format_skipped(&skipped);
            return Err(format!(
                "tool {:?} ended up with zero versions\n{details}",
                tool.id
            ));
        }
    }

    let document = assemble(tools, utc_timestamp(now));
    Ok(BuildReport {
        document,
        skipped,
        total_bytes_downloaded: source.total_bytes_downloaded(),
    })
}

fn load_existing(path: &Path) -> Option<ToolchainsDocument> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Extracts `url -> sha256` for all `hash_origin == "pinned"` artifacts.
pub fn extract_pinned_hashes(doc: &ToolchainsDocument) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for tool in &doc.tools {
        for version in &tool.versions {
            for artifact in &version.artifacts {
                if artifact.hash_origin == "pinned" {
                    map.insert(artifact.url.clone(), artifact.sha256.clone());
                }
            }
        }
    }
    map
}

fn write_atomic(path: &Path, doc: &ToolchainsDocument) -> Result<(), std::io::Error> {
    let mut text =
        serde_json::to_string_pretty(doc).map_err(|err| std::io::Error::other(err.to_string()))?;
    if !text.ends_with('\n') {
        text.push('\n');
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, text.as_bytes())?;
    std::fs::rename(&tmp, path)
}

/// Renders a concise summary table, largest artifacts, total downloaded bytes,
/// and any skipped items.
pub fn render_summary(report: &BuildReport) -> String {
    let mut out = String::new();
    out.push_str("tool\tversions\tartifacts\n");
    let mut all_artifacts: Vec<(&str, &str, &ToolArtifact)> = Vec::new();
    for tool in &report.document.tools {
        let art_count: usize = tool.versions.iter().map(|v| v.artifacts.len()).sum();
        out.push_str(&format!(
            "{}\t{}\t{}\n",
            tool.id,
            tool.versions.len(),
            art_count
        ));
        for ver in &tool.versions {
            for art in &ver.artifacts {
                all_artifacts.push((&tool.id, &ver.version, art));
            }
        }
    }

    out.push_str(&format!(
        "total_bytes_downloaded: {} ({:.2} MiB)\n",
        report.total_bytes_downloaded,
        report.total_bytes_downloaded as f64 / (1024.0 * 1024.0)
    ));

    all_artifacts.sort_by_key(|b| std::cmp::Reverse(b.2.size_bytes));
    out.push_str("largest_artifacts:\n");
    for (tool, ver, art) in all_artifacts.into_iter().take(5) {
        let profile = art.profile.as_deref().unwrap_or("-");
        out.push_str(&format!(
            "  {tool} {ver} {}/{}/{} {:.2} MiB ({} bytes)\n",
            art.os,
            art.arch,
            profile,
            art.size_bytes as f64 / (1024.0 * 1024.0),
            art.size_bytes
        ));
    }

    out.push_str(&format_skipped(&report.skipped));
    out
}

fn format_skipped(skipped: &[SkippedItem]) -> String {
    if skipped.is_empty() {
        return "skipped: none\n".to_owned();
    }
    let mut out = format!("skipped ({}):\n", skipped.len());
    for item in skipped {
        out.push_str(&format!(
            "  [{}] {}: {}\n",
            item.tool, item.target, item.reason
        ));
    }
    out
}

/// Formats `time` as ISO-8601 UTC `YYYY-MM-DDTHH:MM:SSZ`.
pub fn utc_timestamp(time: SystemTime) -> String {
    let secs = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let hour = rem / 3_600;
    let minute = (rem % 3_600) / 60;
    let second = rem % 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

#[cfg(test)]
mod tests;
