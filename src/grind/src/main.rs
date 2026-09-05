//! `grind` - Git Rebase In aNother Dimension: would rebasing HEAD onto a
//! branch conflict, and by how much?

use std::process::ExitCode;

use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::Parser;
use gitscratch::{Conflicts, Console, Repo, Report};
use termbar::TerminalWidth;

/// The tool's own name, on every line it prints and on the report it renders.
///
/// One constant rather than a literal per call site, so the prefix a script
/// greps for and the prefix [`Report`] indents its summary under cannot end up
/// disagreeing.
const TOOL: &str = "grind";

// No `about` in the attribute below, and the absence is the point. clap's
// derive takes the doc comment as the help text. A bare `about` takes
// `CARGO_PKG_DESCRIPTION` instead, and the doc comment then reaches nobody.
// Two sentences describe the tool, one of them is dead, and the dead one sits
// where a developer edits the help. The manifest keeps its own sentence, which
// crates.io and `cargo search` show to a reader who has run nothing.
//
// One line only. A second paragraph here becomes clap's `long_about`, which
// `--help` prints and `-h` does not, and the two spellings of one switch then
// answer differently.
/// Report whether rebasing HEAD onto BRANCH would conflict, and by how much
#[derive(Parser, Debug)]
#[clap(author, version = version_string!())]
struct Args {
    /// Branch to rebase HEAD onto [default: main, else master]
    ///
    // The default is stated in the doc comment above rather than through clap's
    // `default_value`, and the difference is the refusal. A `default_value` of
    // `main` hands the tool a name in a repository that has no `main`, and the
    // run then fails as though the developer had typed a branch that does not
    // exist. The choice needs to see the repository, so it happens where the
    // repository is open - and `None` is how it learns nobody named one.
    #[clap(value_name = "BRANCH")]
    branch: Option<String>,

    /// Print nothing about the rebase - the exit code is the answer
    #[clap(short, long)]
    quiet: bool,
}

/// Hands the whole shell to [`Console::answer`] - what this tool says, the one
/// switch that silences it, and the three exit codes it answers with. The
/// question below is all that is left, and it is all `grind` owns.
///
/// A run that names no `BRANCH` is not a usage error. The argument is
/// optional, and the branch it stands for is chosen from the repository - so a
/// repository that holds no default branch to pick is refused by `grind`
/// itself, in a message `-q` does silence.
///
/// See [`Console::answer`] for why an [`ExitCode`] is returned rather than a
/// `Result`: a `Result` exits **1**, which is already the code for a rebase
/// that would conflict.
fn main() -> ExitCode {
    let args = Args::parse();

    Console::answer(TOOL, args.quiet, |console| run(&args, console))
}

