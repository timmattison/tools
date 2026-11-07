//! `bibibi` finds the commit that caused a regression, with a test that a
//! later commit added.
//!
//! The name is a reference to the NSYNC song "Bye Bye Bye": `bibibi` finds the
//! bad commit, and then you can say goodbye to it.
//!
//! `git bisect` cannot use a red test, because each step puts the tree of an
//! older commit in the working directory, and the test is part of a newer tree.
//! `bibibi` takes the change of the test commit one time, as a patch, and
//! applies it at each commit that it tests, in a scratch worktree from
//! `gitscratch`. The checkout of the user does not change. The `README.md` at
//! the root of the repository gives the interface, the verdicts, and the exit
//! codes.

mod child;
mod interrupt;
mod report;
mod run;
mod step;

use std::ffi::OsString;
use std::process::ExitCode;

use buildinfo::version_string;
use clap::Parser;

/// Exit code when skips leave more than one candidate.
const EXIT_AMBIGUOUS: u8 = 1;

/// Exit code for a usage error, a refusal, or a git error.
const EXIT_ERROR: u8 = 2;

/// Find the commit that caused a regression, with a test from a later commit
#[derive(Parser, Debug)]
#[clap(author, version = version_string!())]
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
    let cli = Cli::parse();
    match run::run(&cli) {
        Ok(run::Outcome::Found) => ExitCode::SUCCESS,
        Ok(run::Outcome::Ambiguous) => ExitCode::from(EXIT_AMBIGUOUS),
        Ok(run::Outcome::Interrupted(signal)) => {
            eprintln!("bibibi: a signal ({signal}) stopped the run");
            // `run` returned, so the patch file and the scratch worktree are gone.
            std::process::exit(interrupt::exit_status(signal));
        }
        Err(error) => {
            eprintln!("bibibi: {error:#}");
            ExitCode::from(EXIT_ERROR)
        }
    }
}
