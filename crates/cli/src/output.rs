//! stdout/stderr writers and formatting helpers.
//!
//! stdout is data; stderr is diagnostics/progress. Nothing here prints secrets
//! except the explicit URI render path.

use std::io::{IsTerminal, Write};
use std::time::Duration;

use dblocal_core::DblError;
use dblocal_core::model::{InstanceId, InstanceStatus};
use dblocal_core::service::{DblService, InstanceView, OperationId, ServiceEvent, UserFacingError};

use crate::cli::{EXIT_BAD_ARGS, EXIT_ENGINE_FAILED, EXIT_GENERAL, EXIT_NOT_FOUND};
use crate::messages;

/// Writes a line to stdout. Errors are ignored because a broken pipe during
/// piping (e.g. `dblocal uri | ...`) should not panic.
pub fn stdout_line(out: &mut dyn Write, line: &str) {
    let _ = writeln!(out, "{line}");
}

pub fn stderr_line(err: &mut dyn Write, line: &str) {
    let _ = writeln!(err, "{line}");
}

pub fn report_error(err_out: &mut dyn Write, err: &DblError) -> u8 {
    stderr_line(err_out, &err.diagnostic());
    code_for(err)
}

/// Reports a [`UserFacingError`] that already carries its module-local code.
///
/// The admin, update and client calls keep their own `DBL-8xxx` ranges, so the
/// CLI prints the code the module chose rather than re-deriving one from
/// `DblError`.
pub fn report_facing(err_out: &mut dyn Write, err: &UserFacingError) -> u8 {
    stderr_line(err_out, &format!("[{}] {}", err.code, err.message));
    code_for_facing(err)
}

/// Exit code for a module-local error, keyed on its code.
///
/// `DBL-81xx` (admin) validation failures are bad arguments; `DBL-8301` (a
/// client disappeared) maps to not-found; an update that cannot apply or an
/// engine-backed admin failure is a general error.
pub fn code_for_facing(err: &UserFacingError) -> u8 {
    let number = err
        .code
        .strip_prefix("DBL-")
        .and_then(|digits| digits.parse::<u16>().ok());
    match number {
        Some(8101..=8111) => EXIT_BAD_ARGS,
        Some(8301) => EXIT_NOT_FOUND,
        Some(8303) => EXIT_BAD_ARGS,
        Some(8300) => EXIT_BAD_ARGS,
        _ => EXIT_GENERAL,
    }
}

/// One JSON array of `{name, owner, size_bytes}` for `db list --json`.
pub fn format_databases_json(databases: &[dblocal_core::service::DatabaseInfo]) -> String {
    let items: Vec<serde_json::Value> = databases
        .iter()
        .map(|db| {
            serde_json::json!({
                "name": db.name(),
                "owner": db.owner(),
                "size_bytes": db.size_bytes(),
            })
        })
        .collect();
    serde_json::Value::Array(items).to_string()
}

/// Aligned table of databases: NAME, OWNER, SIZE.
///
/// Plain text on every stream; `db list` has no colour option, matching `list`.
pub fn format_database_table(databases: &[dblocal_core::service::DatabaseInfo]) -> String {
    let headers = ["NAME", "OWNER", "SIZE"];
    let rows: Vec<[String; 3]> = databases
        .iter()
        .map(|db| {
            [
                db.name().to_owned(),
                db.owner().to_owned(),
                human_size(db.size_bytes()),
            ]
        })
        .collect();
    let mut widths = headers.map(str::len);
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.len());
        }
    }
    let mut out = String::new();
    push_row3(&mut out, &headers.map(str::to_owned), &widths);
    for row in &rows {
        push_row3(&mut out, row, &widths);
    }
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

fn push_row3(out: &mut String, row: &[String; 3], widths: &[usize; 3]) {
    for (i, cell) in row.iter().enumerate() {
        if i > 0 {
            out.push_str("  ");
        }
        out.push_str(cell);
        if i < row.len() - 1 {
            for _ in 0..widths[i].saturating_sub(cell.len()) {
                out.push(' ');
            }
        }
    }
    out.push('\n');
}

