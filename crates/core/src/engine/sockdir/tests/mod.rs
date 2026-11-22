//! Tests for the shared socket-fallback and private-directory module.
//!
//! Environment is never mutated: every function takes `XDG_RUNTIME_DIR` and the
//! uid as arguments, so the tests drive the pure functions and inject candidate
//! lists directly. Helpers live in `prelude`, shared by every group.

mod prelude;

mod cleanup;
mod limits;
mod prepare;
mod resolve;
mod verify;
