//! stdout/stderr writers and formatting helpers.
//!
//! stdout is data; stderr is diagnostics/progress. Nothing here prints secrets
//! except the explicit URI render path.

use std::io::{IsTerminal, Write};
use std::time::Duration;

use dblocal_core::DblError;
use dblocal_core::model::InstanceStatus;
use dblocal_core::service::{InstanceView, OperationId, ServiceEvent, UserFacingError};

use crate::cli::{EXIT_BAD_ARGS, EXIT_ENGINE_FAILED, EXIT_GENERAL, EXIT_NOT_FOUND};

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

pub fn code_for(err: &DblError) -> u8 {
    match err {
        DblError::NotFound { .. } => EXIT_NOT_FOUND,
        DblError::Engine { .. } => EXIT_ENGINE_FAILED,
        DblError::InvalidInput { .. } | DblError::InvalidTransition { .. } => EXIT_BAD_ARGS,
        DblError::Cancelled { .. } => EXIT_GENERAL,
        _ => EXIT_GENERAL,
    }
}

/// Table for `dblocal list`. Columns: NAME, ENGINE, VERSION, PORT, STATUS.
pub fn format_list_table(instances: &[InstanceView], color: bool) -> String {
    let headers = ["NAME", "ENGINE", "VERSION", "PORT", "STATUS"];
    let mut rows: Vec<[String; 5]> = instances
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
    for row in &mut rows {
        if !color {
            // nothing to strip; labels are plain
        }
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

pub fn format_list_json(instances: &[InstanceView]) -> Result<String, serde_json::Error> {
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
    serde_json::to_string(&items)
}

pub fn is_tty_stdout() -> bool {
    std::io::stdout().is_terminal()
}

pub fn is_tty_stderr() -> bool {
    std::io::stderr().is_terminal()
}

pub fn wait_operation_rx(
    rx: &mut tokio::sync::broadcast::Receiver<ServiceEvent>,
    op: &OperationId,
    err_out: &mut dyn Write,
    show_progress: bool,
) -> Result<(), UserFacingError> {
    loop {
        match rx.blocking_recv() {
            Ok(ServiceEvent::OperationProgress { id, stage }) if &id == op => {
                if show_progress {
                    stderr_line(err_out, &format!("... {}", stage.as_str()));
                }
            }
            Ok(ServiceEvent::DownloadProgress { id, progress }) if &id == op => {
                if show_progress {
                    let pct = (progress.fraction() * 100.0).round() as u64;
                    stderr_line(
                        err_out,
                        &format!(
                            "... unduhan {pct}% ({}/{})",
                            progress.bytes_done, progress.bytes_total
                        ),
                    );
                }
            }
            Ok(ServiceEvent::OperationFinished { id, result }) if &id == op => return result,
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                return Err(UserFacingError::from(&DblError::Storage {
                    reason: "event stream closed".to_owned(),
                }));
            }
        }
    }
}

/// Waits for an instance to reach a desired status or for an operation to fail.
/// Also observes the operation finish; `ok_statuses` determines success on finish.
pub struct StatusWait<'a> {
    pub op: &'a OperationId,
    pub instance_id: &'a dblocal_core::model::InstanceId,
    pub ok_statuses: &'a [InstanceStatus],
    pub timeout: Duration,
    pub show_progress: bool,
}

pub fn wait_for_status_rx(
    rx: &mut tokio::sync::broadcast::Receiver<ServiceEvent>,
    service: &dyn dblocal_core::service::DblService,
    wait: StatusWait<'_>,
    err_out: &mut dyn Write,
) -> Result<InstanceStatus, UserFacingError> {
    let StatusWait {
        op,
        instance_id,
        ok_statuses,
        timeout,
        show_progress,
    } = wait;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|e| {
            UserFacingError::from(&DblError::Engine {
                reason: format!("runtime: {e}"),
            })
        })?;
    rt.block_on(async {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut last_status: Option<InstanceStatus> = None;
        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return Err(UserFacingError::from(&DblError::Engine {
                    reason: "timeout".to_owned(),
                }));
            }
            let remaining = deadline - now;
            let recv = tokio::time::timeout(remaining, rx.recv()).await;
            match recv {
                Ok(Ok(ServiceEvent::OperationProgress { id, stage })) if &id == op => {
                    if show_progress {
                        stderr_line(err_out, &format!("... {}", stage.as_str()));
                    }
                }
                Ok(Ok(ServiceEvent::DownloadProgress { id, progress })) if &id == op => {
                    if show_progress {
                        let pct = (progress.fraction() * 100.0).round() as u64;
                        stderr_line(err_out, &format!("... unduhan {pct}%"));
                    }
                }
                Ok(Ok(ServiceEvent::InstanceChanged { view })) if &view.id == instance_id => {
                    last_status = Some(view.status);
                    if ok_statuses.contains(&view.status) {
                        return Ok(view.status);
                    }
                    if view.status == InstanceStatus::Failed {
                        if let Some(failure) = view.failure {
                            let mut err = UserFacingError::new(failure.code, failure.message);
                            err.diagnosis = Some(failure.diagnosis);
                            return Err(err);
                        }
                        return Err(UserFacingError::from(&DblError::Engine {
                            reason: "start failed".to_owned(),
                        }));
                    }
                }
                Ok(Ok(ServiceEvent::OperationFinished { id, result })) if &id == op => match result
                {
                    Ok(()) => {
                        if let Ok(views) = service.list_instances()
                            && let Some(v) = views.into_iter().find(|v| &v.id == instance_id)
                        {
                            if ok_statuses.contains(&v.status) {
                                return Ok(v.status);
                            }
                            last_status = Some(v.status);
                        }
                        if let Some(status) = last_status
                            && ok_statuses.contains(&status)
                        {
                            return Ok(status);
                        }
                        return Ok(last_status.unwrap_or(InstanceStatus::Running));
                    }
                    Err(err) => return Err(err),
                },
                Ok(Ok(_)) => {}
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                    return Err(UserFacingError::from(&DblError::Storage {
                        reason: "event stream closed".to_owned(),
                    }));
                }
                Err(_) => {
                    return Err(UserFacingError::from(&DblError::Engine {
                        reason: "timeout".to_owned(),
                    }));
                }
            }
        }
    })
}

pub fn is_interactive_stdin() -> bool {
    std::io::stdin().is_terminal()
}
