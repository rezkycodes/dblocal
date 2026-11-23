//! Pure state for the Databases tab (`F-DBU-01..03`).
//!
//! The service is the only source of truth: the tab holds what the last read
//! returned, which typed error it got, and which action is in flight. None of
//! this needs a window or a live service, so every branch is unit-tested here
//! and the view only reads it.
//!
//! Identifier validation deliberately calls `dblocal_core::admin::Identifier`
//! instead of re-implementing the allowlist: the form rejects the obvious bad
//! name with the same rule (and the same Indonesian message) the service would
//! apply, without a round trip (`F-DBU-01` AC4).

use std::collections::{BTreeSet, HashMap};

use dblocal_core::admin::Identifier;
use dblocal_core::model::InstanceId;
use dblocal_core::service::{DatabaseInfo, PrivilegeLevel, UserFacingError, UserInfo};

/// One read that may not have happened yet.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Loadable<T> {
    /// Nothing was requested yet.
    #[default]
    Idle,
    /// A read is in flight.
    Loading,
    /// The last read succeeded.
    Ready(T),
    /// The last read failed with a typed error.
    Failed(UserFacingError),
}

impl<T> Loadable<T> {
    /// The value, when the last read succeeded.
    pub fn ready(&self) -> Option<&T> {
        match self {
            Self::Ready(value) => Some(value),
            _ => None,
        }
    }

    /// Whether a read is in flight.
    pub fn is_loading(&self) -> bool {
        matches!(self, Self::Loading)
    }

    /// The typed error of a failed read.
    pub fn error(&self) -> Option<&UserFacingError> {
        match self {
            Self::Failed(err) => Some(err),
            _ => None,
        }
    }
}

/// The rows one instance's Databases tab shows, as last read from the service.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdminData {
    /// Databases with owner and size (`F-DBU-01` AC1).
    pub databases: Vec<DatabaseInfo>,
    /// Login-capable roles (`F-DBU-02`).
    pub users: Vec<UserInfo>,
}

/// One actions the tab can have in flight; each disables its own button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AdminAction {
    /// Reading databases and users.
    Load,
    /// Creating a database (`F-DBU-01` AC2).
    CreateDatabase,
    /// Dropping a database (`F-DBU-01` AC3).
    DropDatabase,
    /// Creating a user (`F-DBU-02`).
    CreateUser,
    /// Resetting the superuser password (`F-DBU-03`).
    ResetPassword,
}

/// The Databases tab of one instance.
#[derive(Debug, Clone, Default)]
pub struct InstanceAdmin {
    /// Databases and users, from the last read.
    pub data: Loadable<AdminData>,
    /// Actions currently in flight for this instance.
    pub busy: BTreeSet<AdminAction>,
    /// The last card-level failure, cleared by the next successful action.
    pub error: Option<UserFacingError>,
    /// Whether the last reset failed; the card then says the old password works.
    pub reset_failed: bool,
}

impl InstanceAdmin {
    /// The rows the view shows, or an empty slice before the first read.
    pub fn rows(&self) -> &[DatabaseInfo] {
        self.data
            .ready()
            .map(|data| data.databases.as_slice())
            .unwrap_or_default()
    }

    /// The users the view shows.
    pub fn users(&self) -> &[UserInfo] {
        self.data
            .ready()
            .map(|data| data.users.as_slice())
            .unwrap_or_default()
    }

    /// Whether the view should show the "Membaca database…" line.
    ///
    /// `Idle` counts: on a running instance the read is issued when the tab is
    /// entered, so a frame drawn before it lands must not claim the list is
    /// empty. A stopped instance never reaches the table (it has its own state).
    pub fn pending(&self) -> bool {
        matches!(self.data, Loadable::Idle | Loadable::Loading)
    }

    /// Whether `action` is in flight right now.
    pub fn is_busy(&self, action: AdminAction) -> bool {
        self.busy.contains(&action)
    }

    /// Marks an action as started, clearing the previous card error.
    pub fn begin(&mut self, action: AdminAction) {
        self.busy.insert(action);
        if action != AdminAction::Load {
            self.error = None;
        }
    }

    /// Marks an action as finished.
    pub fn end(&mut self, action: AdminAction) {
        self.busy.remove(&action);
    }
}

/// Every instance's Databases tab state, keyed by instance.
#[derive(Debug, Clone, Default)]
pub struct AdminState {
    instances: HashMap<InstanceId, InstanceAdmin>,
}

impl AdminState {
    /// The view for one instance, created empty on first access.
    pub fn entry(&mut self, id: &InstanceId) -> &mut InstanceAdmin {
        self.instances.entry(id.clone()).or_default()
    }

