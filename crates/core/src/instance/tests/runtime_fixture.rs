//! Plants runtime trees that mirror what each engine's archive really unpacks.
//!
//! `create` takes its "already installed" path when every expected binary is
//! present (`F-RTM-03` AC3). The fixture must therefore write the real layout —
//! `bin/<name>` for most engines, a root-level `rustfs` for RustFS — never a
//! bare `bin/` directory, which is exactly the assumption this task fixed.

use crate::model::EngineKey;
use crate::paths::DblPaths;

/// The version every harness fixture uses.
pub const FIXTURE_VERSION: &str = "18.7";
/// The architecture every harness fixture uses.
pub const FIXTURE_ARCH: crate::platform::Arch = crate::platform::Arch::X86_64;

/// The runtime directory a harness fixture plants for `engine` at
/// [`FIXTURE_VERSION`].
pub fn fixture_runtime_dir(paths: &DblPaths, engine: EngineKey) -> std::path::PathBuf {
    paths
        .runtime_dir(engine, FIXTURE_VERSION, FIXTURE_ARCH)
        .expect("runtime dir")
}

/// Writes the complete real layout for `engine` at [`FIXTURE_VERSION`].
pub fn plant_runtime(paths: &DblPaths, engine: EngineKey) {
    plant_runtime_at(&fixture_runtime_dir(paths, engine), engine);
}

/// Writes the complete real layout for `engine` under an explicit `dir`.
pub fn plant_runtime_at(dir: &std::path::Path, engine: EngineKey) {
    for rel in crate::engine::layout::expected_binaries(engine) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().expect("binary parent")).expect("runtime dir");
        std::fs::write(&path, b"#!/bin/sh\nexit 0\n").expect("runtime binary");
    }
}
