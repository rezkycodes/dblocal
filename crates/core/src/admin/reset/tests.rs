//! Tests for the shared F-DBU-03 AC2 reset state machine.

use std::sync::atomic::{AtomicBool, Ordering};

use super::*;
use crate::admin::error::AdminError;

struct FakeEngine {
    apply_fails: AtomicBool,
    verify_fails: AtomicBool,
    applied: std::sync::Mutex<Vec<String>>,
}

impl FakeEngine {
    fn new() -> Self {
        Self {
            apply_fails: AtomicBool::new(false),
            verify_fails: AtomicBool::new(false),
            applied: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn failing_apply() -> Self {
        let engine = Self::new();
        engine.apply_fails.store(true, Ordering::SeqCst);
        engine
    }

    fn failing_verify() -> Self {
        let engine = Self::new();
        engine.verify_fails.store(true, Ordering::SeqCst);
        engine
    }

    fn applied(&self) -> Vec<String> {
        self.applied.lock().expect("lock").clone()
    }
}

impl PasswordEngine for FakeEngine {
    fn apply(&self, _connect_as: &SecretString, target: &SecretString) -> AdminResult<()> {
        self.applied
            .lock()
            .expect("lock")
            .push(target.expose().to_owned());
        if self.apply_fails.load(Ordering::SeqCst) {
            return Err(AdminError::Connect);
        }
        Ok(())
    }

    fn verify(&self, password: &SecretString) -> AdminResult<()> {
        // Fail only the new password's verification, so the rollback's own
        // verification of the old password still succeeds.
        if self.verify_fails.load(Ordering::SeqCst) && password.expose() == "new-canary-password" {
            return Err(AdminError::Connect);
        }
        Ok(())
    }
}

struct StubSink {
    fail: bool,
}

impl CredentialSink for StubSink {
    fn store(&self, _password: &SecretString) -> crate::error::Result<()> {
        if self.fail {
            Err(crate::error::DblError::Secret {
                reason: "keychain locked".to_owned(),
            })
        } else {
            Ok(())
        }
    }
}

fn new_password() -> SecretString {
    SecretString::from("new-canary-password")
}

#[test]
fn reset_succeeds_and_returns_the_password_once() {
    let engine = FakeEngine::new();
    let sink = StubSink { fail: false };
    let old = SecretString::from("old-canary");
    let returned = reset_with(&engine, &sink, &old, new_password()).expect("reset");
    assert_eq!(returned.expose(), "new-canary-password");
    // Applied exactly once (no rollback).
    assert_eq!(engine.applied(), vec!["new-canary-password".to_owned()]);
}

#[test]
fn apply_failure_changes_nothing_and_does_not_roll_back() {
    let engine = FakeEngine::failing_apply();
    let sink = StubSink { fail: false };
    let old = SecretString::from("old-canary");
    let err = reset_with(&engine, &sink, &old, new_password()).expect_err("apply fails");
    assert_eq!(err, AdminError::Connect);
    // Only the (failing) new password was attempted: no rollback write.
    assert_eq!(engine.applied(), vec!["new-canary-password".to_owned()]);
}

#[test]
fn verify_failure_rolls_back_to_the_old_password() {
    let engine = FakeEngine::failing_verify();
    let sink = StubSink { fail: false };
    let old = SecretString::from("old-canary");
    let err = reset_with(&engine, &sink, &old, new_password()).expect_err("verify fails");
    assert_eq!(err, AdminError::PasswordVerify);
    // Applied new, then the old password was restored.
    assert_eq!(
        engine.applied(),
        vec!["new-canary-password".to_owned(), "old-canary".to_owned()]
    );
}

#[test]
fn sink_failure_rolls_back_to_the_old_password() {
    let engine = FakeEngine::new();
    let sink = StubSink { fail: true };
    let old = SecretString::from("old-canary");
    let err = reset_with(&engine, &sink, &old, new_password()).expect_err("sink fails");
    assert_eq!(err, AdminError::CredentialStore);
    assert_eq!(
        engine.applied(),
        vec!["new-canary-password".to_owned(), "old-canary".to_owned()]
    );
}

#[test]
fn a_rollback_that_also_fails_reports_inconsistent_state_without_secrets() {
    struct FailsOnRollback {
        applied: std::sync::Mutex<Vec<String>>,
    }
    impl PasswordEngine for FailsOnRollback {
        fn apply(&self, _connect_as: &SecretString, target: &SecretString) -> AdminResult<()> {
            self.applied
                .lock()
                .expect("lock")
                .push(target.expose().to_owned());
            if target.expose() == "old-canary" {
                Err(AdminError::Connect)
            } else {
                Ok(())
            }
        }
        fn verify(&self, _password: &SecretString) -> AdminResult<()> {
            Ok(())
        }
    }
    let engine = FailsOnRollback {
        applied: std::sync::Mutex::new(Vec::new()),
    };
    let sink = StubSink { fail: true };
    let old = SecretString::from("old-canary");
    let err = reset_with(&engine, &sink, &old, new_password()).expect_err("rollback fails");
    match &err {
        AdminError::RollbackFailed(state) => {
            assert!(state.engine_changed);
            assert!(state.sink_failed);
            assert!(state.rollback_failed);
        }
        other => panic!("expected RollbackFailed, got {other:?}"),
    }
    let surfaces = format!("{err:?} {err} {}", err.user_message());
    assert!(!surfaces.contains("new-canary-password"));
    assert!(!surfaces.contains("old-canary"));
}
