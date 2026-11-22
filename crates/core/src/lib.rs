//! Core domain, engine contracts, and service API for DBLocal.

pub mod admin;
pub mod autostart;
pub mod catalog;
pub mod clients;
pub mod consts;
pub mod download;
pub mod engine;
pub mod error;
pub mod fsutil;
pub mod instance;
pub mod model;
pub mod paths;
pub mod platform;
pub mod ports;
pub mod process;
pub mod secret;
pub mod secrets;
pub mod service;
pub mod store;
pub mod strings;
pub mod update;

pub use error::{DblError, Result};
pub use instance::{
    LocalService, ProcessTerminator, RuntimeSource, SecretAccess, ServiceOptions, Terminator,
};
