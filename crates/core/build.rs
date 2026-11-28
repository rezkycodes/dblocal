//! Embeds the toolchain catalog into `dblocal-core` at build time.
//!
//! `catalog/toolchains-v1.json` is generated and owned by the catalog part, so it
//! may not exist while the core part is built on its own. The embedded snapshot is
//! therefore assembled in `OUT_DIR`: the real catalog file when it is present, and
//! an empty but valid schema-1 document otherwise. That keeps a standalone core
//! build green without editing a file another worker owns, while a release build
//! that has the generated file still bundles exactly that file (mirroring how
//! `catalog/engines-v3.json` is embedded).

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

/// Valid, empty catalog used when the generated file is absent.
const FALLBACK: &str =
    "{\n  \"schema\": 1,\n  \"generated_at\": \"1970-01-01T00:00:00Z\",\n  \"tools\": []\n}\n";

fn main() -> ExitCode {
    let Ok(manifest) = env::var("CARGO_MANIFEST_DIR") else {
        eprintln!("build.rs: CARGO_MANIFEST_DIR is not set");
        return ExitCode::FAILURE;
    };
    let Ok(out_dir) = env::var("OUT_DIR") else {
        eprintln!("build.rs: OUT_DIR is not set");
        return ExitCode::FAILURE;
    };

    let source = PathBuf::from(&manifest)
        .join("..")
        .join("..")
        .join("catalog")
        .join("toolchains-v1.json");
    let target = PathBuf::from(&out_dir).join("toolchains-v1.json");
    let profile = env::var("PROFILE").unwrap_or_default();
    let allow_empty = env::var("DBLOCAL_ALLOW_EMPTY_TOOLCHAIN_CATALOG")
        .map(|v| matches!(v.trim(), "1" | "true" | "TRUE"))
        .unwrap_or(false);

    // Rebuild when the generated catalog appears, changes, disappears, or the
    // override env var changes.
    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-env-changed=DBLOCAL_ALLOW_EMPTY_TOOLCHAIN_CATALOG");

    let bytes = match fs::read(&source) {
        Ok(bytes) => bytes,
        Err(_) if profile == "release" && !allow_empty => {
            eprintln!(
                "build.rs: {} is missing; refusing to ship an empty toolchain catalog in a release build.\n\
                 Generate it with `cargo xtask toolchains-catalog --output catalog/toolchains-v1.json`,\n\
                 or set DBLOCAL_ALLOW_EMPTY_TOOLCHAIN_CATALOG=1 to opt into an empty fallback.",
                source.display()
            );
            return ExitCode::FAILURE;
        }
        Err(_) => FALLBACK.as_bytes().to_vec(),
    };
    match fs::write(&target, bytes) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("build.rs: could not write {}: {err}", target.display());
            ExitCode::FAILURE
        }
    }
}
