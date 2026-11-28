//! PHP settings, validated and rendered into a DBLocal-managed ini fragment.
//!
//! The five settings the release manages are `memory_limit`,
//! `upload_max_filesize`, `post_max_size`, `max_execution_time`, `date.timezone`
//! and `display_errors`. They are stored in DBLocal's own `conf.d/99-dblocal.ini`,
//! which the `php` shim exposes through `PHP_INI_SCAN_DIR`, so they apply to
//! whatever PHP version is active without editing a per-version ini.
//!
//! Every value is validated before it is written: no value is copied into the ini
//! unescaped, and a hostile value (a newline, a quote, a `;`, an assignment) is
//! rejected rather than injected into an ini directive. On Windows a managed base
//! `php.ini` is also written with the default extension set, because the official
//! Windows build reads no ini otherwise.

use std::fmt;
use std::path::Path;
use std::str::FromStr;

use crate::DblError;
use crate::toolchain::consts;
use crate::toolchain::error::ToolchainError;
use crate::toolchain::fsops::ToolchainPaths;

/// The settings DBLocal manages, addressed by their ini key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SettingKey {
    /// `memory_limit`.
    MemoryLimit,
    /// `upload_max_filesize`.
    UploadMaxFilesize,
    /// `post_max_size`.
    PostMaxSize,
    /// `max_execution_time`.
    MaxExecutionTime,
    /// `date.timezone`.
    DateTimezone,
    /// `display_errors`.
    DisplayErrors,
}

impl SettingKey {
    /// Every key, in a stable order.
    pub const fn all() -> [SettingKey; 6] {
        [
            SettingKey::MemoryLimit,
            SettingKey::UploadMaxFilesize,
            SettingKey::PostMaxSize,
            SettingKey::MaxExecutionTime,
            SettingKey::DateTimezone,
            SettingKey::DisplayErrors,
        ]
    }

    /// The ini directive name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MemoryLimit => "memory_limit",
            Self::UploadMaxFilesize => "upload_max_filesize",
            Self::PostMaxSize => "post_max_size",
            Self::MaxExecutionTime => "max_execution_time",
            Self::DateTimezone => "date.timezone",
            Self::DisplayErrors => "display_errors",
        }
    }
}

impl fmt::Display for SettingKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SettingKey {
    type Err = DblError;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value.trim() {
            "memory_limit" => Ok(Self::MemoryLimit),
            "upload_max_filesize" => Ok(Self::UploadMaxFilesize),
            "post_max_size" => Ok(Self::PostMaxSize),
            "max_execution_time" => Ok(Self::MaxExecutionTime),
            "date.timezone" => Ok(Self::DateTimezone),
            "display_errors" => Ok(Self::DisplayErrors),
            other => Err(DblError::InvalidInput {
                field: "php_setting",
                reason: format!(
                    "unknown setting: {other} (expected one of memory_limit, upload_max_filesize, \
                     post_max_size, max_execution_time, date.timezone, display_errors)"
                ),
            }),
        }
    }
}

/// A validated value for one setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingValue {
    /// A size such as `512M` or `-1` (memory_limit).
    Size(SizeValue),
    /// A non-negative integer number of seconds.
    Seconds(u32),
    /// An IANA timezone name.
    Timezone(String),
    /// An on/off toggle.
    Toggle(bool),
}

impl SettingValue {
    /// Renders the value as PHP accepts it in an ini file.
    pub fn render(&self) -> String {
        match self {
            Self::Size(size) => size.render(),
            Self::Seconds(seconds) => seconds.to_string(),
            Self::Timezone(name) => name.clone(),
            Self::Toggle(on) => if *on { "On" } else { "Off" }.to_owned(),
        }
    }
}

/// A PHP size: a number optionally suffixed with `K`, `M` or `G`, or `-1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeValue {
    bytes: i64,
    raw: SuffixedSize,
}

/// Internal representation that keeps the exact suffix the user asked for, so
/// `1G` is not rewritten as `1024M`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SuffixedSize {
    Unlimited,
    Bytes { amount: u64, unit: u8 },
}

impl SizeValue {
    /// Builds a mebibyte size directly (used for infallible defaults).
    const fn from_mib(amount: u64) -> Self {
        Self {
            bytes: (amount as i64).saturating_mul(1024 * 1024),
            raw: SuffixedSize::Bytes { amount, unit: b'm' },
        }
    }