/// A byte count as `MB`/`KB`/`B`, rounded, for the size column.
fn human_size(bytes: i64) -> String {
    let bytes = bytes.max(0) as u64;
    const MIB: u64 = 1024 * 1024;
    const KIB: u64 = 1024;
    if bytes >= MIB {
        format!("{} MB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{} KB", bytes / KIB)
    } else {
        format!("{bytes} B")
    }
}

/// One JSON line describing an update check, for `update check --json`.
pub fn format_update_json(status: &dblocal_core::service::UpdateStatus) -> String {
    let available = status.available.as_ref().map(|available| {
        serde_json::json!({
            "version": available.version,
            "tag": available.tag,
            "asset_name": available.asset_name,
            "size_bytes": available.size_bytes,
        })
    });
    serde_json::json!({
        "current": status.current,
        "available": available,
        "can_self_update": status.install_kind.can_self_update(),
        "package_manager_hint": status.package_manager_hint,
    })
    .to_string()
}

/// Renders a launch plan without any secret.
///
/// `--dry-run` prints this: the program, argv, and (when present) the URL and the
/// secure launcher's presence. The password never appears because it only lives
/// inside the launcher script the spawner materialises.
pub fn format_launch_plan(plan: &dblocal_core::clients::LaunchPlan) -> String {
    let mut out = format!("program: {}", plan.program().display());
    if !plan.argv().is_empty() {
        out.push_str("\nargs:");
        for arg in plan.argv() {
            out.push(' ');
            out.push_str(arg);
        }
    }
    if let Some(url) = plan.url() {
        out.push_str("\nurl: ");
        out.push_str(url);
    }
    if plan.temp_file().is_some() {
        out.push_str("\nlauncher: private script (0600, deleted when run)");
    }
    out
}

pub fn code_for(err: &DblError) -> u8 {
    match err {
        DblError::NotFound { .. } => EXIT_NOT_FOUND,
        DblError::Engine { .. } => EXIT_ENGINE_FAILED,
        DblError::InvalidInput { .. }
        | DblError::InvalidTransition { .. }
        | DblError::AlreadyExists { .. }
        | DblError::PortInUse { .. }
        | DblError::Unsupported { .. } => EXIT_BAD_ARGS,
        _ => EXIT_GENERAL,
    }
}

/// Table for `dblocal list`. Columns: NAME, ENGINE, VERSION, PORT, STATUS.
///
/// The table is plain text on every stream: `list` has no colour path, so no
/// caller can be misled into thinking a `--color` flag exists.
pub fn format_list_table(instances: &[InstanceView]) -> String {
    let headers = ["NAME", "ENGINE", "VERSION", "PORT", "STATUS"];
    let rows: Vec<[String; 5]> = instances
        .iter()
        .map(|v| {
            [
                v.name.clone(),
                v.engine.as_str().to_owned(),
                v.version.clone(),
                v.port.get().to_string(),
                status_label(v.status).to_owned(),
            ]
        })
        .collect();
    let mut widths = headers.map(|h| h.len());
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.len());
        }
    }
    let mut out = String::new();
    push_row(&mut out, &headers.map(str::to_owned), &widths);
    for row in &rows {
        push_row(&mut out, row, &widths);
    }
    // remove trailing newline for caller control
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

fn push_row(out: &mut String, row: &[String; 5], widths: &[usize; 5]) {
    for (i, cell) in row.iter().enumerate() {
        if i > 0 {
            out.push_str("  ");
        }
        let pad = widths[i].saturating_sub(cell.len());
        out.push_str(cell);
        if i < row.len() - 1 {
            for _ in 0..pad {
                out.push(' ');
            }
        }
    }
    out.push('\n');
}

