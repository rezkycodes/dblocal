//! Pure state for the Config tab (`F-CFG-01`, `F-CFG-02`).
//!
//! The service is the only source of truth: the tab holds the `ConfigView` its
//! last read returned, the local edits not yet applied ("Apply"), the typed
//! error of each field, and which action is in flight. None of this needs a
//! window or a live service, so every branch is unit-tested here and the screen
//! only reads it.
//!
//! Two rules the PRD makes explicit shape this module:
//!
//! 1. **Edits are local until "Apply".** `edits` holds only the fields whose
//!    text differs from the file, keyed by the setting's key. An untouched field
//!    therefore always shows the value of the latest read, which is exactly what
//!    "re-base only the untouched fields" on a `ConfigChanged` means.
//! 2. **Never a service round trip for the obvious case.** A value is validated
//!    with the same `dblocal_core::config::validate_value` the service runs, so
//!    the form shows the service's own `DBL-8601` sentence without a call.

use std::collections::{BTreeMap, BTreeSet};

use dblocal_core::config::{ConfigApplied, ConfigSetting, ConfigView, spec_for, validate_value};
use dblocal_core::model::{EngineKey, InstanceId};
use dblocal_core::service::UserFacingError;

use super::admin_state::Loadable;

/// One action the Config tab can have in flight; each disables its own button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConfigAction {
    /// Reading the view and the raw path.
    Load,
    /// Applying the local edits (`F-CFG-01` AC2).
    Apply,
    /// Restoring the newest backup (`F-CFG-01` AC3).
    Restore,
    /// Opening the raw file in the OS handler (`F-CFG-02` AC1).
    OpenRaw,
}

/// The Config tab of one instance.
#[derive(Debug, Clone, Default)]
pub struct InstanceConfig {
    /// The read model, from the last read.
    pub data: Loadable<ConfigView>,
    /// Local edits: setting key → text the user typed, only when it differs.
    pub edits: BTreeMap<String, String>,
    /// Per-field validation failures, keyed by setting key.
    pub errors: BTreeMap<String, UserFacingError>,
    /// The last card-level failure (apply/restore/launcher), cleared by the next
    /// successful action.
    pub error: Option<UserFacingError>,
    /// Whether `error` came from a failed READ rather than an action. A re-read
    /// that succeeds replaces a read failure, but a failed apply/restore/open
    /// keeps its banner until the next action or "Reload" (`F10`).
    pub error_from_read: bool,
    /// Actions currently in flight for this instance.
    pub busy: BTreeSet<ConfigAction>,
    /// Result of the last successful apply; drives the restart banner.
    pub applied: Option<ConfigApplied>,
    /// A `ConfigChanged` arrived while this instance had unsaved edits.
    pub touched_outside: bool,
    /// A re-read is in flight while the last good view stays on screen, so a
    /// refresh never blanks the form or drops the focused field (`F7b`).
    pub refreshing: bool,
}

impl InstanceConfig {
    /// The view, when the last read succeeded.
    pub fn view(&self) -> Option<&ConfigView> {
        self.data.ready()
    }

    /// Whether `action` is in flight right now.
    pub fn is_busy(&self, action: ConfigAction) -> bool {
        self.busy.contains(&action)
    }

    /// Whether any action is in flight, so every control is disabled.
    pub fn any_busy(&self) -> bool {
        !self.busy.is_empty()
    }

    /// Marks an action as started, clearing the previous card error.
    pub fn begin(&mut self, action: ConfigAction) {
        self.busy.insert(action);
        if action != ConfigAction::Load {
            self.error = None;
            self.error_from_read = false;
        }
    }

    /// Marks an action as finished.
    pub fn end(&mut self, action: ConfigAction) {
        self.busy.remove(&action);
    }

    /// The text one field holds: the local edit, else the file's value, else
    /// the empty string (an unset key shows its default as a placeholder).
    pub fn field_text(&self, setting: &ConfigSetting) -> String {
        self.edits
            .get(&setting.key)
            .cloned()
            .or_else(|| setting.current.clone())
            .unwrap_or_default()
    }

