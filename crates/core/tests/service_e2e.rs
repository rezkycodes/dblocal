//! W2-C — real-engine end-to-end proofs through the public `DblService` surface.
//!
//! Every test is `#[ignore]`d: it needs network access, a cached engine binary
//! and several hundred megabytes of disk. Run them with
//!
//! ```sh
//! export CARGO_TARGET_DIR="$HOME/.cache/dblocal-target"
//! DBLOCAL_E2E=1 cargo xtask e2e
//! ```
//!
//! or one at a time with
//!
//! ```sh
//! DBLOCAL_E2E=1 cargo test -p dblocal-core --test service_e2e -- --ignored --nocapture <name>
//! ```
//!
//! Design notes and the recorded runs live in `docs/architecture/e2e.md`.
//!
//! Everything the service does is production code. The only substitutions are:
//!
//! - the home directory is a temp dir (`HOME` and `DBLOCAL_HOME` both point into
//!   it), so the developer's `~/.dblocal` and keychain are never touched;
//! - the credential backend is `FileOnlySecrets`, which is the production `0600`
//!   file backend without the keychain attempt (`F-CRD-01` AC2);
//! - tests 3-8 copy an already-verified runtime tree from a process-external
//!   cache instead of re-downloading 40 MB, which is the documented "runtime
//!   already installed" path (`F-RTM-03` AC3). Tests 1 and 2 use the real HTTPS
//!   downloader and seed that cache.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// The temp-home guard sets `HOME`/`DBLOCAL_HOME` process-wide for the lifetime of
// one test. Rust 2024 made that `unsafe` because concurrent readers would race;
// the tests serialise every env-mutating body behind one mutex, so the
// precondition holds.
#![allow(unsafe_code)]

#[path = "support/mod.rs"]
mod support;

use std::time::Duration;

/// Download + verify + extract + initialise can take a while on a slow link.
pub const CREATE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Spawn to healthy.
pub const START_TIMEOUT: Duration = Duration::from_secs(120);
/// Stop, including the forced-kill escalation path.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(90);
/// Delete, including stopping an engine first.
pub const DELETE_TIMEOUT: Duration = Duration::from_secs(180);