    /// Parses a PHP size string (`512M`, `64m`, `1G`, `-1`, `1024`).
    pub fn parse(value: &str) -> crate::Result<Self> {
        let trimmed = value.trim();
        if trimmed == "-1" {
            return Ok(Self {
                bytes: -1,
                raw: SuffixedSize::Unlimited,
            });
        }
        // Reject anything with an interior space, sign or quote early.
        if trimmed.is_empty()
            || !trimmed
                .chars()
                .all(|c| c.is_ascii_digit() || matches!(c, 'k' | 'K' | 'm' | 'M' | 'g' | 'G'))
        {
            return Err(DblError::InvalidInput {
                field: "size",
                reason: format!("'{trimmed}' is not a size such as 512M, 1G or -1"),
            });
        }
        let (digits, unit) = split_unit(trimmed);
        let amount: u64 = digits.parse().map_err(|_| DblError::InvalidInput {
            field: "size",
            reason: format!("'{trimmed}' is not a size"),
        })?;
        if amount == 0 {
            return Err(DblError::InvalidInput {
                field: "size",
                reason: "size must be greater than zero".to_owned(),
            });
        }
        let multiplier = match unit {
            b'k' => 1024,
            b'm' => 1024 * 1024,
            b'g' => 1024 * 1024 * 1024,
            _ => 1,
        };
        let bytes = (amount as i64).saturating_mul(multiplier);
        Ok(Self {
            bytes,
            raw: SuffixedSize::Bytes { amount, unit },
        })
    }

    /// The value in bytes (`-1` for unlimited).
    pub const fn bytes(&self) -> i64 {
        self.bytes
    }

    /// Renders with the suffix the user chose (upper-cased), or `-1`.
    pub fn render(&self) -> String {
        match self.raw {
            SuffixedSize::Unlimited => "-1".to_owned(),
            SuffixedSize::Bytes { amount, unit } => match unit {
                b'k' => format!("{amount}K"),
                b'm' => format!("{amount}M"),
                b'g' => format!("{amount}G"),
                _ => amount.to_string(),
            },
        }
    }
}

fn split_unit(value: &str) -> (&str, u8) {
    let mut chars = value.chars();
    let unit = value
        .chars()
        .last()
        .filter(|c| c.is_ascii_alphabetic())
        .map(|c| c.to_ascii_lowercase() as u8);
    match unit {
        Some(unit) if matches!(unit, b'k' | b'm' | b'g') => {
            let digits = &value[..value.len() - 1];
            (digits, unit)
        }
        _ => {
            let _ = chars.next();
            (value, 0u8)
        }
    }
}

/// The full set of managed settings, with defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhpSettings {
    memory_limit: SettingValue,
    upload_max_filesize: SettingValue,
    post_max_size: SettingValue,
    max_execution_time: SettingValue,
    date_timezone: SettingValue,
    display_errors: SettingValue,
}

impl Default for PhpSettings {
    fn default() -> Self {
        Self {
            memory_limit: SettingValue::Size(
                SizeValue::parse(consts::PHP_DEFAULT_MEMORY_LIMIT)
                    .unwrap_or_else(|_| SizeValue::from_mib(512)),
            ),
            upload_max_filesize: SettingValue::Size(
                SizeValue::parse(consts::PHP_DEFAULT_UPLOAD_MAX_FILESIZE)
                    .unwrap_or_else(|_| SizeValue::from_mib(64)),
            ),
            post_max_size: SettingValue::Size(
                SizeValue::parse(consts::PHP_DEFAULT_POST_MAX_SIZE)
                    .unwrap_or_else(|_| SizeValue::from_mib(64)),
            ),
            max_execution_time: SettingValue::Seconds(
                consts::PHP_DEFAULT_MAX_EXECUTION_TIME as u32,
            ),
            date_timezone: SettingValue::Timezone(consts::PHP_DEFAULT_DATE_TIMEZONE.to_owned()),
            display_errors: SettingValue::Toggle(consts::PHP_DEFAULT_DISPLAY_ERRORS),
        }
    }
}

impl PhpSettings {
    /// The value of one key.
    pub fn get(&self, key: SettingKey) -> &SettingValue {
        match key {
            SettingKey::MemoryLimit => &self.memory_limit,
            SettingKey::UploadMaxFilesize => &self.upload_max_filesize,
            SettingKey::PostMaxSize => &self.post_max_size,
            SettingKey::MaxExecutionTime => &self.max_execution_time,
            SettingKey::DateTimezone => &self.date_timezone,
            SettingKey::DisplayErrors => &self.display_errors,
        }
    }

    /// Returns a copy with one value replaced (after validation).
    pub fn with_value(&self, key: SettingKey, value: SettingValue) -> Self {
        let mut next = self.clone();
        match key {
            SettingKey::MemoryLimit => next.memory_limit = value,
            SettingKey::UploadMaxFilesize => next.upload_max_filesize = value,
            SettingKey::PostMaxSize => next.post_max_size = value,
            SettingKey::MaxExecutionTime => next.max_execution_time = value,
            SettingKey::DateTimezone => next.date_timezone = value,
            SettingKey::DisplayErrors => next.display_errors = value,
        }
        next
    }