    /// The editable rows in display order.
    pub fn editable(&self) -> Vec<&ConfigSetting> {
        self.view()
            .map(|view| view.settings.iter().filter(|s| !s.readonly).collect())
            .unwrap_or_default()
    }

    /// The read-only rows in display order.
    pub fn readonly(&self) -> Vec<&ConfigSetting> {
        self.view()
            .map(|view| view.settings.iter().filter(|s| s.readonly).collect())
            .unwrap_or_default()
    }

    /// The changed pairs, ready for `config_apply`.
    ///
    /// A field whose text equals the file's value is not sent; an empty value for
    /// a key the file does not set is "leave as is", not "write an empty line".
    pub fn changed_pairs(&self) -> Vec<(String, String)> {
        let Some(view) = self.view() else {
            return Vec::new();
        };
        view.settings
            .iter()
            .filter(|s| !s.readonly)
            .filter_map(|setting| {
                let text = self.field_text(setting);
                if text.is_empty() && setting.current.is_none() {
                    return None;
                }
                if setting.current.as_deref() == Some(text.as_str()) {
                    return None;
                }
                Some((setting.key.clone(), text))
            })
            .collect()
    }

    /// Whether "Apply" is enabled.
    ///
    /// Something must have changed, nothing may be in flight, and no field may
    /// carry a validation error: a blue button that silently does nothing while
    /// `DBL-8601` sits under a field is the defect this pins (`F5`).
    pub fn can_apply(&self) -> bool {
        !self.any_busy()
            && self.data.ready().is_some()
            && self.errors.is_empty()
            && !self.changed_pairs().is_empty()
    }

    /// Whether "Discard changes" is enabled.
    pub fn can_reset(&self) -> bool {
        !self.any_busy() && !self.edits.is_empty()
    }

    /// Whether a restore is offered (a backup exists and nothing is in flight).
    pub fn can_restore(&self) -> bool {
        !self.any_busy()
            && self
                .view()
                .is_some_and(|view| view.backup_available.is_some())
    }

    /// Records one typed field, replacing the previous text or clearing it when
    /// it goes back to the file's value.
    pub fn set_field(&mut self, key: &str, text: &str) {
        let Some(view) = self.view() else {
            return;
        };
        let current = view
            .settings
            .iter()
            .find(|setting| setting.key == key)
            .and_then(|setting| setting.current.clone())
            .unwrap_or_default();
        if text == current {
            self.edits.remove(key);
        } else {
            self.edits.insert(key.to_owned(), text.to_owned());
        }
        self.errors.remove(key);
    }

    /// Drops every local edit, going back to the file's values.
    pub fn reset_edits(&mut self) {
        self.edits.clear();
        self.errors.clear();
    }

    /// Validates every changed field with the core validator, filling `errors`.
    ///
    /// Returns `true` when every field is valid, i.e. the call may go out. A bad
    /// value never reaches the service and the form shows its `DBL-8601` text.
    pub fn validate(&mut self, engine: EngineKey) -> bool {
        self.errors.clear();
        for (key, value) in self.changed_pairs() {
            let Some(spec) = spec_for(engine, &key) else {
                continue;
            };
            if let Err(err) = validate_value(spec, &value) {
                self.errors.insert(key, UserFacingError::from(err));
            }
        }
        self.errors.is_empty()
    }

