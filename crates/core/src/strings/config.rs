//! Config tab strings (`F-CFG-01`, `F-CFG-02`), live over `DblService`.
//!
//! Re-exported from `strings` so call sites stay `strings::UI_CONFIG_...`. The
//! service's own messages live in `crate::config::messages`; these are only the
//! chrome the view draws. Names avoid the placeholder strings still in
//! `strings/mod.rs` (`UI_CONFIG_HEADING`, `UI_CONFIG_COLUMNS`, `UI_CONFIG_SAVE`).

// --- Card and columns -------------------------------------------------------

/// Config card heading (PRD F-CFG-01 AC1: one card "Settings").
pub const UI_CONFIG_CARD_TITLE: &str = "Settings";
/// The settings are being read.
pub const UI_CONFIG_LOADING: &str = "Reading the configuration…";
/// No editable or read-only settings were returned.
pub const UI_CONFIG_EMPTY: &str = "No settings yet for this instance.";
/// A read of the config failed; the card offers a retry.
pub const UI_CONFIG_RETRY: &str = "Reload";

// --- One row ----------------------------------------------------------------

/// Default value shown next to the field (`Default: 100`).
pub const UI_CONFIG_DEFAULT_PREFIX: &str = "Default: ";
/// Word shown for a key the file does not set, so the default is in effect.
pub const UI_CONFIG_UNSET_NOTE: &str = "default";
/// Marker that an applied value needs a restart (text, never colour alone).
pub const UI_CONFIG_RESTART_MARKER: &str = "restart required";
/// Unit hint shown after the field when the view carries one.
pub const UI_CONFIG_UNIT_PREFIX: &str = "unit: ";
/// What a byte-size field accepts: the value is written in MB/GB, not bytes.
pub const UI_CONFIG_UNIT_BYTES: &str = "bytes (KB, MB, GB allowed)";
/// What a duration field accepts: seconds.
pub const UI_CONFIG_UNIT_SECONDS: &str = "seconds";
/// Why `port` is read-only here (it goes through the Change port action).
pub const UI_CONFIG_READONLY_PORT: &str = "Change the port with the Change port action";
/// Why the PostgreSQL authentication method is read-only here.
pub const UI_CONFIG_READONLY_MANAGED: &str = "Managed by DBLocal, cannot be changed here";

// --- Actions and results ----------------------------------------------------

/// Apply the edited settings ("Apply").
pub const UI_CONFIG_APPLY: &str = "Apply";
/// Discard the local edits and go back to the file's values.
pub const UI_CONFIG_CANCEL_CHANGES: &str = "Discard changes";
/// Shown after a successful apply (F-CFG-01 AC2).
pub const UI_CONFIG_APPLIED: &str = "Saved. Takes effect after a restart.";
/// The banner that a config file changed on disk while the tab is open.
pub const UI_CONFIG_CHANGED_OUTSIDE: &str = "File changed outside the app";
/// Copy the raw path to the clipboard.
pub const UI_CONFIG_COPY_PATH: &str = "Copy";
/// Toast after copying the path.
pub const UI_CONFIG_PATH_COPIED: &str = "Config path copied to the clipboard";
/// A restore done in the background.
pub const UI_CONFIG_RESTORED: &str = "Configuration restored from the newest backup.";

// --- Restore confirmation ---------------------------------------------------

/// Restore dialog title (F-CFG-01 AC3).
pub const UI_CONFIG_RESTORE_TITLE: &str = "Restore the previous configuration";
/// Restore action on the card.
pub const UI_CONFIG_RESTORE: &str = "Restore the previous configuration";
/// Restore dialog body; `{time}` is the newest backup's time.
pub const UI_CONFIG_RESTORE_PROMPT: &str = "The current file is saved as the newest backup, then the configuration from {time} is restored. A restart is required for it to take effect.";
/// Restore dialog confirm action.
pub const UI_CONFIG_RESTORE_CONFIRM: &str = "Restore";
/// The one-click failure-banner action (F-CFG-01 AC3).
pub const UI_CONFIG_RESTORE_AND_RESTART: &str = "Restore the configuration and restart";
/// The failure banner's short body: why a restore is offered.
pub const UI_CONFIG_RESTORE_BANNER_BODY: &str = "The configuration file changed since the last successful start. Restore the copy from before the change, then start the instance again.";
/// The backup time, as an age: `{n} seconds ago`.
pub const UI_CONFIG_AGE_SECONDS: &str = "{n} seconds ago";
/// The backup time, as an age: `{n} minutes ago`.
pub const UI_CONFIG_AGE_MINUTES: &str = "{n} minutes ago";
/// The backup time, as an age: `{n} hours ago`.
pub const UI_CONFIG_AGE_HOURS: &str = "{n} hours ago";
/// The backup time, as an age: `{n} days ago`.
pub const UI_CONFIG_AGE_DAYS: &str = "{n} days ago";
/// The backup time when the clock is behind the backup's stamp.
pub const UI_CONFIG_AGE_NOW: &str = "just now";