    /// Validates a raw string for `key` and returns the typed value.
    pub fn parse_value(key: SettingKey, raw: &str) -> crate::Result<SettingValue> {
        match key {
            SettingKey::MemoryLimit | SettingKey::UploadMaxFilesize | SettingKey::PostMaxSize => {
                Ok(SettingValue::Size(SizeValue::parse(raw)?))
            }
            SettingKey::MaxExecutionTime => {
                let trimmed = raw.trim();
                if !trimmed.chars().all(|c| c.is_ascii_digit()) || trimmed.is_empty() {
                    return Err(invalid(key, "must be a whole number of seconds"));
                }
                let seconds: u32 = trimmed.parse().map_err(|_| invalid(key, "is too large"))?;
                Ok(SettingValue::Seconds(seconds))
            }
            SettingKey::DateTimezone => {
                // A valid IANA timezone name, checked against the local tzdb.
                let trimmed = raw.trim();
                if trimmed.is_empty() || contains_ini_metachar(trimmed) {
                    return Err(invalid(key, "is not a valid timezone name"));
                }
                jiff::tz::TimeZone::get(trimmed)
                    .map_err(|_| invalid(key, "is not a known timezone"))?;
                Ok(SettingValue::Timezone(trimmed.to_owned()))
            }
            SettingKey::DisplayErrors => match raw.trim().to_ascii_lowercase().as_str() {
                "1" | "on" | "true" | "yes" => Ok(SettingValue::Toggle(true)),
                "0" | "off" | "false" | "no" => Ok(SettingValue::Toggle(false)),
                _ => Err(invalid(key, "must be on or off")),
            },
        }
    }

    /// Parses `key=value` pairs into a settings update.
    pub fn parse_pairs(pairs: &[String]) -> crate::Result<Vec<(SettingKey, SettingValue)>> {
        let mut out = Vec::with_capacity(pairs.len());
        for pair in pairs {
            let (key, value) = pair.split_once('=').ok_or_else(|| DblError::InvalidInput {
                field: "setting",
                reason: format!("expected key=value, got '{pair}'"),
            })?;
            let key = SettingKey::from_str(key)?;
            out.push((key, Self::parse_value(key, value)?));
        }
        Ok(out)
    }

    /// Renders the managed ini fragment with every setting and a fixed header.
    pub fn render_ini(&self) -> String {
        let mut out = String::new();
        out.push_str("; DBLocal managed PHP settings. Do not edit by hand.\n");
        out.push_str("; Change them with `dblocal toolchain php-settings set <key> <value>`.\n");
        for key in SettingKey::all() {
            out.push_str(&format!("{} = {}\n", key.as_str(), self.get(key).render()));
        }
        out
    }

    /// Reads settings back from a rendered ini fragment.
    ///
    /// Unknown keys and the header comments are ignored, so reading a file that a
    /// newer DBLocal added to still works. A value that fails validation falls back
    /// to its default, so a hand-edited, broken file never prevents the settings
    /// from being displayed.
    pub fn parse_ini(text: &str) -> Self {
        let mut settings = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let Ok(key) = SettingKey::from_str(key) else {
                continue;
            };
            if let Ok(parsed) = Self::parse_value(key, value) {
                settings = settings.with_value(key, parsed);
            }
        }
        settings
    }
}

fn invalid(key: SettingKey, reason: &str) -> DblError {
    ToolchainError::InvalidSetting {
        key: key.as_str(),
        reason: reason.to_owned(),
    }
    .into_dbl_error()
}

/// Whether a value contains a character that could break out of an ini value.
fn contains_ini_metachar(value: &str) -> bool {
    value
        .chars()
        .any(|c| matches!(c, '\n' | '\r' | '"' | '\'' | ';' | '=' | '[' | ']' | '\\'))
}

/// Writes the managed `99-dblocal.ini` atomically.
pub fn write_managed_ini(
    paths: &ToolchainPaths,
    settings: &PhpSettings,
) -> crate::Result<std::path::PathBuf> {
    let path = paths.php_managed_ini();
    let bytes = settings.render_ini();
    crate::fsutil::write_private_file_atomic(&path, bytes.as_bytes())
        .map_err(|source| DblError::Io { source })?;
    Ok(path)
}

/// Writes the managed base `php.ini` for a Windows install (extension set).
///
/// The official Windows build reads no ini by default, so DBLocal writes one with
/// the documented default extensions enabled and `extension_dir` pointed at the
/// install's `ext` directory.
pub fn write_windows_base_ini(
    install_dir: &Path,
    extensions: &[String],
) -> crate::Result<std::path::PathBuf> {
    let path = install_dir.join(consts::PHP_BASE_INI_FILE);
    let ext_dir = install_dir.join("ext");
    let mut out = String::new();
    out.push_str("; DBLocal managed php.ini (Windows). Do not edit by hand.\n");
    out.push_str(&format!(
        "extension_dir = \"{}\"\n",
        ini_quote(&ext_dir.display().to_string())
    ));
    for ext in extensions {
        out.push_str(&format!("extension={ext}\n"));
    }
    out.push_str("date.timezone = UTC\n");
    crate::fsutil::write_private_file_atomic(&path, out.as_bytes())
        .map_err(|source| DblError::Io { source })?;
    Ok(path)
}

/// Quotes a path for an ini value, escaping the only metacharacters that matter.
fn ini_quote(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests;