    /// Re-bases the local edits onto a freshly read view after `ConfigChanged`.
    ///
    /// An edit whose text now equals the new file value is no longer an edit, so
    /// it is dropped; every untouched field already renders the new value. The
    /// flag is only raised when an edit actually survived, so the banner says
    /// "File changed outside the app" only when there is a field to protect.
    pub fn rebase(&mut self, view: ConfigView) {
        // Whether the file itself differs between the view on screen and the one
        // just read. A re-read that returned the same file must never raise the
        // "changed outside" banner (`F7c`); comparing the whole read model also
        // covers a change to a key nobody has an edit on.
        let file_changed = self
            .view()
            .is_none_or(|old| view_values(old) != view_values(&view));
        let engine = view.engine;
        self.data = Loadable::Ready(view.clone());
        let current: BTreeMap<&str, &str> = view
            .settings
            .iter()
            .filter_map(|s| s.current.as_deref().map(|v| (s.key.as_str(), v)))
            .collect();
        self.edits
            .retain(|key, text| current.get(key.as_str()) != Some(&text.as_str()));
        // `errors` must always match the remaining edits: a surviving `abc` keeps
        // its `DBL-8601` across the re-read, so "Apply" never turns blue while
        // a field is invalid (`F8`). Re-validating is what an apply would do.
        self.validate(engine);
        // The banner shows only when the file really changed outside and the user
        // still has an unsaved edit to protect; when the read returned the same
        // file, or every edit already matches the new one, there is nothing to
        // warn about.
        self.touched_outside = file_changed && !self.edits.is_empty();
    }

    /// Clears the card error explicitly ("Reload" / the next user action).
    ///
    /// The one way a failed apply/restore/open banner goes away besides a new
    /// action; a successful re-read alone no longer does (`F10`).
    pub fn dismiss_card_error(&mut self) {
        self.error = None;
        self.error_from_read = false;
    }

    /// Marks a re-read as started without blanking the view on screen.
    ///
    /// A refresh keeps the last good `Ready` rows (and every field entity and
    /// its focus) visible; only a tab that has nothing yet goes to `Loading`, so
    /// its card can honestly say "Loading..." (`F7b`).
    pub fn begin_refresh(&mut self) {
        self.refreshing = true;
        if self.data.ready().is_none() {
            self.data = Loadable::Loading;
        }
        // A read failure is what the next read replaces; a failed apply or open
        // keeps its banner until the next action or "Reload" (`F10`).
        if self.error_from_read {
            self.error = None;
            self.error_from_read = false;
        }
    }

    /// Marks the re-read as finished and records its outcome.
    ///
    /// A fresh view re-bases the edits (only what really differs survives); a
    /// failure is the card error while the previous rows stay on screen.
    pub fn end_refresh_ok(&mut self, view: ConfigView) {
        self.refreshing = false;
        self.rebase(view);
    }

    /// Records a failed re-read; the rows on screen are kept, never blanked.
    pub fn end_refresh_failed(&mut self, err: UserFacingError) {
        self.refreshing = false;
        if self.data.ready().is_none() {
            self.data = Loadable::Failed(err.clone());
        }
        self.error = Some(err);
        self.error_from_read = true;
    }
}

/// Every current value of a view, keyed by setting, for the "did the file
/// change" comparison in [`InstanceConfig::rebase`].
fn view_values(view: &ConfigView) -> BTreeMap<&str, Option<&str>> {
    view.settings
        .iter()
        .map(|s| (s.key.as_str(), s.current.as_deref()))
        .collect()
}

/// Every instance's Config tab state, keyed by instance.
#[derive(Debug, Clone, Default)]
pub struct ConfigState {
    instances: std::collections::HashMap<InstanceId, InstanceConfig>,
}

impl ConfigState {
    /// The view for one instance, created empty on first access.
    pub fn entry(&mut self, id: &InstanceId) -> &mut InstanceConfig {
        self.instances.entry(id.clone()).or_default()
    }

    /// Read-only access; `None` when the tab was never opened for the instance.
    pub fn get(&self, id: &InstanceId) -> Option<&InstanceConfig> {
        self.instances.get(id)
    }

    /// Drops the state of a removed instance.
    pub fn forget(&mut self, id: &InstanceId) {
        self.instances.remove(id);
    }
}

/// A stable element id for one setting row, derived from the instance and key.
///
/// The a11y duplicate-node trap: two instances share the same keys
/// (`max_connections`), so the id carries the instance id as well.
pub fn config_row_id(kind: &str, instance: &InstanceId, key: &str) -> String {
    format!("{kind}-{instance}-{key}")
}

