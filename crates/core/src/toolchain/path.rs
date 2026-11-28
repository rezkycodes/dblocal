//! PATH management and the `doctor` checks.
//!
//! DBLocal must put its shims first on PATH, but only with explicit consent, and
//! must be able to undo it exactly:
//!
//! * On Linux, `apply` appends a guarded block to every shell startup file that
//!   already exists (`~/.profile`, `~/.bashrc`, `~/.zshrc`, fish's `conf.d`). The
//!   block has clear begin/end markers, so applying twice is one block and
//!   `remove` restores the file byte for byte. A backup is written first. An
//!   unreadable or read-only file is reported, never clobbered.
//! * On Windows, `apply` edits the user `Path` (`HKCU\Environment`) through a
//!   small [`WindowsEnv`] trait, preserving the value's type and broadcasting the
//!   change. The registry part is `cfg(windows)`; the pure string surgery is shared
//!   and unit-tested on Linux with a fake.
//!
//! The doctor reports four things, each as a typed finding with a fix hint: the
//! shims directory missing from PATH, the shims directory not first, another
//! `php`/`node` earlier on PATH (naming it), and a shim pointing at a removed
//! install.

use std::path::{Path, PathBuf};

use crate::DblError;
use crate::platform::Os;
use crate::toolchain::consts;
use crate::toolchain::fsops::ToolchainPaths;
use crate::toolchain::shim::{ShimTool, shim_target};

// ---------------------------------------------------------------------------
// The managed block
// ---------------------------------------------------------------------------

/// Renders the POSIX managed block for `shims_dir`.
pub fn render_block(shims_dir: &Path) -> String {
    let quoted = sh_single_quote(&shims_dir.display().to_string());
    let mut out = String::new();
    out.push_str(consts::PATH_BLOCK_BEGIN);
    out.push('\n');
    out.push_str("# Managed by DBLocal. Remove with `dblocal toolchain path --remove`.\n");
    out.push_str(&format!(
        "case \":${{PATH}}:\" in\n  *:{quoted}:*) ;;\n  *) PATH={quoted}${{PATH:+:$PATH}}; export PATH ;;\nesac\n"
    ));
    out.push_str(consts::PATH_BLOCK_END);
    out.push('\n');
    out
}