/// Answer the question, returning what the rebase would cost.
///
/// # Errors
///
/// Returns an error if the current directory cannot be read, is not inside a
/// git repository, has no commit at HEAD to replay, holds no branch the run
/// could measure against - the one named, or a default when none was named -
/// or if the replay itself failed without leaving a conflict to measure. Not for
/// a working tree that cannot be inspected: that only costs the uncommitted-work
/// note, which is a caveat rather than part of the answer.
fn run(args: &Args, console: &Console) -> Result<Conflicts> {
    let cwd = std::env::current_dir().context("could not determine the current directory")?;
    let repo = Repo::open(&cwd)?;

    // Both revisions the run depends on, before any scratch worktree exists.
    // Creating one costs a temporary directory, a real `git worktree add`, and
    // administrative state in the developer's repository - all of it wasted if a
    // revision was never going to resolve, and worse, the failure would arrive
    // looking like a failed simulation instead of a bad state.
    //
    // HEAD is the one that is easy to leave out, because it is the only revision
    // the user does not type. `git worktree add` resolves it eventually, so the
    // exit code came out right either way - but it came out as
    // `fatal: invalid reference: HEAD` wrapped around a temporary path that no
    // longer exists, which tells a developer with an empty repository or a fresh
    // orphan branch nothing about what is actually wrong.
    repo.resolve("HEAD").context(
        "a replay starts from HEAD, and there is no commit at HEAD to start from \
         - an empty repository, or a branch nothing has been committed to yet",
    )?;
    // Before the branch resolves, because there is no branch to resolve until
    // the choice is made. `gitscratch` owns the choice rather than this file,
    // so every tool built on it picks the same branch and refuses in the same
    // words.
    //
    // The name comes back unresolved, and the line below is what resolves it.
    // That keeps one message for a branch that does not exist, whether the
    // developer typed the name or the default supplied it.
    let branch = repo.branch_or_default(args.branch.as_deref())?;
    repo.resolve(&branch)?;

    // The tool's name alone. The note below reads nothing else, and the verdict
    // adds the action on the next line. `UnwordedReport` is `Copy`, so one
    // value serves both.
    let unworded = Report::for_tool(TOOL);
    // The chosen name, so this line tells the developer which branch got
    // measured on a run that named none.
    let action = format!("replaying HEAD onto {branch}");
    let report = unworded.describing(&action);

    // Read here and printed later, which is two decisions rather than one.
    //
    // Read before the scratch worktree exists, because a scratch worktree can
    // land inside the repository. A `TMPDIR` pointing under the repository is
    // all that takes, and `git status` then counts the scratch itself as the
    // user's own uncommitted work.
    //
    // Printed after the replay comes back, because a caveat qualifies an
    // answer. A caveat ahead of a failed scratch, or ahead of a failed replay,
    // qualifies a verdict that never arrives. That is a wrong sentence rather
    // than an early one. The suite states the same rule one step earlier, where
    // a HEAD with nothing on it is refused with no note.
    //
    // The wait costs the reader nothing. The note goes to stderr and the
    // verdict to stdout, so the caveat still reaches a terminal ahead of the
    // sentence it qualifies. A caller who pipes stdout gets the same bytes
    // whether or not the tree was dirty.
    //
    // `unwrap_or_default` rather than `?`, because a caveat that cannot be
    // computed has to cost the caveat and not the answer. A bare repository is
    // the case that settles it: `git worktree add --detach HEAD` succeeds
    // against one, so the replay runs and measures the collision exactly as
    // usual, while `git status` cannot run at all for want of a working tree.
    // Propagating that made the cheap pre-flight query *stricter* than the
    // expensive replay it exists to spare the user - exit 2 and a raw git
    // complaint about a query they never asked for, in place of a right answer.
    // A default count is a clean tree, which `UnwordedReport::dirty_note`
    // already words as no note at all.
    let dirty_note = unworded.dirty_note(repo.uncommitted_files().unwrap_or_default());

    let scratch = repo.scratch("HEAD")?;
    let conflicts = scratch.replay_rebase(&branch)?;

    // There is a verdict now, so the caveat has something to qualify.
    if let Some(note) = dirty_note {
        console.note(&note);
    }

    // The breakdown lines its hunk counts up in one column, and past the
    // right-hand edge of the terminal there is no such column: the terminal
    // wraps every one of those lines instead. So the width goes to the
    // renderer, which clamps the name column to fit and gives a name too wide
    // for the clamp a line of its own. Reading the terminal is this tool's job
    // rather than the library's, because it is a decision about this program's
    // own output and `gitscratch` renders for every consumer that asks.
    //
    // Through `TerminalWidth` rather than off the ioctl, because a terminal
    // that carries no window answers that ioctl with zero columns and succeeds.
    // A run laid out at zero columns puts every name on a line of its own for
    // no reason. `get_or_default` refuses the zero and stands the fallback of
    // 80 columns in its place.
    //
    // It also decides between the two sources of a width. A width the
    // environment states in `COLUMNS` wins, and the ioctl answers when the
    // environment states none. That is the rule POSIX gives the variable, and
    // it is what lets a wrapper report the terminal it holds and lets a test
    // state a width of its own. `tests/controlling-terminal.rs` holds that rule
    // against a pseudo-terminal of a size it chose.
    console
        .verdict(&report.render_within(&conflicts, usize::from(TerminalWidth::get_or_default())));

    Ok(conflicts)
}