    /// Read-only access; `None` when the tab was never opened for the instance.
    pub fn get(&self, id: &InstanceId) -> Option<&InstanceAdmin> {
        self.instances.get(id)
    }

    /// Drops the state of a removed instance.
    pub fn forget(&mut self, id: &InstanceId) {
        self.instances.remove(id);
    }

    /// Marks an instance's rows unread after its status changed.
    ///
    /// A read taken while the server was stopped (or before it started) says
    /// nothing about the server that is running now, so the rows go back to
    /// "not read" and the next entry issues a fresh read. A read already in
    /// flight keeps its slot: its answer is newer than this transition.
    pub fn invalidate(&mut self, id: &InstanceId) {
        let Some(admin) = self.instances.get_mut(id) else {
            return;
        };
        if !admin.data.is_loading() {
            admin.data = Loadable::Idle;
        }
        // A failure or a "the old password still works" note describes the
        // server that just went away; neither survives the transition.
        admin.error = None;
        admin.reset_failed = false;
    }
}

/// A validated database name, ready to send.
///
/// Uses the core allowlist, so the form and the service cannot disagree about
/// what a legal name is (`F-DBU-01` AC4).
pub fn validate_database_name(raw: &str) -> Result<String, UserFacingError> {
    Identifier::new(raw)
        .map(|id| id.as_str().to_owned())
        .map_err(UserFacingError::from)
}

/// A validated role name, using the role allowlist (no reserved-db rule).
pub fn validate_user_name(raw: &str) -> Result<String, UserFacingError> {
    Identifier::role(raw)
        .map(|id| id.as_str().to_owned())
        .map_err(UserFacingError::from)
}

/// Whether the typed text confirms dropping `name` (`F-DBU-01` AC3).
///
/// The confirm button stays disabled until the text equals the name exactly.
pub fn drop_confirmation_matches(typed: &str, name: &str) -> bool {
    !name.is_empty() && typed == name
}

/// The Indonesian label of a privilege level.
pub fn level_label(level: PrivilegeLevel) -> &'static str {
    match level {
        PrivilegeLevel::ReadOnly => dblocal_core::strings::UI_DATABASES_LEVEL_READ_ONLY,
        PrivilegeLevel::ReadWrite => dblocal_core::strings::UI_DATABASES_LEVEL_READ_WRITE,
    }
}

/// The label a listed role shows: superuser, or plain login.
///
/// The read model does not carry a role's grants, so the list never claims a
/// level it did not read; the superuser flag is the one fact it does carry.
pub fn user_label(user: &UserInfo) -> &'static str {
    if user.is_superuser() {
        dblocal_core::strings::UI_DATABASES_LEVEL_SUPERUSER
    } else {
        dblocal_core::strings::UI_DATABASES_LEVEL_LOGIN
    }
}

/// A stable element id for a database or user row.
///
/// The a11y duplicate-node trap: two instances can hold a database of the same
/// name (`app`), so the id carries the instance as well as the row name.
pub fn row_id(kind: &str, instance: &InstanceId, name: &str) -> String {
    format!("{kind}-{instance}-{name}")
}

/// A stable element id for one control inside a row.
pub fn row_part_id(kind: &str, instance: &InstanceId, name: &str, part: &str) -> String {
    format!("{kind}-{instance}-{name}-{part}")
}

/// A stable element id for a dialog owned by one instance.
pub fn dialog_id(kind: &str, instance: &InstanceId) -> String {
    format!("{kind}-{instance}")
}