/// Formats the default-value note next to a field.
pub fn config_default(default: &str) -> String {
    if default.is_empty() {
        UI_CONFIG_DEFAULT_PREFIX.trim_end().to_owned()
    } else {
        format!("{UI_CONFIG_DEFAULT_PREFIX}{default}")
    }
}

/// Formats the unit note when the view carries a unit.
///
/// The core `unit` is the base unit the service stores (`bytes`, `seconds`), but
/// the field accepts `16MB` or `60`, so the note says what may be typed instead
/// of echoing a base unit the value visibly is not (`F9`). An unknown unit is
/// shown verbatim rather than dropped.
pub fn config_unit(unit: &str) -> Option<String> {
    if unit.is_empty() {
        return None;
    }
    let text = match unit {
        "bytes" => UI_CONFIG_UNIT_BYTES,
        "seconds" => UI_CONFIG_UNIT_SECONDS,
        other => other,
    };
    Some(format!("{UI_CONFIG_UNIT_PREFIX}{text}"))
}

/// Formats the restore prompt with the backup time.
pub fn config_restore_prompt(time: &str) -> String {
    UI_CONFIG_RESTORE_PROMPT.replace("{time}", time)
}

/// Formats a backup's age from its Unix stamp, given the current Unix time.
///
/// A backup is shown as "12 minutes ago", never a raw epoch number; a stamp in
/// the future (a clock skew) reads "just now" rather than a negative age.
pub fn config_backup_age(taken_at_secs: u64, now_secs: u64) -> String {
    let age = now_secs.saturating_sub(taken_at_secs);
    if age < 60 {
        if age == 0 {
            return UI_CONFIG_AGE_NOW.to_owned();
        }
        return UI_CONFIG_AGE_SECONDS.replace("{n}", &age.to_string());
    }
    let minutes = age / 60;
    if minutes < 60 {
        return UI_CONFIG_AGE_MINUTES.replace("{n}", &minutes.to_string());
    }
    let hours = minutes / 60;
    if hours < 24 {
        return UI_CONFIG_AGE_HOURS.replace("{n}", &hours.to_string());
    }
    let days = hours / 24;
    UI_CONFIG_AGE_DAYS.replace("{n}", &days.to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn a_byte_size_unit_says_what_may_be_typed_not_the_base_unit() {
        // `F9`: the field takes `16MB`, so "unit: bytes" next to `128MB` is a
        // contradiction; the note names the suffixes the field accepts.
        let note = config_unit("bytes").expect("a byte unit has a note");
        assert_eq!(
            note,
            format!("{UI_CONFIG_UNIT_PREFIX}{UI_CONFIG_UNIT_BYTES}")
        );
        assert!(
            note.contains("KB") && note.contains("MB") && note.contains("GB"),
            "the byte note names the accepted suffixes: {note}"
        );
    }

    #[test]
    fn a_duration_unit_says_seconds() {
        let note = config_unit("seconds").expect("a duration unit has a note");
        assert_eq!(
            note,
            format!("{UI_CONFIG_UNIT_PREFIX}{UI_CONFIG_UNIT_SECONDS}")
        );
    }

    #[test]
    fn an_empty_unit_has_no_note_and_an_unknown_one_is_verbatim() {
        assert_eq!(config_unit(""), None, "a unitless value shows no note");
        assert_eq!(
            config_unit("rows"),
            Some(format!("{UI_CONFIG_UNIT_PREFIX}rows")),
            "an unknown unit is shown verbatim, never dropped"
        );
    }
}
