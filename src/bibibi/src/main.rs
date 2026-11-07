//! `bibibi` finds the commit that caused a regression, with a test that a
//! later commit added.

mod report;
mod step;

use std::ffi::OsString;
use std::process::ExitCode;

use buildinfo::version_string;
use clap::Parser;

/// Exit code for a usage error, a refusal, or a git error.
const EXIT_ERROR: u8 = 2;

/// Find the commit that caused a regression, with a test from a later commit
#[derive(Parser, Debug)]
#[clap(author, version = version_string!())]
#[allow(
    dead_code,
    reason = "the fields are read by the search, which a later commit adds"
)]
struct Cli {
    /// The commit that adds the test [default: HEAD]
    #[clap(value_name = "TEST_COMMIT")]
    test_commit: Option<String>,

    /// A commit where the test passes. Repeat the flag for more commits
    #[clap(long = "good", value_name = "COMMIT", required = true)]
    good: Vec<String>,

    /// A commit where the test fails [default: the parent of the test commit]
    #[clap(long, value_name = "COMMIT")]
    bad: Option<String>,

    /// A shell command that runs before the test. A failure skips the commit
    #[clap(long, value_name = "COMMAND")]
    build: Option<String>,

    /// The test, as an argument vector. It comes after `--`
    #[clap(last = true, required = true, value_name = "TEST_COMMAND")]
    test_command: Vec<OsString>,
}

fn main() -> ExitCode {
    let _cli = Cli::parse();
    ExitCode::from(EXIT_ERROR)
}