/// A stable element id for one control in the Config tab of one instance.
pub fn config_action_id(kind: &str, instance: &InstanceId) -> String {
    format!("{kind}-{instance}")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use std::path::PathBuf;

    use dblocal_core::config::BackupInfo;

    use super::*;

    fn setting(key: &str, current: Option<&str>, default: &str, readonly: bool) -> ConfigSetting {
        ConfigSetting {
            key: key.to_owned(),
            current: current.map(str::to_owned),
            default: default.to_owned(),
            unit: String::new(),
            restart_required: !readonly,
            description: "desc".to_owned(),
            readonly,
        }
    }

    fn view(engine: EngineKey) -> ConfigView {
        ConfigView {
            id: InstanceId::new(),
            engine,
            settings: vec![
                setting("max_connections", Some("100"), "100", false),
                setting("shared_buffers", None, "128MB", false),
                setting("port", Some("5432"), "5432", true),
            ],
            backup_available: None,
            raw_path: PathBuf::from("/data/postgresql.conf"),
            restart_required: false,
        }
    }

    fn ready(engine: EngineKey) -> InstanceConfig {
        InstanceConfig {
            data: Loadable::Ready(view(engine)),
            ..InstanceConfig::default()
        }
    }

    #[test]
    fn a_field_shows_the_edit_then_the_file_value_then_nothing() {
        let mut config = ready(EngineKey::Postgresql);
        let view = config.view().cloned().expect("view");
        let max = &view.settings[0];
        let buffers = &view.settings[1];
        assert_eq!(config.field_text(max), "100");
        assert_eq!(
            config.field_text(buffers),
            "",
            "unset shows the placeholder"
        );
        config.set_field("max_connections", "200");
        config.set_field("shared_buffers", "256MB");
        assert_eq!(config.field_text(max), "200");
        assert_eq!(config.field_text(buffers), "256MB");
    }

    #[test]
    fn setting_a_field_back_to_the_file_value_clears_the_edit() {
        let mut config = ready(EngineKey::Postgresql);
        config.set_field("max_connections", "200");
        assert_eq!(config.changed_pairs().len(), 1);
        config.set_field("max_connections", "100");
        assert!(config.edits.is_empty(), "an unchanged field is not an edit");
        assert!(config.changed_pairs().is_empty());
    }

    #[test]
    fn an_empty_value_for_an_unset_key_is_not_sent() {
        let mut config = ready(EngineKey::Postgresql);
        // The unset key left empty is "leave as is", not an empty write.
        assert!(config.changed_pairs().is_empty());
        config.set_field("shared_buffers", "512MB");
        let pairs = config.changed_pairs();
        assert_eq!(
            pairs,
            vec![("shared_buffers".to_owned(), "512MB".to_owned())]
        );
    }

    #[test]
    fn read_only_rows_are_listed_separately_and_never_edited() {
        let config = ready(EngineKey::Postgresql);
        assert_eq!(config.editable().len(), 2);
        assert_eq!(config.readonly().len(), 1);
        assert_eq!(config.readonly()[0].key, "port");
    }

    #[test]
    fn can_apply_tracks_changes_and_the_in_flight_flag() {
        let mut config = ready(EngineKey::Postgresql);
        assert!(!config.can_apply(), "nothing changed yet");
        config.set_field("max_connections", "250");
        assert!(config.can_apply());
        config.begin(ConfigAction::Apply);
        assert!(!config.can_apply(), "a call in flight disables Apply");
        config.end(ConfigAction::Apply);
        assert!(config.can_apply());
    }

    #[test]
    fn validation_uses_the_core_message_and_stops_a_bad_value() {
        let mut config = ready(EngineKey::Postgresql);
        config.set_field("max_connections", "not-a-number");
        assert!(!config.validate(EngineKey::Postgresql));
        let err = config.errors.get("max_connections").expect("field error");
        assert_eq!(err.code, "DBL-8601");

        config.set_field("max_connections", "200");
        assert!(config.validate(EngineKey::Postgresql));
        assert!(config.errors.is_empty());
    }

    #[test]
    fn validation_rejects_a_managed_or_unknown_engine_change() {
        // A value that is out of range for the engine is refused locally.
        let mut config = ready(EngineKey::Postgresql);
        config.set_field("max_connections", "0");
        assert!(!config.validate(EngineKey::Postgresql));
        assert_eq!(
            config.errors.get("max_connections").map(|e| e.code.clone()),
            Some("DBL-8601".to_owned())
        );
    }

    #[test]
    fn busy_tracks_actions_independently_and_begin_clears_the_card_error() {
        let mut config = ready(EngineKey::Postgresql);
        config.error = Some(UserFacingError::new("DBL-8604", "busy"));
        config.begin(ConfigAction::Restore);
        assert!(config.is_busy(ConfigAction::Restore));
        assert!(!config.is_busy(ConfigAction::Apply));
        assert!(config.error.is_none(), "a new action clears the card error");
        assert!(config.any_busy());
        config.end(ConfigAction::Restore);
        assert!(!config.any_busy());
    }

    #[test]
    fn restore_needs_a_backup_and_a_free_tab() {
        let mut config = InstanceConfig {
            data: Loadable::Ready(view(EngineKey::Postgresql)),
            ..InstanceConfig::default()
        };
        assert!(!config.can_restore(), "no backup yet");
        let mut with_backup = view(EngineKey::Postgresql);
        with_backup.backup_available = Some(BackupInfo {
            taken_at: 1_700_000_000,
            taken_at_secs: 1_700_000_000,
            path: PathBuf::from("/b.bak"),
        });
        config.data = Loadable::Ready(with_backup);
        assert!(config.can_restore());
        config.begin(ConfigAction::Restore);
        assert!(!config.can_restore());
    }

    #[test]
    fn a_rebase_keeps_only_edits_that_still_differ_and_flags_the_banner() {
        let mut config = ready(EngineKey::Postgresql);
        config.set_field("max_connections", "200");
        // The file changed: `max_connections` now reads 100 still, but the
        // untouched `shared_buffers` gained a value; the edit survives.
        let mut changed = view(EngineKey::Postgresql);
        changed.settings[1].current = Some("256MB".to_owned());
        config.rebase(changed.clone());
        assert_eq!(
            config.edits.get("max_connections").map(String::as_str),
            Some("200")
        );
        assert!(config.touched_outside, "a surviving edit raises the banner");
        assert_eq!(
            config.field_text(&changed.settings[1]),
            "256MB",
            "an untouched field shows the new value"
        );

        // A later re-read that already carries the edited value drops the edit and
        // the banner, because nothing is left to protect.
        let mut same = view(EngineKey::Postgresql);
        same.settings[0].current = Some("200".to_owned());
        config.rebase(same.clone());
        assert!(config.edits.is_empty());
        assert!(!config.touched_outside);
    }

    #[test]
    fn can_apply_is_false_while_a_field_error_exists() {
        let mut config = ready(EngineKey::Postgresql);
        config.set_field("max_connections", "x");
        assert!(!config.validate(EngineKey::Postgresql));
        assert!(
            !config.can_apply(),
            "F5: Apply is disabled while a field shows an error"
        );
        // Clearing the error re-enables it (the change is still there).
        config.set_field("max_connections", "200");
        assert!(config.validate(EngineKey::Postgresql));
        assert!(config.can_apply());
    }

    #[test]
    fn a_re_read_that_returns_the_same_file_does_not_raise_the_banner() {
        let mut config = ready(EngineKey::Postgresql);
        config.set_field("max_connections", "200");
        // The file did not change: the same view comes back.
        config.rebase(view(EngineKey::Postgresql));
        assert!(
            !config.touched_outside,
            "F7c: an unchanged file must not claim it changed outside"
        );
        assert_eq!(
            config.edits.get("max_connections").map(String::as_str),
            Some("200"),
            "the edit survives a no-op re-read"
        );
    }

    #[test]
    fn a_re_read_that_changed_a_key_raises_the_banner_even_without_an_edit_on_it() {
        let mut config = ready(EngineKey::Postgresql);
        config.set_field("max_connections", "200");
        let mut changed = view(EngineKey::Postgresql);
        changed.settings[1].current = Some("256MB".to_owned());
        config.rebase(changed);
        assert!(
            config.touched_outside,
            "F7c: a real file change raises the banner even with no edit on that key"
        );
    }

    #[test]
    fn a_refresh_keeps_the_rows_on_screen_and_only_a_first_read_says_loading() {
        let mut config = ready(EngineKey::Postgresql);
        config.begin_refresh();
        assert!(
            config.view().is_some(),
            "F7b: a refresh keeps the last good view on screen"
        );
        assert!(config.refreshing);

        // No view yet: the card honestly says 'Loading...'.
        let mut fresh = InstanceConfig::default();
        fresh.begin_refresh();
        assert!(fresh.view().is_none(), "F7b: nothing to show yet");
        assert!(fresh.data.is_loading());
    }

    #[test]
    fn a_failed_refresh_keeps_the_previous_rows_and_shows_the_error() {
        let mut config = ready(EngineKey::Postgresql);
        config.begin_refresh();
        config.end_refresh_failed(UserFacingError::new("DBL-8603", "read failed"));
        assert!(!config.refreshing);
        assert!(config.view().is_some(), "F7b: the rows stay on screen");
        assert_eq!(
            config.error.as_ref().map(|e| e.code.as_str()),
            Some("DBL-8603")
        );
    }

    #[test]
    fn reset_drops_every_edit_and_field_error() {
        let mut config = ready(EngineKey::Postgresql);
        config.set_field("max_connections", "x");
        assert!(!config.validate(EngineKey::Postgresql));
        config.reset_edits();
        assert!(config.edits.is_empty());
        assert!(config.errors.is_empty());
        assert!(!config.can_reset());
    }

    #[test]
    fn state_is_keyed_per_instance_and_forget_drops_one() {
        let mut state = ConfigState::default();
        let a = InstanceId::new();
        let b = InstanceId::new();
        state.entry(&a).data = Loadable::Ready(view(EngineKey::Postgresql));
        state.entry(&b);
        assert!(state.get(&a).is_some());
        state.forget(&a);
        assert!(state.get(&a).is_none());
        assert!(state.get(&b).is_some());
    }

    #[test]
    fn row_ids_carry_the_instance_so_two_instances_cannot_collide() {
        let one = InstanceId::new();
        let two = InstanceId::new();
        assert_ne!(
            config_row_id("config-row", &one, "max_connections"),
            config_row_id("config-row", &two, "max_connections")
        );
        assert_ne!(
            config_action_id("config-apply", &one),
            config_action_id("config-apply", &two)
        );
        assert_ne!(
            config_row_id("config-row", &one, "port"),
            config_action_id("config-port", &one)
        );
    }
}

