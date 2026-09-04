//! `grind` - Git Rebase In aNother Dimension: would rebasing HEAD onto a
//! branch conflict, and by how much?

use std::io::Write;
use std::process::ExitCode;

use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::Parser;
use gitscratch::{Repo, Report};
use termbar::TerminalWidth;

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

/// Everything `grind` says, and the one switch that can silence it.
///
/// `-q` has to reach three writes on three different paths - the
/// uncommitted-work note, the verdict, and the failure - and the last of those
/// is printed from [`main`], nowhere near the other two. Three independent
/// `if !quiet` checks would be one design decision smeared across three sites,
/// which is precisely the shape that goes wrong the first time somebody adds a
/// fourth line. Routing every write through one type makes the check
/// impossible to forget rather than merely easy to remember.
///
/// The methods are named for what is being said rather than for which stream
/// it lands on, because which stream is this type's decision to make: the
/// verdict is the answer and belongs on stdout, while a caveat or a failure
/// belongs on stderr where it cannot contaminate a pipeline.
///
/// Routing the writes through one type is also what makes them unable to
/// *panic*, which matters more here than in most tools. `println!` and
/// `eprintln!` panic when the write fails, and a reader that closes early -
/// `grind main | head -1`, a pipeline whose consumer exits first, a terminal
/// that went away - makes it fail with `EPIPE`, because Rust ignores `SIGPIPE`
/// and hands the error back rather than letting the signal end the process. A
/// panic there unwinds straight past [`main`]'s hand-mapped codes and exits
/// **101**: a fourth code the README does not publish, produced by the one tool
/// whose entire contract is that its exit code is the answer. So every write
/// here goes through `writeln!` with the result deliberately discarded - the
/// words are what a broken pipe costs, never the answer - and the discarding
/// lives at the three sites this type already owns rather than being a rule
/// every future caller has to know.
struct Console {
    quiet: bool,
}

impl Console {
    /// A caveat that qualifies the verdict without changing it.
    fn note(&self, note: &str) {
        if !self.quiet {
            let _ = writeln!(std::io::stderr(), "{note}");
        }
    }

    /// The answer itself.
    fn verdict(&self, verdict: &str) {
        if !self.quiet {
            let _ = writeln!(std::io::stdout(), "{verdict}");
        }
    }

    /// Why there is no answer.
    fn failure(&self, err: &anyhow::Error) {
        if !self.quiet {
            // Alternate formatting so the whole context chain arrives, not just
            // the outermost sentence: git's own stderr is carried in the causes
            // and is usually the only part that says what actually went wrong.
            let _ = writeln!(std::io::stderr(), "{TOOL}: error: {err:#}");
        }
    }
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
    let console = Console { quiet: args.quiet };

    match run(&args, &console) {
        Ok(code) => code,
        Err(err) => {
            console.failure(&err);
            ExitCode::from(ERROR)
        }
    }
}

/// Answer the question, returning the code that carries the answer.
///
/// # Errors
///
/// Returns an error if the current directory cannot be read, is not inside a
/// git repository, has no commit at HEAD to replay, does not contain `branch`,
/// or if the replay itself failed without leaving a conflict to measure. Not for
/// a working tree that cannot be inspected: that only costs the uncommitted-work
/// note, which is a caveat rather than part of the answer.
fn run(args: &Args, console: &Console) -> Result<ExitCode> {
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
    repo.resolve(&args.branch)?;

    // The tool's name alone. The note below reads nothing else, and the verdict
    // adds the action on the next line. `UnwordedReport` is `Copy`, so one
    // value serves both.
    let unworded = Report::for_tool(TOOL);
    let action = format!("replaying HEAD onto {}", args.branch);
    let report = unworded.describing(&action);

    // Before the verdict, and on stderr rather than stdout: a reader has to see
    // the caveat before the sentence it qualifies, and a caller piping stdout
    // somewhere has to get the same bytes whether or not the tree was dirty.
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
    if let Some(note) = unworded.dirty_note(repo.uncommitted_files().unwrap_or_default()) {
        console.note(&note);
    }

    let scratch = repo.scratch("HEAD")?;
    let conflicts = scratch.replay_rebase(&args.branch)?;

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
    console
        .verdict(&report.render_within(&conflicts, usize::from(TerminalWidth::get_or_default())));

    // Read off the same fact the report was rendered from, so the words and the
    // number a script acts on cannot tell two different stories.
    Ok(ExitCode::from(if conflicts.is_clean() {
        CLEAN
    } else {
        CONFLICTS
    }))
}
