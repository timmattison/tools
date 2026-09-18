//! `faulte` — "fault rate": rank the processes of this Mac by page faults per
//! second, and stop the old idle Claude Code sessions.

use std::process::ExitCode;

use buildinfo::version_string;
use clap::Parser;

/// The one-line description that `--help` shows.
const ABOUT: &str =
    "Fault rate: rank processes by page faults per second, and stop old idle Claude Code sessions";

#[derive(Parser)]
#[command(name = "faulte", version = version_string!(), about = ABOUT)]
struct Cli {}

fn main() -> ExitCode {
    let Cli {} = Cli::parse();
    ExitCode::SUCCESS
}