fn sh_single_quote(value: &str) -> String {
    // Inside the case pattern the path is used unquoted for `*:` matching, so we
    // escape characters that matter to the shell pattern/assignment. Paths with
    // spaces are handled by the `PATH=<quoted>...` assignment using single quotes.
    let mut out = String::from("'");
    for ch in value.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Applies the managed block to `original`, idempotently.
///
/// The inserted unit is `"\n" + block` when `original` is non-empty (so the begin
/// marker starts on its own line), and just `block` when it is empty. Removal
/// strips exactly that unit, so [`remove_block`] restores the original bytes. If a
/// block is already present it is replaced in place, so a moved shims dir updates
/// without duplicating.
pub fn apply_block(original: &str, shims_dir: &Path) -> String {
    let block = render_block(shims_dir);
    match strip_existing_block(original) {
        Some((before, after)) => {
            let mut out = String::new();
            out.push_str(before);
            out.push_str(&unit_for(before, &block));
            out.push_str(after);
            out
        }
        None => {
            let mut out = String::from(original);
            out.push_str(&unit_for(original, &block));
            out
        }
    }
}

/// The text inserted after `preceding`: a leading newline only when what precedes
/// is non-empty (so the block is never glued to a previous line) and the block
/// itself.
fn unit_for(preceding: &str, block: &str) -> String {
    if preceding.is_empty() {
        block.to_owned()
    } else {
        format!("\n{block}")
    }
}

/// Removes the managed block, restoring the original bytes.
pub fn remove_block(original: &str) -> String {
    match strip_existing_block(original) {
        Some((before, after)) => {
            let mut out = String::from(before);
            out.push_str(after);
            out
        }
        None => original.to_owned(),
    }
}

/// Whether `text` already contains the managed block.
pub fn has_block(text: &str) -> bool {
    text.contains(consts::PATH_BLOCK_BEGIN) && text.contains(consts::PATH_BLOCK_END)
}

/// Splits `text` around the managed block, returning (before, after).
///
/// The leading newline `apply_block` inserts before the marker is part of the
/// removed unit, so `before` is the exact text that preceded the block.
fn strip_existing_block(text: &str) -> Option<(&str, &str)> {
    let begin = text.find(consts::PATH_BLOCK_BEGIN)?;
    let end_rel = text[begin..].find(consts::PATH_BLOCK_END)?;
    let mut end = begin + end_rel + consts::PATH_BLOCK_END.len();
    if text[end..].starts_with('\n') {
        end += 1;
    }
    // The unit starts at the newline apply inserted, which is exactly the byte
    // before the marker when the marker is not at the very start of the text.
    let mut start = begin;
    if start > 0 && text[..start].ends_with('\n') {
        start -= 1;
    }
    let before = &text[..start];
    let after = &text[end..];
    Some((before, after))
}

// ---------------------------------------------------------------------------
// State detection and the doctor
// ---------------------------------------------------------------------------

/// One problem the doctor can report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// The shims directory is not on PATH at all.
    ShimsDirMissing {
        /// The shims dir.
        dir: String,
    },
    /// The shims directory is on PATH but not first.
    ShimsDirNotFirst {
        /// The shims dir.
        dir: String,
        /// The entry that comes before it.
        before: String,
    },
    /// Another `php`/`node` resolves earlier than the shim.
    ShadowedBinary {
        /// The shim name.
        name: String,
        /// The earlier path that would win.
        earlier: String,
    },
    /// A shim points at an install directory that no longer exists.
    DanglingShim {
        /// The shim file.
        shim: String,
        /// The missing target.
        target: String,
    },
    /// The managed PATH block is present but the shims dir it names is stale.
    StaleBlock {
        /// The dir the block names.
        dir: String,
    },
}

impl Finding {
    /// A stable machine-readable code for `--json` output.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ShimsDirMissing { .. } => "shims_dir_missing",
            Self::ShimsDirNotFirst { .. } => "shims_dir_not_first",
            Self::ShadowedBinary { .. } => "shadowed_binary",
            Self::DanglingShim { .. } => "dangling_shim",
            Self::StaleBlock { .. } => "stale_block",
        }
    }

    /// A one-line, human-readable message with a fix hint.
    pub fn message(&self) -> String {
        match self {
            Self::ShimsDirMissing { dir } => format!(
                "the DBLocal shims directory {dir} is not on PATH; run `dblocal toolchain path --apply`"
            ),
            Self::ShimsDirNotFirst { dir, before } => format!(
                "the DBLocal shims directory {dir} is not first on PATH ({before} comes before it); \
                 put it first so the shims win"
            ),
            Self::ShadowedBinary { name, earlier } => format!(
                "another {name} at {earlier} is earlier on PATH and shadows the DBLocal shim; \
                 move the DBLocal shims directory ahead of it"
            ),
            Self::DanglingShim { shim, target } => format!(
                "the shim {shim} points at {target}, which no longer exists; \
                 run `dblocal toolchain remove` for that version or reinstall it"
            ),
            Self::StaleBlock { dir } => format!(
                "the managed PATH block names {dir}, which is not the current shims directory; \
                 run `dblocal toolchain path --apply` to refresh it"
            ),
        }
    }
}

