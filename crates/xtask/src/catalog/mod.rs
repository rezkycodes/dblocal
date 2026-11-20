//! `cargo xtask catalog ...` commands.

mod build;
mod sources;
mod validate;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum CatalogCommand {
    /// Rebuilds the bundled engine catalog.
    Build {
        /// Where to write the generated JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Validates a catalog JSON file offline.
    Validate {
        /// Path to validate (defaults to `catalog/engines.json`).
        #[arg(long)]
        file: Option<PathBuf>,
    },
}

pub fn run(command: CatalogCommand) -> ExitCode {
    match command {
        CatalogCommand::Build { output } => match build::run(&output) {
            Ok(summary) => {
                print!("{summary}");
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("xtask catalog build: {err}");
                ExitCode::from(1)
            }
        },
        CatalogCommand::Validate { file } => {
            let path = file.unwrap_or_else(|| PathBuf::from("catalog/engines.json"));
            match validate::run(&path) {
                Ok(msg) => {
                    println!("{msg}");
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("xtask catalog validate: {err}");
                    ExitCode::from(1)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Debug, Parser)]
    struct Cli {
        #[command(subcommand)]
        command: CatalogCommand,
    }

    #[test]
    fn build_argument_matches_the_workflow_command() {
        let cli = Cli::try_parse_from(["catalog", "build", "--output", "catalog/engines.json"])
            .expect("parse");
        match cli.command {
            CatalogCommand::Build { output } => {
                assert_eq!(output, PathBuf::from("catalog/engines.json"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn validate_defaults_to_the_bundled_file() {
        let cli = Cli::try_parse_from(["catalog", "validate"]).expect("parse");
        match cli.command {
            CatalogCommand::Validate { file } => assert!(file.is_none()),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn validate_subcommand_reports_success_and_failure() {
        let bundled = PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../catalog/engines.json"
        ));
        assert_eq!(
            run(CatalogCommand::Validate {
                file: Some(bundled)
            }),
            ExitCode::SUCCESS
        );

        let dir = tempfile::tempdir().expect("tempdir");
        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, b"{}").expect("write");
        assert_eq!(
            run(CatalogCommand::Validate { file: Some(bad) }),
            ExitCode::from(1)
        );
    }
}
