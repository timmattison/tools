//! `grind` - Git Rebase In aNother Dimension: would rebasing HEAD onto a
//! branch conflict, and by how much?

use std::process::ExitCode;

use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::Parser;
use gitscratch::{Repo, Report, Scratch};

/// The tool's own name, on every line it prints and on the report it renders.
///
/// One constant rather than a literal per call site, so the prefix a script
/// greps for and the prefix [`Report`] indents its summary under cannot end up
/// disagreeing.
const TOOL: &str = "grind";

/// Exit code for a replay that hit no conflicts.
const CLEAN: u8 = 0;

/// Exit code for a replay that hit conflicts.
///
/// The conflict verdict is the *answer*, not a failure, which is why it gets a
/// code of its own rather than sharing one with the things that went wrong.
const CONFLICTS: u8 = 1;

/// Exit code for a run that could not answer the question at all - a branch
/// that does not resolve, a directory that is not a repository, or git failing
/// in a way that left no conflict to measure.
///
/// Deliberately not `1`. "The rebase would conflict" and "I could not tell you"
/// are different answers, and the shell function this tool replaces reported
/// both as the same number - which is how a typo'd branch name came to be
/// reported as a conflict.
const ERROR: u8 = 2;

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

/// Returns an [`ExitCode`] rather than a `Result`, which looks like a stylistic
/// choice and is not.
///
/// `fn main() -> Result<()>` prints the error and exits **1** - the same code
/// this tool uses to mean "the rebase would conflict". Every failure would then
/// be indistinguishable from a conflict to the script reading the number, which
/// is precisely the defect `grind` was written to fix. Mapping the codes by
/// hand is the only way to keep "conflicts" and "could not tell" apart.
fn main() -> ExitCode {
    let args = Args::parse();

    match run(&args) {
        Ok(code) => code,
        Err(err) => {
            // Alternate formatting so the whole context chain arrives, not just
            // the outermost sentence: git's own stderr is carried in the causes
            // and is usually the only part that says what actually went wrong.
            eprintln!("{TOOL}: error: {err:#}");
            ExitCode::from(ERROR)
        }
    }
}

/// Answer the question, returning the code that carries the answer.
///
/// # Errors
///
/// Returns an error if the current directory cannot be read, is not inside a
/// git repository, does not contain `branch`, if the working tree cannot be
/// inspected, or if the replay itself failed without leaving a conflict to
/// measure.
fn run(args: &Args) -> Result<ExitCode> {
    let cwd = std::env::current_dir().context("could not determine the current directory")?;
    let repo = Repo::open(&cwd)?;

    // Before any scratch worktree exists. Creating one costs a temporary
    // directory, a real `git worktree add`, and administrative state in the
    // developer's repository - all of it wasted if the argument was a typo, and
    // worse, the failure would arrive looking like a failed simulation instead
    // of a bad argument.
    repo.resolve(&args.branch)?;

    let action = format!("replaying HEAD onto {}", args.branch);
    let report = Report::new(TOOL, &action);

    // Before the verdict, and on stderr rather than stdout: a reader has to see
    // the caveat before the sentence it qualifies, and a caller piping stdout
    // somewhere has to get the same bytes whether or not the tree was dirty.
    if let Some(note) = report.dirty_note(repo.uncommitted_files()?) {
        eprintln!("{note}");
    }

    let scratch = Scratch::create(repo.path(), "HEAD")?;
    let conflicts = scratch.replay_rebase(&args.branch)?;

    println!("{}", report.render(&conflicts));

    // Read off the same fact the report was rendered from, so the words and the
    // number a script acts on cannot tell two different stories.
    Ok(ExitCode::from(if conflicts.is_clean() {
        CLEAN
    } else {
        CONFLICTS
    }))
}