pub fn status_label(status: InstanceStatus) -> &'static str {
    match status {
        InstanceStatus::Draft => "draft",
        InstanceStatus::Provisioning => "provisioning",
        InstanceStatus::Initializing => "initializing",
        InstanceStatus::Stopped => "stopped",
        InstanceStatus::Starting => "starting",
        InstanceStatus::Running => "running",
        InstanceStatus::Stopping => "stopping",
        InstanceStatus::Failed => "failed",
        InstanceStatus::Deleting => "deleting",
        _ => "unknown",
    }
}

/// One JSON line: an array of objects with the stable English keys.
///
/// Infallible on purpose: every value is a string or an integer inside a
/// `serde_json::Value`, so serialization cannot fail. A `Result` here would be
/// a branch no input can reach, and the caller would carry a dead error path.
pub fn format_list_json(instances: &[InstanceView]) -> String {
    let items: Vec<serde_json::Value> = instances
        .iter()
        .map(|v| {
            serde_json::json!({
                "name": v.name,
                "engine": v.engine.as_str(),
                "version": v.version,
                "port": v.port.get(),
                "status": status_label(v.status),
            })
        })
        .collect();
    serde_json::Value::Array(items).to_string()
}

pub fn is_tty_stdout() -> bool {
    std::io::stdout().is_terminal()
}

pub fn is_tty_stderr() -> bool {
    std::io::stderr().is_terminal()
}

/// How to recognise that an operation a caller waits for is already over.
///
/// Only used when the stream dropped events ([`tokio::sync::broadcast::error::RecvError::Lagged`]):
/// the final event may be one of the dropped ones, so the store, not the stream,
/// has to answer.
#[derive(Debug, Clone, Copy)]
pub enum Completion<'a> {
    /// The instance is gone: a delete finished.
    Gone,
    /// The instance settled into one of these statuses: a create finished.
    StatusIn(&'a [InstanceStatus]),
}

/// Which instance an operation is about, so a dropped event can be re-checked.
///
/// A create's row is inserted by the background task, so its id is not known
/// before the operation starts; the name is, and it is unique.
#[derive(Debug, Clone, Copy)]
pub enum Target<'a> {
    /// The operation names a known instance id.
    Id(&'a InstanceId),
    /// The operation names an instance that may not be listed yet.
    Name(&'a str),
}

impl Target<'_> {
    fn find(&self, views: Vec<InstanceView>) -> Option<InstanceView> {
        match *self {
            Self::Id(id) => views.into_iter().find(|v| &v.id == id),
            Self::Name(name) => views.into_iter().find(|v| v.name == name),
        }
    }
}

pub fn wait_operation_rx(
    rx: &mut tokio::sync::broadcast::Receiver<ServiceEvent>,
    op: &OperationId,
    service: &dyn DblService,
    target: Target<'_>,
    completion: Completion<'_>,
    err_out: &mut dyn Write,
    show_progress: bool,
) -> Result<(), UserFacingError> {
    loop {
        match rx.blocking_recv() {
            Ok(ServiceEvent::OperationProgress { id, stage }) if &id == op => {
                // A stage change is a plain line on every terminal; only the
                // high-frequency download percentage is TTY-only noise.
                stderr_line(err_out, &messages::stage_line(stage.as_str()));
            }
            Ok(ServiceEvent::DownloadProgress { id, progress }) if &id == op => {
                if show_progress {
                    report_download(err_out, &progress);
                }
            }
            Ok(ServiceEvent::OperationFinished { id, result }) if &id == op => return result,
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                if let Some(result) = settled(service, target, completion) {
                    return result;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                return Err(UserFacingError::from(&DblError::Storage {
                    reason: "event stream closed".to_owned(),
                }));
            }
        }
    }
}

