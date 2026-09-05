//! `grime` - Git ReadIness for Merging Externally: would merging a branch into
//! HEAD conflict, and by how much?

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
const TOOL: &str = "grime";

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
/// Report whether merging BRANCH into HEAD would conflict, and by how much
#[derive(Parser, Debug)]
#[clap(author, version = version_string!())]
struct Args {
    /// Branch to merge into HEAD
    #[clap(value_name = "BRANCH")]
    branch: String,

    /// Print nothing about the merge - the exit code is the answer
    #[clap(short, long)]
    quiet: bool,
}

/// Hands the whole shell to [`Console::answer`] - what this tool says, the one
/// switch that silences it, and the three exit codes it answers with. The
/// question below is all that is left, and it is all `grime` owns.
///
/// See [`Console::answer`] for why an [`ExitCode`] is returned rather than a
/// `Result`: a `Result` exits **1**, which is already the code for a merge that
/// would conflict.
fn main() -> ExitCode {
    let args = Args::parse();

    Console::answer(TOOL, args.quiet, |console| run(&args, console))
}

/// Answer the question, returning what the merge would cost.
///
/// # Errors
///
/// Returns an error if the current directory cannot be read, is not inside a
/// git repository, has no commit at HEAD to merge into, does not contain
/// `branch`, or if the merge itself failed without leaving a conflict to
/// measure. Not for a working tree that cannot be inspected: that only costs
/// the uncommitted-work note, which is a caveat rather than part of the answer.
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
        "a merge starts from HEAD, and there is no commit at HEAD to merge into \
         - an empty repository, or a branch nothing has been committed to yet",
    )?;
    repo.resolve(&args.branch)?;

    // The tool's name alone. The note below reads nothing else, and the verdict
    // adds the action on the next line. `UnwordedReport` is `Copy`, so one
    // value serves both.
    let unworded = Report::for_tool(TOOL);
    let action = format!("merging {} into HEAD", args.branch);

    // `without_stops`, because a merge halts exactly once. Git makes one
    // three-way merge and stops at it, so the count is `1` for every conflicted
    // merge and `0` for every clean one - a constant dressed up as a
    // measurement. Printing it would invite a reader to weigh it against
    // `grind`'s stop count, which is a real measurement of how many times a
    // rebase halted, and the comparison would be meaningless.
    //
    // The count is dropped from the *words* and nowhere else. `Conflicts` still
    // records the halt, because a caller folding several replays together adds
    // those halts up, and because the two tools have to measure the same thing
    // to stay comparable. Only this sentence leaves it out.
    let report = unworded.describing(&action).without_stops();

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
    let conflicts = scratch.replay_merge(&args.branch)?;

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
    // state a width of its own.
    console
        .verdict(&report.render_within(&conflicts, usize::from(TerminalWidth::get_or_default())));

    Ok(conflicts)
}
