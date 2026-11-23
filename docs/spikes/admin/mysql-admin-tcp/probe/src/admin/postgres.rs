//! Stub of the two record types `admin/mysql.rs` consumes. Copied verbatim from
//! `crates/core/src/admin/postgres.rs` (getters only); the PostgreSQL executor
//! itself is not part of this probe.
pub use super::reset::CredentialSink;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseInfo {
    name: String,
    owner: String,
    size_bytes: i64,
}

impl DatabaseInfo {
    pub fn new(name: impl Into<String>, owner: impl Into<String>, size_bytes: i64) -> Self {
        Self {
            name: name.into(),
            owner: owner.into(),
            size_bytes,
        }
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn owner(&self) -> &str {
        &self.owner
    }
    pub fn size_bytes(&self) -> i64 {
        self.size_bytes
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserInfo {
    name: String,
    can_login: bool,
    is_superuser: bool,
}

impl UserInfo {
    pub fn new(name: impl Into<String>, can_login: bool, is_superuser: bool) -> Self {
        Self {
            name: name.into(),
            can_login,
            is_superuser,
        }
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn can_login(&self) -> bool {
        self.can_login
    }
    pub fn is_superuser(&self) -> bool {
        self.is_superuser
    }
}

pub use super::reset::PasswordEngine;