/// A stable element id for a dialog about one database of one instance.
pub fn dialog_row_id(kind: &str, instance: &InstanceId, name: &str) -> String {
    format!("{kind}-{instance}-{name}")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    fn id() -> InstanceId {
        InstanceId::new()
    }

    #[test]
    fn a_loadable_reports_its_four_states() {
        let mut value: Loadable<u8> = Loadable::Idle;
        assert!(value.ready().is_none());
        assert!(!value.is_loading());
        assert!(value.error().is_none());

        value = Loadable::Loading;
        assert!(value.is_loading());
        assert!(value.ready().is_none());

        value = Loadable::Ready(7);
        assert_eq!(value.ready(), Some(&7));

        value = Loadable::Failed(UserFacingError::new("DBL-8111", "unsupported"));
        assert_eq!(value.error().map(|e| e.code.as_str()), Some("DBL-8111"));
        assert!(value.ready().is_none());
    }

    #[test]
    fn database_names_use_the_core_allowlist_and_its_message() {
        assert_eq!(validate_database_name("app_dev").expect("valid"), "app_dev");
        assert_eq!(validate_database_name("App2").expect("valid"), "App2");

        for bad in ["", "1app", "app-dev", "app dev", "app;drop", "postgres"] {
            let err = validate_database_name(bad).expect_err(bad);
            assert_eq!(
                err.code, "DBL-8101",
                "{bad} must be rejected with the identifier code"
            );
            assert_eq!(
                err.message,
                dblocal_core::admin::messages::ERR_INVALID_IDENTIFIER,
                "{bad}: the form must show the service's own sentence"
            );
        }
    }

    #[test]
    fn role_names_allow_the_default_superuser() {
        assert_eq!(validate_user_name("app_rw").expect("valid"), "app_rw");
        // `postgres` is a legal role but a reserved database name.
        assert_eq!(validate_user_name("postgres").expect("role"), "postgres");
        assert!(validate_database_name("postgres").is_err());
        assert!(validate_user_name("app-rw").is_err());
    }

    #[test]
    fn drop_needs_the_exact_name_and_never_an_empty_one() {
        assert!(drop_confirmation_matches("app", "app"));
        assert!(!drop_confirmation_matches("App", "app"));
        assert!(!drop_confirmation_matches("app ", "app"));
        assert!(!drop_confirmation_matches("", "app"));
        assert!(
            !drop_confirmation_matches("", ""),
            "an empty target can never be confirmed"
        );
    }

    #[test]
    fn row_ids_do_not_collide_between_instances() {
        let one = id();
        let two = id();
        let a = row_id("db", &one, "app");
        let b = row_id("db", &two, "app");
        assert_ne!(a, b, "two instances with the same database name collided");
        assert_eq!(a, row_id("db", &one, "app"));
        assert_ne!(a, row_part_id("db", &one, "app", "name"));
        assert_ne!(dialog_id("drop", &one), dialog_row_id("drop", &one, "app"));
    }

    #[test]
    fn busy_tracks_one_action_without_affecting_the_others() {
        let mut admin = InstanceAdmin {
            error: Some(UserFacingError::new("DBL-8104", "exists")),
            ..InstanceAdmin::default()
        };
        admin.begin(AdminAction::CreateDatabase);
        assert!(admin.is_busy(AdminAction::CreateDatabase));
        assert!(!admin.is_busy(AdminAction::CreateUser));
        assert!(admin.error.is_none(), "a new action clears the card error");

        admin.begin(AdminAction::CreateUser);
        assert!(admin.is_busy(AdminAction::CreateDatabase));
        assert!(admin.is_busy(AdminAction::CreateUser));

        admin.end(AdminAction::CreateDatabase);
        assert!(!admin.is_busy(AdminAction::CreateDatabase));
        assert!(admin.is_busy(AdminAction::CreateUser));
    }

    #[test]
    fn rows_and_users_are_empty_until_a_read_succeeds() {
        let mut admin = InstanceAdmin::default();
        assert!(admin.rows().is_empty());
        assert!(admin.users().is_empty());

        admin.data = Loadable::Loading;
        assert!(admin.rows().is_empty());

        admin.data = Loadable::Ready(AdminData::default());
        assert!(admin.rows().is_empty());

        admin.data = Loadable::Failed(UserFacingError::new("DBL-8111", "no"));
        assert!(admin.users().is_empty());
    }

    #[test]
    fn state_is_keyed_per_instance_and_forget_drops_one() {
        let mut state = AdminState::default();
        let a = id();
        let b = id();
        state.entry(&a).data = Loadable::Ready(AdminData::default());
        state.entry(&b);
        assert!(state.get(&a).is_some());
        assert!(state.get(&b).is_some());

        state.forget(&a);
        assert!(state.get(&a).is_none());
        assert!(state.get(&b).is_some());
    }

    #[test]
    fn level_and_user_labels_are_the_indonesian_strings() {
        assert_eq!(
            level_label(PrivilegeLevel::ReadOnly),
            dblocal_core::strings::UI_DATABASES_LEVEL_READ_ONLY
        );
        assert_eq!(
            level_label(PrivilegeLevel::ReadWrite),
            dblocal_core::strings::UI_DATABASES_LEVEL_READ_WRITE
        );
        assert_eq!(
            user_label(&UserInfo::new("postgres", true, true)),
            dblocal_core::strings::UI_DATABASES_LEVEL_SUPERUSER
        );
        assert_eq!(
            user_label(&UserInfo::new("app_rw", true, false)),
            dblocal_core::strings::UI_DATABASES_LEVEL_LOGIN
        );
    }
}
