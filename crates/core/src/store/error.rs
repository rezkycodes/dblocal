//! Translation of `rusqlite` failures into the crate's typed errors.

use crate::DblError;

/// Uniqueness rule a constraint violation hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Constraint {
    /// `instances.name`: unique, case-insensitive.
    InstanceName,
    /// `instances.port`: globally unique.
    InstancePort,
    /// `runtimes (engine, version, arch)`.
    RuntimeKey,
    /// A primary key of `instances`, `runtimes` or `archives`.
    PrimaryKey,
}

/// Maps any SQLite failure to [`DblError::Storage`] (`DBL-3003`).
pub(crate) fn map_sql(err: rusqlite::Error) -> DblError {
    DblError::Storage {
        reason: err.to_string(),
    }
}

/// Reports which uniqueness rule rejected a write, if any.
pub(crate) fn constraint_target(err: &rusqlite::Error) -> Option<Constraint> {
    let rusqlite::Error::SqliteFailure(code, message) = err else {
        return None;
    };
    if code.code != rusqlite::ErrorCode::ConstraintViolation {
        return None;
    }
    let text = message.as_deref().unwrap_or_default();
    if text.contains("instances.name") {
        Some(Constraint::InstanceName)
    } else if text.contains("instances.port") || text.contains("instances.extra_port") {
        Some(Constraint::InstancePort)
    } else if text.contains("runtimes.engine")
        || text.contains("runtimes.version")
        || text.contains("runtimes.arch")
    {
        Some(Constraint::RuntimeKey)
    } else if text.contains("instances.id")
        || text.contains("runtimes.id")
        || text.contains("archives.id")
    {
        Some(Constraint::PrimaryKey)
    } else {
        None
    }
}

/// Rejects a write that matched no row.
pub(crate) fn ensure_affected(
    affected: usize,
    kind: &'static str,
    name: &str,
) -> crate::Result<()> {
    if affected == 0 {
        Err(DblError::NotFound {
            kind,
            name: name.to_owned(),
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_errors_become_storage_errors() {
        let mapped = map_sql(rusqlite::Error::InvalidQuery);
        assert!(matches!(mapped, DblError::Storage { .. }));
        assert_eq!(mapped.code(), crate::error::CODE_STORAGE);
    }

    #[test]
    fn non_constraint_errors_have_no_target() {
        assert_eq!(constraint_target(&rusqlite::Error::InvalidQuery), None);
    }

    #[test]
    fn missing_row_is_not_found() {
        assert!(matches!(
            ensure_affected(0, "instance", "app-dev"),
            Err(DblError::NotFound {
                kind: "instance",
                ..
            })
        ));
        assert!(ensure_affected(1, "instance", "app-dev").is_ok());
    }
}