/// Reads the store and decides whether the operation is already over.
///
/// `None` means "still running, keep waiting". A read failure is also `None`:
/// a transient store error must not turn into a wrong verdict, and the next
/// event or tick will decide.
fn settled(
    service: &dyn DblService,
    target: Target<'_>,
    completion: Completion<'_>,
) -> Option<Result<(), UserFacingError>> {
    let view = target.find(service.list_instances().ok()?);
    match (completion, view) {
        (Completion::Gone, None) => Some(Ok(())),
        (Completion::Gone, Some(_)) => None,
        (Completion::StatusIn(_), Some(view)) if view.status == InstanceStatus::Failed => {
            Some(Err(failure_error(view.failure)))
        }
        (Completion::StatusIn(ok), Some(view)) if ok.contains(&view.status) => Some(Ok(())),
        (Completion::StatusIn(_), Some(_)) => None,
        // A create's row is inserted by its own task, so "not there yet" is
        // normal early on; keep waiting for the operation's final event.
        (Completion::StatusIn(_), None) => None,
    }
}

fn failure_error(failure: Option<dblocal_core::service::FailureInfo>) -> UserFacingError {
    match failure {
        Some(failure) => {
            let mut err = UserFacingError::new(failure.code, failure.message);
            err.diagnosis = Some(failure.diagnosis);
            err
        }
        None => UserFacingError::from(&DblError::Engine {
            reason: "operation failed".to_owned(),
        }),
    }
}

/// Waits for an instance to reach a desired status or for an operation to fail.
/// Also observes the operation finish; `ok_statuses` determines success on finish.
pub struct StatusWait<'a> {
    pub op: &'a OperationId,
    pub instance_id: &'a InstanceId,
    pub ok_statuses: &'a [InstanceStatus],
    pub timeout: Duration,
    pub show_progress: bool,
}

pub fn wait_for_status_rx(
    rx: &mut tokio::sync::broadcast::Receiver<ServiceEvent>,
    service: &dyn DblService,
    wait: StatusWait<'_>,
    err_out: &mut dyn Write,
) -> Result<InstanceStatus, UserFacingError> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|e| {
            // A CLI runtime that cannot start is a CLI failure, not an engine
            // one: the engine code would claim a database problem that does not
            // exist.
            UserFacingError::new(
                messages::CODE_RUNTIME,
                messages::runtime_message(&e.to_string()),
            )
        })?;
    rt.block_on(async {
        let deadline = tokio::time::Instant::now() + wait.timeout;
        let mut last_status: Option<InstanceStatus> = None;
        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return Err(timeout_error());
            }
            let remaining = deadline - now;
            let recv = tokio::time::timeout(remaining, rx.recv()).await;
            let event = match recv {
                Ok(event) => event,
                Err(_) => return Err(timeout_error()),
            };
            if let Some(done) = observe(event, service, &wait, &mut last_status, err_out) {
                return done;
            }
        }
    })
}

/// One download-progress line, TTY-only.
fn report_download(err_out: &mut dyn Write, progress: &dblocal_core::model::DownloadProgress) {
    let pct = (progress.fraction() * 100.0).round() as u64;
    stderr_line(
        err_out,
        &messages::download_progress(pct, progress.bytes_done, progress.bytes_total),
    );
}

fn timeout_error() -> UserFacingError {
    UserFacingError::new(messages::CODE_TIMEOUT, messages::ERR_TIMEOUT)
}

