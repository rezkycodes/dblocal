//! Behavioural tests for every command against the in-memory mock service.
//!
//! The mock is the real `DblService`, so a handler that works here works against
//! `LocalService` too. The tests are split by theme so a failure names the
//! behaviour it covers:
//!
//! - `inspect`  — `list`, `status`, `uri`, `logs`, formatters, the exit table
//! - `lifecycle` — `create`, `start`, `stop`, `delete`
//! - `races`    — the F-CLI-02 AC3 start race and the cross-process wait

#![cfg(test)]

mod inspect;
mod lifecycle;
mod races;

use dblocal_core::model::{
    AuthMode, EngineKey, InstanceId, InstanceName, InstanceSpec, InstanceStatus, Port,
};
use dblocal_core::service::mock::MockService;
use dblocal_core::service::{DblService, UriFormat};

use crate::cli::{Command, EXIT_BAD_ARGS, EXIT_GENERAL, EXIT_NOT_FOUND, EXIT_OK};
use crate::commands::{Ctx, dispatch};
use crate::output;

fn spec(name: &str, engine: EngineKey, version: &str, port: u16) -> InstanceSpec {
    InstanceSpec::new(
        InstanceName::new(name).expect("name"),
        engine,
        version,
        Some(Port::new(port).expect("port")),
        AuthMode::Password,
    )
    .expect("spec")
}

fn run_cmd(service: &dyn DblService, cmd: &Command, interactive: bool) -> (u8, String, String) {
    run_cmd_with(service, cmd, interactive, &|| None)
}

fn run_cmd_with(
    service: &dyn DblService,
    cmd: &Command,
    interactive: bool,
    confirm: &dyn Fn() -> Option<String>,
) -> (u8, String, String) {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut ctx = Ctx {
        service,
        out: &mut out,
        err: &mut err,
        is_interactive: interactive,
        stdout_is_tty: false,
        stderr_is_tty: false,
        confirm,
    };
    let code = dispatch(&mut ctx, cmd);
    (
        code,
        String::from_utf8_lossy(&out).to_string(),
        String::from_utf8_lossy(&err).to_string(),
    )
}

fn assert_no_secret(code: u8, out: &str, err: &str, secret: &str) {
    assert!(
        !out.contains(secret),
        "stdout leaked secret on exit {code}: {out}"
    );
    assert!(
        !err.contains(secret),
        "stderr leaked secret on exit {code}: {err}"
    );
}