/// The four doctor checks.
///
/// `path_var` is the PATH string to inspect, `shims_dir` is where the shims live,
/// `shims` lists the shim names to check for dangling targets, and `block_dir` is
/// the shims dir the managed block names (if any).
pub fn diagnose(
    path_var: &str,
    shims_dir: &Path,
    o: Os,
    shims: &[(String, PathBuf)],
    block_dir: Option<&Path>,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    let shims_str_dir = shims_dir.display().to_string();
    let separator = path_separator(o);
    let entries: Vec<&str> = path_var
        .split(separator)
        .filter(|e| !e.is_empty())
        .collect();

    let shims_str = normalize_path_str(shims_dir);
    let position = entries
        .iter()
        .position(|e| normalize_path_str(Path::new(e)) == shims_str);

    match position {
        None => findings.push(Finding::ShimsDirMissing {
            dir: shims_str_dir.clone(),
        }),
        Some(0) => {}
        Some(_) => findings.push(Finding::ShimsDirNotFirst {
            dir: shims_str_dir.clone(),
            before: entries[0].to_owned(),
        }),
    }

    // Shadowing: if any of our shim names resolves to a different file earlier on
    // PATH than the shims directory.
    let cutoff = position.unwrap_or(entries.len());
    for name in ["php", "node"] {
        for entry in &entries[..cutoff] {
            let candidate = Path::new(entry).join(executable_name(name, o));
            if candidate.is_file() {
                findings.push(Finding::ShadowedBinary {
                    name: name.to_owned(),
                    earlier: candidate.display().to_string(),
                });
                break;
            }
        }
    }

    // Dangling shims: a shim whose baked-in target no longer exists.
    for (_, shim_path) in shims {
        if let Some(target) = shim_target(shim_path)
            && !target.exists()
        {
            findings.push(Finding::DanglingShim {
                shim: shim_path.display().to_string(),
                target: target.display().to_string(),
            });
        }
    }

    if let Some(dir) = block_dir
        && normalize_path_str(dir) != shims_str
    {
        findings.push(Finding::StaleBlock {
            dir: dir.display().to_string(),
        });
    }

    findings
}

fn path_separator(o: Os) -> char {
    match o {
        Os::Windows => ';',
        _ => ':',
    }
}

fn normalize_path_str(path: &Path) -> String {
    let text = path.display().to_string();
    let trimmed = text.trim_end_matches(['/', '\\']);
    if o_is_windows_style(&text) {
        trimmed.replace('\\', "/").to_ascii_lowercase()
    } else {
        trimmed.to_owned()
    }
}

fn o_is_windows_style(text: &str) -> bool {
    // A drive letter or backslashes mean a Windows path regardless of host.
    text.contains('\\') || text.get(1..3) == Some(":\\")
}

fn executable_name(stem: &str, o: Os) -> String {
    match o {
        Os::Windows => format!("{stem}.exe"),
        _ => stem.to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Shell startup detection (Linux/macOS)
// ---------------------------------------------------------------------------

/// A shell startup file DBLocal may edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RcFile {
    /// The file path.
    pub path: PathBuf,
}

/// The POSIX startup files that exist under `home`, in a deterministic order.
///
/// Only files that exist are returned, so `apply` never creates a shell file the
/// user did not have. Fish is handled separately via [`fish_fragment`] using
/// fish syntax, never the POSIX block.
pub fn existing_rc_files(home: &Path) -> Vec<RcFile> {
    let candidates = [
        home.join(".profile"),
        home.join(".bashrc"),
        home.join(".zshrc"),
    ];
    candidates
        .into_iter()
        .filter(|path| path.is_file())
        .map(|path| RcFile { path })
        .collect()
}

/// The fish config directory under `home`.
pub fn fish_config_dir(home: &Path) -> PathBuf {
    home.join(".config/fish")
}

/// The fish fragment path (inside `conf.d`), whether or not it exists yet.
pub fn fish_fragment(home: &Path) -> PathBuf {
    home.join(".config/fish/conf.d/dblocal.fish")
}

/// Renders the fish-syntax managed fragment for `shims_dir`.
pub fn render_fish_block(shims_dir: &Path) -> String {
    let quoted = fish_single_quote(&shims_dir.display().to_string());
    let mut out = String::new();
    out.push_str(consts::PATH_BLOCK_BEGIN);
    out.push('\n');
    out.push_str("# Managed by DBLocal. Remove with `dblocal toolchain path --remove`.\n");
    out.push_str(&format!("fish_add_path --prepend --move {quoted}\n"));
    out.push_str(consts::PATH_BLOCK_END);
    out.push('\n');
    out
}

fn fish_single_quote(value: &str) -> String {
    let mut out = String::from("'");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            _ => out.push(ch),
        }
    }
    out.push('\'');
    out
}

