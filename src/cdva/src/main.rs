//! `cdva` — "count da various attributes".
//!
//! Counts the lines of code of a tree, as `cloc` does, and reports the test
//! code apart from the production code.
//!
//! This slice carries the command line, the walk, and the default table. The
//! tree rule and the other reports arrive in later slices.

use anyhow::Result;
use buildinfo::version_string;
use clap::Parser;
use std::path::PathBuf;

/// The path the tool counts when the command line names none.
const DEFAULT_PATH: &str = ".";

/// Count the lines of a tree, and report the test code apart from the
/// production code.
#[derive(Parser)]
#[command(
    name = "cdva",
    version = version_string!(),
    about = "Count da various attributes: count the lines of a tree, and report the test code apart from the production code"
)]
struct Cli {
    /// The paths to count.
    #[arg(value_name = "PATH", default_value = DEFAULT_PATH)]
    paths: Vec<PathBuf>,
    /// Count a hidden file or directory.
    #[arg(long)]
    hidden: bool,
    /// Ignore every ignore file, including .gitignore.
    #[arg(long)]
    no_ignore: bool,
    /// Mark a path as test material. Repeat for more than one glob.
    #[arg(long, value_name = "GLOB")]
    test_glob: Vec<String>,
    /// Hold a path out of the test bucket. Repeat for more than one glob.
    #[arg(long, value_name = "GLOB")]
    production_glob: Vec<String>,
}

fn main() -> Result<()> {
    let _cli = Cli::parse();

    Ok(())
}
