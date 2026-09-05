//! Everything a dry-run reporter says, and the code it exits with.
//!
//! `grime` asks whether merging a branch into HEAD would conflict; `grind` asks
//! whether replaying HEAD onto a branch would. Different questions, and the
//! same program around them: parse a command line, run the replay, print the
//! verdict, exit with a number a script can act on. Only the middle of that
//! differs.
//!
//! Both tools carried a copy of the rest, and the copy covered every guarantee
//! their READMEs publish - the three exit codes, the stream each kind of
//! sentence lands on, the one switch that silences them, and the write
//! discipline that keeps a closed pipe from turning the answer into a panic.
//! Two copies of a published guarantee is one fix that reaches one binary, and
//! the two had already begun to drift.
//!
//! So the shell lives here, beside the [`Report`](crate::report::Report) it
//! prints, for the reason the renderer does: what two tools must answer alike
//! belongs to neither of them. A tool built on it owns the question and nothing
//! around it - its arguments, the sentence naming what it did, and which replay
//! answers it.
//!
//! [`Console::answer`] is the only door. It builds the console, hands it to the
//! tool's own body, and turns what that body answers into the code the caller
//! reads. So a consumer cannot silence one stream and forget another, cannot
//! map a verdict to a number of its own, and has no route to a write that could
//! panic on a broken pipe.
//!
//! The shell stops short of the terminal, and that boundary is deliberate. A
//! tool reads its own width and hands it to
//! [`Report::render_within`](crate::report::Report::render_within), because
//! measuring a terminal is a decision about one program's output rather than
//! about every program built on this crate. `grist` settles it: it is built on
//! this crate, prints no breakdown at all, and must not pay for a
//! terminal-size dependency it would never read.

use std::io::Write;
use std::process::ExitCode;

use anyhow::Result;

use crate::scratch::Conflicts;

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
/// Deliberately not `1`. "The operation would conflict" and "I could not tell
/// you" are different answers, and the shell functions these tools replace
/// reported both as the same number - which is how a typo'd branch name came to
/// be reported as a conflict.
const ERROR: u8 = 2;

/// Everything a tool built on this crate says about a replay, and the one
/// switch that can silence it.
///
/// The switch reaches what this type writes, and nothing before it. Argument
/// parsing runs ahead of the `Console`, so the parser owns two writes of its
/// own. One is the usage error for a command line it refuses, and the other is
/// the version line. Both answer about the tool rather than about a replay.
/// Silencing the refusal leaves a caller with a bare exit code and no word
/// about which argument is missing.
///
/// `-q` has to reach three writes on three different paths - the
/// uncommitted-work note, the verdict, and the failure - and the last of those
/// is written by [`Console::answer`], nowhere near the other two and on a path
/// the tool's own body never reaches. Three independent `if !quiet` checks
/// would be one design decision smeared across three sites, which is precisely
/// the shape that goes wrong the first time somebody adds a fourth line.
/// Routing every write through one type makes the check impossible to forget
/// rather than merely easy to remember.
///
/// The methods are named for what is being said rather than for which stream it
/// lands on, because which stream is this type's decision to make: the verdict
/// is the answer and belongs on stdout, while a caveat or a failure belongs on
/// stderr where it cannot contaminate a pipeline.
///
/// Routing the writes through one type is also what makes them unable to
/// *panic*, which matters more here than in most tools. `println!` and
/// `eprintln!` panic when the write fails, and a reader that closes early - a
/// run piped into `head -1`, a pipeline whose consumer exits first, a terminal
/// that went away - makes it fail with `EPIPE`, because Rust ignores `SIGPIPE`
/// and hands the error back rather than letting the signal end the process. A
/// panic there unwinds straight past the codes [`Console::answer`] maps by hand
/// and exits **101**: a fourth code no consumer's README publishes, produced by
/// the one kind of tool whose entire contract is that its exit code is the
/// answer. So every write here goes through `writeln!` with the result
/// deliberately discarded - the words are what a broken pipe costs, never the
/// answer - and the discarding lives at the three sites this type already owns
/// rather than being a rule every future caller has to know.
pub struct Console {
    /// The tool's own name, on the failure line and on the report it renders.
    tool: &'static str,

    /// Whether `-q` was given, and therefore whether any of the three writes
    /// below happens at all.
    quiet: bool,
}

impl Console {
    /// Answer the question this tool exists to answer, and exit with the code
    /// that carries the answer.
    ///
    /// The one entrance, and the whole of a tool's `main`. `tool` names the
    /// program on the lines it prints, `quiet` is its `-q`, and `ask` is the
    /// question itself - the pre-flight, the replay and the verdict, with a
    /// [`Console`] to say them through.
    ///
    /// Returns an [`ExitCode`] rather than a `Result`, which looks like a
    /// stylistic choice and is not. `fn main() -> Result<()>` prints the error
    /// and exits **1** - the same code these tools use to mean "the operation
    /// would conflict". Every failure would then be indistinguishable from a
    /// conflict to the script reading the number, which is precisely the defect
    /// they were written to fix. Mapping the codes by hand is the only way to
    /// keep "conflicts" and "could not tell" apart, and mapping them once here
    /// is what stops two tools mapping them two ways.
    ///
    /// `ask` answers with the [`Conflicts`] it measured and never with a code
    /// of its own. The number a script acts on is then read off the very value
    /// the report was rendered from, so the words and the number cannot tell
    /// two different stories, and no tool can publish a fourth code or swap two
    /// of the three.
    pub fn answer<F>(tool: &'static str, quiet: bool, ask: F) -> ExitCode
    where
        F: FnOnce(&Console) -> Result<Conflicts>,
    {
        let console = Console { tool, quiet };

        match ask(&console) {
            Ok(conflicts) => ExitCode::from(if conflicts.is_clean() {
                CLEAN
            } else {
                CONFLICTS
            }),
            Err(err) => {
                console.failure(&err);
                ExitCode::from(ERROR)
            }
        }
    }

    /// A caveat that qualifies the verdict without changing it.
    pub fn note(&self, note: &str) {
        if !self.quiet {
            let _ = writeln!(std::io::stderr(), "{note}");
        }
    }

    /// The answer itself.
    pub fn verdict(&self, verdict: &str) {
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
            let _ = writeln!(std::io::stderr(), "{}: error: {err:#}", self.tool);
        }
    }
}
