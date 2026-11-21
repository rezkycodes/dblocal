//! Open-in-client detection and launch planning (F-OPN-01, F-OPN-02).
//!
//! This module is pure core logic: it detects installed database clients,
//! builds a safe [`LaunchPlan`] for opening one against an instance, and runs
//! that plan through an injected [`Spawner`]. It never touches the GUI, never
//! embeds a password in argv/URL/command strings, and never builds a shell
//! command by concatenation.

mod detect;
mod error;
mod messages;
mod plan;
mod quote;
mod run;
mod terminal;

pub use detect::{ClientKind, DetectedClient, HostProbe, OpenMenu, detect_clients};
pub use error::{ClientError, ClientResult};
pub use plan::{
    LaunchEnv, LaunchPlan, OpenTarget, OsKind, TempFile, TerminalChoice, TerminalKind, plan_open,
};
pub use quote::shell_quote;
pub use run::{Spawner, SystemSpawner, run};
pub use terminal::resolve_terminal;

#[cfg(test)]
mod tests;
