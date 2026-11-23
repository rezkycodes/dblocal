//! Deterministic open-in-client behaviour for [`super::MockService`] (`F-OPN`).
//!
//! Detection returns a fixed cast so a GUI render is reproducible. Opening builds
//! a real [`plan_open`] plan from the mock's own target, exactly as the real
//! service does, and never launches anything.

use std::path::PathBuf;
use std::sync::Arc;

use crate::clients::{
    ClientKind, DetectedClient, LaunchPlan, OpenTarget, OsKind, TerminalChoice, TerminalKind,
    plan_open,
};
use crate::model::InstanceId;
use crate::service::{ServiceResult, UserFacingError};

use super::{Inner, lock, not_found};

/// The clients the mock pretends are installed, in menu order.
///
/// Detection is deterministic and ignores `refresh`, so a GUI render is stable
/// across calls.
pub(crate) fn detect(
    inner: &Arc<std::sync::Mutex<Inner>>,
    refresh: bool,
) -> ServiceResult<Vec<DetectedClient>> {
    let _ = refresh;
    // `set_no_clients(true)` makes the menu show "Salin URI" instead of a list
    // (`F-OPN-01` AC2), so the demo build and the GUI tests can render that
    // state without a machine where nothing is installed.
    if lock(inner).no_clients {
        return Ok(Vec::new());
    }
    Ok(DetectedClients::default_detected())
}

/// The deterministic cast of "installed" clients.
struct DetectedClients;

impl DetectedClients {
    fn default_detected() -> Vec<DetectedClient> {
        vec![
            DetectedClient::new(
                ClientKind::Psql,
                PathBuf::from("/usr/bin/psql"),
                ClientKind::Psql.supported_engines(),
            ),
            DetectedClient::new(
                ClientKind::TablePlus,
                PathBuf::from("/usr/bin/tableplus"),
                ClientKind::TablePlus.supported_engines(),
            ),
        ]
    }
}

pub(crate) fn open(
    inner: &Arc<std::sync::Mutex<Inner>>,
    id: &InstanceId,
    client: ClientKind,
    _dry_run: bool,
) -> ServiceResult<LaunchPlan> {
    let (engine, port, superuser, name) = {
        let g = lock(inner);
        match g.instances.get(id) {
            Some(instance) => (
                instance.engine(),
                instance.port().get(),
                instance.superuser().to_owned(),
                instance.name().as_str().to_owned(),
            ),
            None => return Err(UserFacingError::from(not_found("instance", id))),
        }
    };
    if lock(inner).no_clients {
        return Err(UserFacingError::new(
            crate::clients::ClientError::ClientNotFound {
                name: client.as_str().to_owned(),
            }
            .code(),
            "Client tidak ditemukan.",
        ));
    }
    let found = DetectedClients::default_detected()
        .into_iter()
        .find(|candidate| candidate.kind() == client)
        .ok_or_else(|| {
            UserFacingError::new(
                crate::clients::ClientError::ClientNotFound {
                    name: client.as_str().to_owned(),
                }
                .code(),
                "Client tidak ditemukan.",
            )
        })?;
    let target = OpenTarget::new(
        engine,
        "127.0.0.1",
        port,
        superuser,
        crate::consts::DEFAULT_DATABASE,
        name,
    )
    .map_err(UserFacingError::from)?;
    let terminal = TerminalChoice::new("/usr/bin/xterm", TerminalKind::Xterm);
    plan_open(&found, &target, None, OsKind::Linux, &terminal).map_err(UserFacingError::from)
}
