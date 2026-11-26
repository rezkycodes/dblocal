//! Decoding of SQLite rows into domain values.
//!
//! Every stored string is re-validated through the domain constructors, so a
//! corrupted row surfaces as [`DblError::Storage`] instead of a bogus value.

use std::path::PathBuf;

use crate::DblError;
use crate::model::{
    ArchivedData, AuthMode, EngineKey, Instance, InstanceId, InstanceName, InstanceSpec,
    InstanceStatus, Port, Runtime, RuntimeState,
};
use crate::platform::Arch;

use super::time::{decode, decode_opt};

/// Columns of `instances`, in select order.
pub(crate) const INSTANCE_COLUMNS: &str = "id, name, engine, version, port, extra_port, status, auth, superuser, \
     pid, process_started_at_secs, process_started_at_nanos, process_exe, \
     autostart, created_at_secs, created_at_nanos";

/// Columns of `runtimes`, in select order.
pub(crate) const RUNTIME_COLUMNS: &str = "id, engine, version, arch, path, size_bytes, state, \
     installed_at_secs, installed_at_nanos";

/// Columns of `archives`, in select order.
pub(crate) const ARCHIVE_COLUMNS: &str = "id, instance_name, engine, version, path, size_bytes, \
     archived_at_secs, archived_at_nanos";

/// Decodes one `instances` row.
pub(crate) fn instance(row: &rusqlite::Row<'_>) -> rusqlite::Result<Instance> {
    let id: String = row.get(0)?;
    let name: String = row.get(1)?;
    let engine: String = row.get(2)?;
    let version: String = row.get(3)?;
    let port: i64 = row.get(4)?;
    let extra_port: Option<i64> = row.get(5)?;
    let status: String = row.get(6)?;
    let auth: String = row.get(7)?;
    let superuser: String = row.get(8)?;
    let pid: Option<i64> = row.get(9)?;
    let started_secs: Option<i64> = row.get(10)?;
    let started_nanos: Option<i64> = row.get(11)?;
    let exe: Option<String> = row.get(12)?;
    let autostart: i64 = row.get(13)?;
    let created_secs: i64 = row.get(14)?;
    let created_nanos: i64 = row.get(15)?;

    let built = build_instance(InstanceRow {
        id,
        name,
        engine,
        version,
        port,
        extra_port,
        status,
        auth,
        superuser,
        pid,
        started_secs,
        started_nanos,
        exe,
        autostart,
        created_secs,
        created_nanos,
    });
    built.map_err(row_error)
}

struct InstanceRow {
    id: String,
    name: String,
    engine: String,
    version: String,
    port: i64,
    extra_port: Option<i64>,
    status: String,
    auth: String,
    superuser: String,
    pid: Option<i64>,
    started_secs: Option<i64>,
    started_nanos: Option<i64>,
    exe: Option<String>,
    autostart: i64,
    created_secs: i64,
    created_nanos: i64,
}

fn build_instance(row: InstanceRow) -> crate::Result<Instance> {
    let id = InstanceId::parse(row.id)?;
    let name = InstanceName::new(row.name)?;
    let engine = row.engine.parse::<EngineKey>()?;
    let port_number = u16::try_from(row.port).map_err(|_| out_of_range("port"))?;
    let port = Port::new(port_number)?;
    let extra_port = match row.extra_port {
        Some(value) => {
            let num = u16::try_from(value).map_err(|_| out_of_range("extra_port"))?;
            Some(Port::new(num)?)
        }
        None => None,
    };
    let status = status_from_str(&row.status)?;
    let auth = row.auth.parse::<AuthMode>()?;
    let created_at = decode(row.created_secs, nanos(row.created_nanos)?)?;

    let started = match row.started_nanos {
        Some(value) => Some(nanos(value)?),
        None => None,
    };
    let process_started_at = decode_opt(row.started_secs, started)?;
    let pid = match row.pid {
        Some(value) => Some(u32::try_from(value).map_err(|_| out_of_range("pid"))?),
        None => None,
    };

    let spec = InstanceSpec::new(name, engine, row.version, Some(port), auth)?
        .with_autostart(row.autostart != 0)
        .with_extra_port(extra_port);
    let mut instance = Instance::new(&spec, id, status, created_at);
    if engine == EngineKey::Rustfs {
        if row.superuser.trim().is_empty() || row.superuser == "rustfsadmin" {
            return Err(DblError::Storage {
                reason: "stored RustFS access key is empty or default".to_owned(),
            });
        }
        instance = instance.with_superuser(row.superuser);
    } else if instance.superuser() != row.superuser {
        return Err(DblError::Storage {
            reason: format!(
                "stored superuser {:?} does not match engine default {:?}",
                row.superuser,
                instance.superuser()
            ),
        });
    }
    if let (Some(pid), Some(started), Some(exe)) = (pid, process_started_at, row.exe) {
        instance = instance.with_process(pid, started, PathBuf::from(exe));
    }
    Ok(instance)
}

