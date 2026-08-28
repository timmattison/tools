//! `cdva` — "count da various attributes".
//!
//! Counts the lines of code of a tree, as `cloc` does, and reports the test
//! code apart from the production code.
//!
//! This slice carries the command line and the language table. The walk, the
//! line classifier, the test rules, and the report arrive in later slices.

use anyhow::Result;
use buildinfo::version_string;
use clap::Parser;
use std::path::PathBuf;

/// The path the tool counts when the command line names none.
const DEFAULT_PATH: &str = ".";

/// Count the lines of code of a tree, and report the test code apart from the
/// production code.
#[derive(Parser)]
#[command(name = "cdva", version = version_string!())]
struct Cli {
    /// The files and directories to count.
    #[arg(value_name = "PATH", default_value = DEFAULT_PATH)]
    paths: Vec<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    for path in &cli.paths {
        println!("{}", path.display());
    }

    Ok(())
}
