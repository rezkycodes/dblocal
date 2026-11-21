//! `dblocal-cli` — command handlers, formatting and exit-code contract.
//!
//! The binary entry point lives in `main.rs`; everything here is a library so
//! tests can drive commands against an injected `DblService`.

pub mod cli;
pub mod commands;
pub mod consts;
pub mod messages;
pub mod output;