/// Decodes one `runtimes` row.
pub(crate) fn runtime(row: &rusqlite::Row<'_>) -> rusqlite::Result<Runtime> {
    let id: String = row.get(0)?;
    let engine: String = row.get(1)?;
    let version: String = row.get(2)?;
    let arch: String = row.get(3)?;
    let path: String = row.get(4)?;
    let size: i64 = row.get(5)?;
    let state: String = row.get(6)?;
    let secs: i64 = row.get(7)?;
    let nanos_value: i64 = row.get(8)?;

    let built = (|| {
        Ok(Runtime::new(
            id,
            engine.parse::<EngineKey>()?,
            version,
            arch.parse::<Arch>()?,
            PathBuf::from(path),
            u64::try_from(size).map_err(|_| out_of_range("size_bytes"))?,
            decode(secs, nanos(nanos_value)?)?,
            state.parse::<RuntimeState>()?,
        ))
    })();
    built.map_err(row_error)
}

/// Decodes one `archives` row.
pub(crate) fn archive(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArchivedData> {
    let id: String = row.get(0)?;
    let name: String = row.get(1)?;
    let engine: String = row.get(2)?;
    let version: String = row.get(3)?;
    let path: String = row.get(4)?;
    let size: i64 = row.get(5)?;
    let secs: i64 = row.get(6)?;
    let nanos_value: i64 = row.get(7)?;

    let built = (|| {
        Ok(ArchivedData::new(
            id,
            InstanceName::new(name)?,
            engine.parse::<EngineKey>()?,
            version,
            PathBuf::from(path),
            u64::try_from(size).map_err(|_| out_of_range("size_bytes"))?,
            decode(secs, nanos(nanos_value)?)?,
        ))
    })();
    built.map_err(row_error)
}

/// Maps the stored status tag back to [`InstanceStatus`].
///
/// `InstanceStatus` has no `FromStr` in the contract, so the mapping lives here
/// and a test keeps it in sync with `InstanceStatus::as_str`.
pub(crate) fn status_from_str(value: &str) -> crate::Result<InstanceStatus> {
    let status = match value {
        "draft" => InstanceStatus::Draft,
        "provisioning" => InstanceStatus::Provisioning,
        "initializing" => InstanceStatus::Initializing,
        "stopped" => InstanceStatus::Stopped,
        "starting" => InstanceStatus::Starting,
        "running" => InstanceStatus::Running,
        "stopping" => InstanceStatus::Stopping,
        "failed" => InstanceStatus::Failed,
        "deleting" => InstanceStatus::Deleting,
        other => {
            return Err(DblError::Storage {
                reason: format!("unknown instance status in database: {other}"),
            });
        }
    };
    Ok(status)
}

fn nanos(value: i64) -> crate::Result<u32> {
    u32::try_from(value).map_err(|_| out_of_range("nanoseconds"))
}

fn out_of_range(field: &'static str) -> DblError {
    DblError::Storage {
        reason: format!("stored {field} is outside its valid range"),
    }
}

fn row_error(err: DblError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            err.to_string(),
        )),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_tags_round_trip_with_the_model() {
        for status in [
            InstanceStatus::Draft,
            InstanceStatus::Provisioning,
            InstanceStatus::Initializing,
            InstanceStatus::Stopped,
            InstanceStatus::Starting,
            InstanceStatus::Running,
            InstanceStatus::Stopping,
            InstanceStatus::Failed,
            InstanceStatus::Deleting,
        ] {
            assert_eq!(
                status_from_str(status.as_str()).ok(),
                Some(status),
                "tag drift for {status:?}"
            );
        }
        assert!(status_from_str("paused").is_err());
    }
}
