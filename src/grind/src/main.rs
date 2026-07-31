//! `grind` - Git Rebase In aNother Dimension: would rebasing HEAD onto a
//! branch conflict, and by how much?

use std::process::ExitCode;

use buildinfo::version_string;
use clap::Parser;

/// Report whether rebasing HEAD onto BRANCH would conflict, and by how much
#[derive(Parser, Debug)]
#[clap(author, version = version_string!(), about)]
struct Args {
    /// Branch to rebase HEAD onto
    #[clap(value_name = "BRANCH")]
    branch: String,

    /// Print nothing; the exit code is the answer
    #[clap(short, long)]
    quiet: bool,
}

fn main() -> ExitCode {
    let _args = Args::parse();

    ExitCode::from(2)
}
