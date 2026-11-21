//! Database and user administration.
//!
//! This module provides a pure SQL-building layer (`sql`) and engine-facing
//! execution (`postgres`). It never stores passwords, never interpolates raw
//! identifiers into SQL, and never touches the GUI/service wiring.

pub mod postgres;
pub mod sql;
