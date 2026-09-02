//! `prgz` (Progress Gzip): compress one file with gzip.
//!
//! This file holds the command line of the tool. The library beside it holds
//! the compression and the closing report.

#![warn(clippy::unwrap_used)]
#![warn(clippy::expect_used)]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

/// The name of the tool.
const PROGRAM_NAME: &str = "prgz";

/// Compress one file with gzip and show the progress of the run.
#[derive(Parser)]
#[command(name = PROGRAM_NAME, about, long_about = None)]
struct Cli {
    /// The file to compress
    #[arg(long, value_name = "PATH")]
    input: Option<PathBuf>,

    /// The file to write. A run that gets no output name adds `.gz` to the
    /// input name
    #[arg(long, value_name = "PATH")]
    output: Option<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let _ = (cli.input, cli.output);
    ExitCode::SUCCESS
}