/// The result of applying or removing the block across rc files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PathEditReport {
    /// Files changed.
    pub changed: Vec<PathBuf>,
    /// Files already in the requested state (no write needed).
    pub unchanged: Vec<PathBuf>,
    /// Files that could not be read or written, with the reason.
    pub failures: Vec<(PathBuf, String)>,
    /// Backup files written.
    pub backups: Vec<PathBuf>,
}

/// Applies the managed block to every existing rc file under `home`.
///
/// A read-only or unreadable file is reported in `failures` and left untouched. A
/// backup of each changed file is written first.
pub fn apply_posix(home: &Path, shims_dir: &Path) -> PathEditReport {
    let mut report = PathEditReport::default();
    for rc in existing_rc_files(home) {
        let original = match std::fs::read_to_string(&rc.path) {
            Ok(text) => text,
            Err(err) => {
                report.failures.push((rc.path, err.to_string()));
                continue;
            }
        };
        if has_block(&original) {
            // Already applied; refreshing a stale block is a no-op when the dir
            // matches. If it differs, rewrite it.
            let updated = apply_block(&original, shims_dir);
            if updated == original {
                report.unchanged.push(rc.path);
                continue;
            }
            match backup_and_write(&rc.path, &original, &updated) {
                Ok(backup) => {
                    report.changed.push(rc.path);
                    report.backups.push(backup);
                }
                Err(reason) => report.failures.push((rc.path, reason)),
            }
            continue;
        }
        let updated = apply_block(&original, shims_dir);
        match backup_and_write(&rc.path, &original, &updated) {
            Ok(backup) => {
                report.changed.push(rc.path);
                report.backups.push(backup);
            }
            Err(reason) => report.failures.push((rc.path, reason)),
        }
    }
    apply_fish(home, shims_dir, &mut report);
    report
}

fn apply_fish(home: &Path, shims_dir: &Path, report: &mut PathEditReport) {
    if !fish_config_dir(home).is_dir() {
        return;
    }
    let frag = fish_fragment(home);
    let rendered = render_fish_block(shims_dir);
    if frag.exists() {
        let original = match std::fs::read_to_string(&frag) {
            Ok(text) => text,
            Err(err) => {
                report.failures.push((frag, err.to_string()));
                return;
            }
        };
        if !has_block(&original) {
            report.failures.push((
                frag,
                "existing fish fragment is not managed by DBLocal".to_owned(),
            ));
            return;
        }
        if original == rendered {
            report.unchanged.push(frag);
            return;
        }
        match backup_and_write(&frag, &original, &rendered) {
            Ok(backup) => {
                report.changed.push(frag);
                report.backups.push(backup);
            }
            Err(reason) => report.failures.push((frag, reason)),
        }
        return;
    }
    if let Some(parent) = frag.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        report.failures.push((frag, err.to_string()));
        return;
    }
    match crate::fsutil::write_private_file_atomic(&frag, rendered.as_bytes()) {
        Ok(()) => report.changed.push(frag),
        Err(err) => report.failures.push((frag, err.to_string())),
    }
}

/// Removes the managed block from every existing rc file under `home`, and
/// deletes the managed fish fragment if it carries DBLocal's markers.
pub fn remove_posix(home: &Path, shims_dir: &Path) -> PathEditReport {
    let _ = shims_dir;
    let mut report = PathEditReport::default();
    for rc in existing_rc_files(home) {
        let original = match std::fs::read_to_string(&rc.path) {
            Ok(text) => text,
            Err(err) => {
                report.failures.push((rc.path, err.to_string()));
                continue;
            }
        };
        if !has_block(&original) {
            report.unchanged.push(rc.path);
            continue;
        }
        let updated = remove_block(&original);
        if updated == original {
            report.unchanged.push(rc.path);
            continue;
        }
        match backup_and_write(&rc.path, &original, &updated) {
            Ok(backup) => {
                report.changed.push(rc.path);
                report.backups.push(backup);
            }
            Err(reason) => report.failures.push((rc.path, reason)),
        }
    }
    remove_fish(home, &mut report);
    report
}