#[cfg(test)]
mod f8_tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use std::path::PathBuf;

    use dblocal_core::config::ConfigView;
    use dblocal_core::model::EngineKey;
    use dblocal_core::service::UserFacingError;

    use super::*;

    fn view(engine: EngineKey, max: Option<&str>) -> ConfigView {
        let mut view = ConfigView::new(
            InstanceId::new(),
            engine,
            PathBuf::from("/mock/postgresql.conf"),
        );
        view.settings.push(ConfigSetting {
            key: "max_connections".to_owned(),
            current: max.map(str::to_owned),
            default: "100".to_owned(),
            unit: String::new(),
            restart_required: true,
            description: "desc".to_owned(),
            readonly: false,
        });
        view.settings.push(ConfigSetting {
            key: "shared_buffers".to_owned(),
            current: None,
            default: "128MB".to_owned(),
            unit: "bytes".to_owned(),
            restart_required: true,
            description: "desc".to_owned(),
            readonly: false,
        });
        view
    }

    fn ready(engine: EngineKey) -> InstanceConfig {
        InstanceConfig {
            data: Loadable::Ready(view(engine, Some("100"))),
            ..InstanceConfig::default()
        }
    }

    #[test]
    fn an_invalid_edit_keeps_its_error_and_terapkan_stays_disabled_after_a_re_read() {
        let mut config = ready(EngineKey::Postgresql);
        config.set_field("max_connections", "abc");
        assert!(!config.validate(EngineKey::Postgresql));
        assert!(!config.can_apply(), "invalid before the re-read");

        // The window is re-activated: a re-read of the SAME file re-bases.
        config.rebase(view(EngineKey::Postgresql, Some("100")));
        assert_eq!(
            config.edits.get("max_connections").map(String::as_str),
            Some("abc"),
            "F8: the typed text survives the re-read"
        );
        assert_eq!(
            config
                .errors
                .get("max_connections")
                .map(|e| e.code.as_str()),
            Some("DBL-8601"),
            "F8: the surviving invalid edit keeps its DBL-8601"
        );
        assert!(
            !config.can_apply(),
            "F8: Apply never turns blue while a field is invalid"
        );
    }

    #[test]
    fn a_valid_edit_has_no_error_after_a_re_read() {
        let mut config = ready(EngineKey::Postgresql);
        config.set_field("max_connections", "300");
        assert!(config.validate(EngineKey::Postgresql));
        config.rebase(view(EngineKey::Postgresql, Some("100")));
        assert!(config.errors.is_empty(), "F8: a valid edit is not flagged");
        assert!(config.can_apply(), "F8: and Apply is enabled");
    }

    #[test]
    fn an_edit_the_new_file_value_makes_equal_disappears_with_its_error() {
        let mut config = ready(EngineKey::Postgresql);
        config.set_field("max_connections", "abc");
        assert!(!config.validate(EngineKey::Postgresql));
        // The file now assigns exactly the typed text: it is no longer an edit.
        config.rebase(view(EngineKey::Postgresql, Some("abc")));
        assert!(
            config.edits.is_empty(),
            "F8: an edit equal to the file is dropped"
        );
        assert!(config.errors.is_empty(), "F8: and its error goes with it");
    }

    #[test]
    fn a_failed_apply_error_survives_a_successful_re_read_until_dismissed() {
        let mut config = ready(EngineKey::Postgresql);
        config.set_field("max_connections", "300");
        config.begin(ConfigAction::Apply);
        config.error = Some(UserFacingError::new("DBL-8604", "busy"));
        config.end(ConfigAction::Apply);
        // A re-read lands and succeeds: the failed-apply banner stays (`F10`).
        config.begin_refresh();
        config.end_refresh_ok(view(EngineKey::Postgresql, Some("100")));
        assert_eq!(
            config.error.as_ref().map(|e| e.code.as_str()),
            Some("DBL-8604"),
            "F10: a failed apply keeps its banner through a re-read"
        );

        // "Reload" is the explicit dismissal.
        config.dismiss_card_error();
        assert!(config.error.is_none(), "F10: Reload clears it");
    }

    #[test]
    fn a_failed_read_error_is_replaced_by_the_next_read() {
        let mut config = ready(EngineKey::Postgresql);
        config.begin_refresh();
        config.end_refresh_failed(UserFacingError::new("DBL-8603", "read failed"));
        assert_eq!(
            config.error.as_ref().map(|e| e.code.as_str()),
            Some("DBL-8603")
        );
        // The next read is a successful one: a read error is what it replaces.
        config.begin_refresh();
        assert!(
            config.error.is_none(),
            "F10: a read error is cleared as the next read starts"
        );
        config.end_refresh_ok(view(EngineKey::Postgresql, Some("100")));
        assert!(config.error.is_none());
    }
}