/// Handles one event; `Some` ends the wait.
fn observe(
    event: Result<ServiceEvent, tokio::sync::broadcast::error::RecvError>,
    service: &dyn DblService,
    wait: &StatusWait<'_>,
    last_status: &mut Option<InstanceStatus>,
    err_out: &mut dyn Write,
) -> Option<Result<InstanceStatus, UserFacingError>> {
    match event {
        Ok(ServiceEvent::OperationProgress { id, stage }) if &id == wait.op => {
            if wait.show_progress {
                stderr_line(err_out, &messages::stage_line(stage.as_str()));
            }
        }
        Ok(ServiceEvent::DownloadProgress { id, progress }) if &id == wait.op => {
            if wait.show_progress {
                report_download(err_out, &progress);
            }
        }
        Ok(ServiceEvent::InstanceChanged { view }) if &view.id == wait.instance_id => {
            *last_status = Some(view.status);
            return decide(view, wait.ok_statuses);
        }
        Ok(ServiceEvent::OperationFinished { id, result }) if &id == wait.op => {
            return Some(finish(
                result,
                service,
                wait.instance_id,
                wait.ok_statuses,
                *last_status,
            ));
        }
        Ok(_) => {}
        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
            if let Some(result) = reread_decides(service, wait, last_status) {
                return Some(result);
            }
        }
        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
            return Some(Err(UserFacingError::from(&DblError::Storage {
                reason: "event stream closed".to_owned(),
            })));
        }
    }
    None
}

/// Re-reads the store after a dropped event; `Some` when the wait is over.
///
/// The final event may be one of the dropped ones, so the store, not the
/// stream, is the authority.
fn reread_decides(
    service: &dyn DblService,
    wait: &StatusWait<'_>,
    last_status: &mut Option<InstanceStatus>,
) -> Option<Result<InstanceStatus, UserFacingError>> {
    match reread(service, wait.instance_id, wait.ok_statuses) {
        Some(Outcome::Status(status)) => {
            *last_status = Some(status);
            None
        }
        Some(Outcome::Done(result)) => Some(result),
        None => None,
    }
}

/// Verdict from one view: `Some` when the wait is over, `None` to keep waiting.
fn decide(
    view: InstanceView,
    ok_statuses: &[InstanceStatus],
) -> Option<Result<InstanceStatus, UserFacingError>> {
    if ok_statuses.contains(&view.status) {
        return Some(Ok(view.status));
    }
    if view.status == InstanceStatus::Failed {
        return Some(Err(failure_error(view.failure)));
    }
    None
}

/// What a store re-read says about the wait.
enum Outcome {
    /// The instance settled into a status that is not the goal and not a
    /// failure; remember it and keep waiting.
    Status(InstanceStatus),
    /// The wait is over.
    Done(Result<InstanceStatus, UserFacingError>),
}

fn reread(
    service: &dyn DblService,
    instance_id: &InstanceId,
    ok_statuses: &[InstanceStatus],
) -> Option<Outcome> {
    let views = service.list_instances().ok()?;
    match views.into_iter().find(|v| &v.id == instance_id) {
        Some(view) => match decide(view.clone(), ok_statuses) {
            Some(result) => Some(Outcome::Done(result)),
            None => Some(Outcome::Status(view.status)),
        },
        // Gone: `stopped` and then deleted. Nothing to wait for any more.
        None => Some(Outcome::Done(Err(UserFacingError::from(
            &DblError::NotFound {
                kind: "instance",
                name: instance_id.to_string(),
            },
        )))),
    }
}

/// Resolves an `OperationFinished` event into the wait's result.
fn finish(
    result: Result<(), UserFacingError>,
    service: &dyn DblService,
    instance_id: &InstanceId,
    ok_statuses: &[InstanceStatus],
    last_status: Option<InstanceStatus>,
) -> Result<InstanceStatus, UserFacingError> {
    match result {
        Ok(()) => {
            if let Ok(views) = service.list_instances()
                && let Some(v) = views.into_iter().find(|v| &v.id == instance_id)
            {
                if ok_statuses.contains(&v.status) {
                    return Ok(v.status);
                }
                if let Some(status) = last_status
                    && ok_statuses.contains(&status)
                {
                    return Ok(status);
                }
                return Ok(v.status);
            }
            if let Some(status) = last_status
                && ok_statuses.contains(&status)
            {
                return Ok(status);
            }
            Ok(last_status.unwrap_or(InstanceStatus::Running))
        }
        Err(err) => Err(err),
    }
}

pub fn is_interactive_stdin() -> bool {
    std::io::stdin().is_terminal()
}

#[cfg(test)]
mod tests;