fn remove_fish(home: &Path, report: &mut PathEditReport) {
    let frag = fish_fragment(home);
    if !frag.is_file() {
        return;
    }
    let original = match std::fs::read_to_string(&frag) {
        Ok(text) => text,
        Err(err) => {
            report.failures.push((frag, err.to_string()));
            return;
        }
    };
    if !has_block(&original) {
        report.unchanged.push(frag);
        return;
    }
    match std::fs::remove_file(&frag) {
        Ok(()) => report.changed.push(frag),
        Err(err) => report.failures.push((frag, err.to_string())),
    }
}

fn backup_and_write(
    path: &Path,
    _original: &str,
    updated: &str,
) -> std::result::Result<PathBuf, String> {
    use std::io::Write as _;

    // If `path` is a symlink (e.g. managed by a dotfile manager), resolve it
    // and atomically replace the target so the symlink itself stays intact.
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let meta = std::fs::metadata(&target)
        .map_err(|err| format!("could not inspect the file: {err}"))?;
    let perms = meta.permissions();
    if perms.readonly() {
        return Err("file is read-only".to_owned());
    }

    let backup = backup_path(path);
    if let Err(err) = std::fs::copy(&target, &backup) {
        return Err(format!("could not write a backup: {err}"));
    }

    let parent = target
        .parent()
        .ok_or_else(|| "target file has no parent directory".to_owned())?;
    let leaf = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("rc");
    let nonce: u64 = rand::random();
    let tmp = parent.join(format!(".{leaf}.dblocal-tmp-{nonce:016x}"));

    let write_res = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(updated.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::set_permissions(&tmp, perms)?;
        std::fs::rename(&tmp, &target)?;
        Ok(())
    })();

    if let Err(err) = write_res {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("could not write the file atomically: {err}"));
    }
    Ok(backup)
}

fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(consts::RC_BACKUP_SUFFIX);
    PathBuf::from(name)
}

/// Reads the shims dir the managed block names in the first rc file (or fish
/// fragment) that has one.
pub fn block_shims_dir(home: &Path) -> Option<PathBuf> {
    for rc in existing_rc_files(home) {
        let Ok(text) = std::fs::read_to_string(&rc.path) else {
            continue;
        };
        if let Some(dir) = parse_block_dir(&text) {
            return Some(dir);
        }
    }
    let fish = fish_fragment(home);
    if let Ok(text) = std::fs::read_to_string(&fish)
        && let Some(dir) = parse_block_dir(&text)
    {
        return Some(dir);
    }
    None
}

/// Extracts the shims dir from a rendered POSIX or fish block.
fn parse_block_dir(text: &str) -> Option<PathBuf> {
    let start = text.find(consts::PATH_BLOCK_BEGIN)?;
    let end = text.find(consts::PATH_BLOCK_END)?;
    let block = &text[start..end];
    // POSIX form: `PATH='<dir>'${PATH:+:$PATH}`.
    if let Some(pos) = block.find("PATH=") {
        let rest = &block[pos + "PATH=".len()..];
        if let Some(dir) = parse_single_quoted(rest) {
            return Some(PathBuf::from(dir));
        }
    }
    // Fish form: `fish_add_path --prepend --move '<dir>'`.
    let fish_marker = "fish_add_path --prepend --move ";
    if let Some(pos) = block.find(fish_marker) {
        let rest = &block[pos + fish_marker.len()..];
        if let Some(dir) = parse_single_quoted(rest) {
            return Some(PathBuf::from(dir));
        }
    }
    None
}

fn parse_single_quoted(text: &str) -> Option<String> {
    let mut chars = text.char_indices();
    let (_, first) = chars.next()?;
    if first != '\'' {
        return None;
    }
    let mut out = String::new();
    let mut escaped = false;
    for (_, ch) in chars {
        if escaped {
            out.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '\'' {
            return Some(out);
        }
        out.push(ch);
    }
    None
}

// ---------------------------------------------------------------------------
// Windows registry (cfg-gated behind a trait)
// ---------------------------------------------------------------------------

/// The user environment store, so the Windows Path surgery is unit-testable.
///
/// On Windows the real implementation edits `HKCU\Environment` and broadcasts
/// `WM_SETTINGCHANGE`; the trait exists so the string logic above it can be tested
/// on Linux with a fake.
pub trait WindowsEnv {
    /// Reads the user `Path` value (`None` when unset).
    fn get_user_path(&self) -> crate::Result<Option<String>>;
    /// Writes the user `Path` value, preserving its type, and broadcasts.
    fn set_user_path(&self, value: &str) -> crate::Result<()>;
}

/// Whether `path_var` contains `shims_dir` as an entry.
pub fn path_contains(path_var: &str, shims_dir: &Path, o: Os) -> bool {
    let sep = path_separator(o);
    let target = normalize_path_str(shims_dir);
    path_var
        .split(sep)
        .any(|e| !e.is_empty() && normalize_path_str(Path::new(e)) == target)
}

/// Prepends `shims_dir` to a `;`-separated Windows `Path` string, idempotently.
pub fn prepend_windows_path(current: &str, shims_dir: &Path) -> String {
    let dir = shims_dir.display().to_string();
    let already = current
        .split(';')
        .any(|e| normalize_path_str(Path::new(e)) == normalize_path_str(shims_dir));
    if already {
        // Move it to the front if it is present but not first.
        let mut rest: Vec<&str> = current
            .split(';')
            .filter(|e| {
                !e.is_empty() && normalize_path_str(Path::new(e)) != normalize_path_str(shims_dir)
            })
            .collect();
        rest.insert(0, dir.as_str());
        return rest.join(";");
    }
    if current.trim().is_empty() {
        dir
    } else {
        format!("{dir};{current}")
    }
}

/// Removes `shims_dir` from a `;`-separated Windows `Path` string, exactly.
pub fn remove_from_windows_path(current: &str, shims_dir: &Path) -> String {
    let target = normalize_path_str(shims_dir);
    current
        .split(';')
        .filter(|e| normalize_path_str(Path::new(e)) != target)
        .collect::<Vec<_>>()
        .join(";")
}

/// Applies the Windows user `Path` update through `env`, idempotently.
pub fn apply_windows_with<E: WindowsEnv>(
    env: &E,
    shims_dir: &Path,
) -> crate::Result<PathEditReport> {
    let mut report = PathEditReport::default();
    let current = env.get_user_path()?.unwrap_or_default();
    let updated = prepend_windows_path(&current, shims_dir);
    let label = PathBuf::from(r"HKCU\Environment#Path");
    if updated == current {
        report.unchanged.push(label);
    } else {
        env.set_user_path(&updated)?;
        report.changed.push(label);
    }
    Ok(report)
}

/// Removes `shims_dir` from the Windows user `Path` through `env`, idempotently.
pub fn remove_windows_with<E: WindowsEnv>(
    env: &E,
    shims_dir: &Path,
) -> crate::Result<PathEditReport> {
    let mut report = PathEditReport::default();
    let current = env.get_user_path()?.unwrap_or_default();
    let updated = remove_from_windows_path(&current, shims_dir);
    let label = PathBuf::from(r"HKCU\Environment#Path");
    if updated == current {
        report.unchanged.push(label);
    } else {
        env.set_user_path(&updated)?;
        report.changed.push(label);
    }
    Ok(report)
}

#[cfg(windows)]
struct RegExeWindowsEnv;

#[cfg(windows)]
impl RegExeWindowsEnv {
    fn query_path_and_type(&self) -> crate::Result<(Option<String>, &'static str)> {
        let output = std::process::Command::new("reg")
            .args(["query", r"HKCU\Environment", "/v", "Path"])
            .output()
            .map_err(|source| DblError::Io { source })?;
        if !output.status.success() {
            return Ok((None, "REG_EXPAND_SZ"));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if let Some((_, rest)) = line.split_once("REG_EXPAND_SZ") {
                return Ok((Some(rest.trim().to_owned()), "REG_EXPAND_SZ"));
            }
            if let Some((_, rest)) = line.split_once("REG_SZ") {
                return Ok((Some(rest.trim().to_owned()), "REG_SZ"));
            }
        }
        Ok((None, "REG_EXPAND_SZ"))
    }
}

#[cfg(windows)]
impl WindowsEnv for RegExeWindowsEnv {
    fn get_user_path(&self) -> crate::Result<Option<String>> {
        let (val, _) = self.query_path_and_type()?;
        Ok(val)
    }

    fn set_user_path(&self, value: &str) -> crate::Result<()> {
        let (_, value_type) = self.query_path_and_type()?;
        let status = std::process::Command::new("reg")
            .args([
                "add",
                r"HKCU\Environment",
                "/v",
                "Path",
                "/t",
                value_type,
                "/d",
                value,
                "/f",
            ])
            .status()
            .map_err(|source| DblError::Io { source })?;
        if !status.success() {
            return Err(DblError::Io {
                source: std::io::Error::other("reg add HKCU\\Environment\\Path failed"),
            });
        }
        // Best-effort broadcast of WM_SETTINGCHANGE so new shells see the updated PATH.
        let _ = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "[Environment]::SetEnvironmentVariable('Path', $null, 'Process'); \
                 Add-Type -Namespace Win32 -Name Native -MemberDefinition \
                 '[DllImport(\"user32.dll\", SetLastError=true, CharSet=CharSet.Auto)] \
                  public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);'; \
                 $r = [UIntPtr]::Zero; \
                 [Win32.Native]::SendMessageTimeout([IntPtr]0xffff, 0x001A, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref]$r) | Out-Null",
            ])
            .status();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Top-level operations bound to a home
// ---------------------------------------------------------------------------

/// Applies PATH on the host platform, returning the report.
pub fn apply(home: &Path, shims_dir: &Path, o: Os) -> crate::Result<PathEditReport> {
    match o {
        Os::Windows => {
            #[cfg(windows)]
            {
                apply_windows_with(&RegExeWindowsEnv, shims_dir)
            }
            #[cfg(not(windows))]
            {
                let _ = (home, shims_dir);
                Err(DblError::Unsupported {
                    reason: "Windows registry PATH editing requires running on Windows".to_owned(),
                })
            }
        }
        _ => Ok(apply_posix(home, shims_dir)),
    }
}

/// Removes PATH on the host platform, returning the report.
pub fn remove(home: &Path, shims_dir: &Path, o: Os) -> crate::Result<PathEditReport> {
    match o {
        Os::Windows => {
            #[cfg(windows)]
            {
                remove_windows_with(&RegExeWindowsEnv, shims_dir)
            }
            #[cfg(not(windows))]
            {
                let _ = (home, shims_dir);
                Err(DblError::Unsupported {
                    reason: "Windows registry PATH editing requires running on Windows".to_owned(),
                })
            }
        }
        _ => Ok(remove_posix(home, shims_dir)),
    }
}

/// Builds the doctor findings for the current home and PATH.
pub fn doctor(paths: &ToolchainPaths, home: &Path, path_var: &str, o: Os) -> Vec<Finding> {
    let shims_dir = paths.shims_dir();
    // Collect the shims DBLocal would check for dangling targets.
    let mut shims: Vec<(String, PathBuf)> = Vec::new();
    for shim in ShimTool::all() {
        let path = paths.shim_path(shim.name(), o);
        if path.is_file() {
            shims.push((shim.name().to_owned(), path));
        }
    }
    let block_dir = block_shims_dir(home);
    diagnose(path_var, &shims_dir, o, &shims, block_dir.as_deref())
}

#[cfg(test)]
mod tests;
