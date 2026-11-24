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
//!
//! That holds for a build without the `progress` feature. The progress display
//! is the one part of the shell that measures a terminal: it draws a spinner
//! and a bar on standard error while a replay runs, and it needs the width to
//! size the bar. So the display lives behind the `progress` feature, which
//! `grind` and `grime` turn on and `grist` does not. Without the feature, this
//! crate holds no dependency on `indicatif` or `termbar`.
//!
//! The display shows only when standard error is a terminal, `-q` is not on,
//! and the run has lasted `PROGRESS_DELAY`. A run that ends sooner draws no
//! byte. With standard error on a pipe or a file, the bytes of a run are the
//! bytes that it wrote before the display existed. [`Console::note`],
//! [`Console::verdict`], and the failure line clear the display before they
//! write, so a partial bar never shares a line with them.

#[cfg(feature = "progress")]
use std::io::IsTerminal;
use std::io::Write;
use std::process::ExitCode;
#[cfg(feature = "progress")]
use std::sync::{Mutex, PoisonError};
#[cfg(feature = "progress")]
use std::time::Duration;

use anyhow::Result;

use crate::scratch::Conflicts;

#[cfg(feature = "progress")]
mod progress;

#[cfg(feature = "progress")]
pub use progress::Progress;

/// How long a run must take before the progress display shows.
///
/// A run that ends in less time draws nothing. A person then sees no flash of
/// a bar that is gone before it can be read.
#[cfg(feature = "progress")]
pub const PROGRESS_DELAY: Duration = Duration::from_millis(500);

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
/// With the `progress` feature, the type also owns the progress display of a
/// replay, and `-q` hides it. The display goes to standard error and only when
/// standard error is a terminal. The three writes below clear it first.
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

    /// How long a run takes before the progress display shows.
    #[cfg(feature = "progress")]
    progress_delay: Duration,

    /// The live progress display, if one is.
    #[cfg(feature = "progress")]
    active: Mutex<Option<progress::Active>>,
}

/// A body that returns with no verdict leaves no bar on the screen.
#[cfg(feature = "progress")]
impl Drop for Console {
    fn drop(&mut self) {
        self.clear_progress();
    }
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
        #[cfg(feature = "progress")]
        {
            Self::answer_with_progress_delay(tool, quiet, PROGRESS_DELAY, ask)
        }
        #[cfg(not(feature = "progress"))]
        {
            Self::run(Console { tool, quiet }, ask)
        }
    }

    /// Answer the question as [`Console::answer`] does, with a progress display
    /// that waits `delay` before it shows.
    ///
    /// A test sets the delay to zero, so the display shows at once, or to a
    /// very long time, so the display never shows. A tool calls
    /// [`Console::answer`], which uses [`PROGRESS_DELAY`].
    #[cfg(feature = "progress")]
    pub fn answer_with_progress_delay<F>(
        tool: &'static str,
        quiet: bool,
        delay: Duration,
        ask: F,
    ) -> ExitCode
    where
        F: FnOnce(&Console) -> Result<Conflicts>,
    {
        Self::run(
            Console {
                tool,
                quiet,
                progress_delay: delay,
                active: Mutex::new(None),
            },
            ask,
        )
    }

    /// Run `ask` against `console` and map its answer onto the exit code.
    ///
    /// The one place that maps the answer onto a code, so the two doors cannot
    /// map it two ways.
    fn run<F>(console: Console, ask: F) -> ExitCode
    where
        F: FnOnce(&Console) -> Result<Conflicts>,
    {
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

    /// Start the progress display of a replay, and give the handle that the
    /// replay reports to.
    ///
    /// `action` is the sentence of the replay, for example
    /// `replaying HEAD onto main`. The display shows it next to the name of
    /// the tool, and the replay then reports its phase and its commit
    /// position through the handle, for example through
    /// [`Repo::scratch_with_progress`](crate::Repo::scratch_with_progress).
    ///
    /// A call ends the display that an earlier call started, and then starts a
    /// new one. The handle is hidden, and does nothing on a report, when `-q`
    /// is on, when standard error is not a terminal, or when the thread of the
    /// display does not start. A display that cannot start costs the display
    /// and never the verdict or the exit code, so this method returns no error.
    /// The display shows after the delay of the console, and
    /// [`Console::note`], [`Console::verdict`], and the failure line end it
    /// before they write.
    #[cfg(feature = "progress")]
    pub fn progress(&self, action: &str) -> Progress {
        self.clear_progress();
        if self.quiet || !std::io::stderr().is_terminal() {
            return Progress::hidden();
        }
        let Some((handle, active)) = Progress::start(self.tool, action, self.progress_delay) else {
            return Progress::hidden();
        };
        *self.active.lock().unwrap_or_else(PoisonError::into_inner) = Some(active);
        handle
    }

    /// End the display, if one is live, and take its lines off the screen.
    ///
    /// Every write of this type calls it first, so a partial bar never shares
    /// a line with a note, a verdict, or an error. It does nothing when no
    /// display is live. A poisoned lock does not stop it, because the display
    /// holds no state that a panic can leave half written.
    #[cfg(feature = "progress")]
    fn clear_progress(&self) {
        let taken = self
            .active
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(active) = taken {
            active.stop();
        }
    }

    /// A caveat that qualifies the verdict without changing it.
    pub fn note(&self, note: &str) {
        #[cfg(feature = "progress")]
        self.clear_progress();
        if !self.quiet {
            let _ = writeln!(std::io::stderr(), "{note}");
        }
    }

    /// The answer itself.
    pub fn verdict(&self, verdict: &str) {
        #[cfg(feature = "progress")]
        self.clear_progress();
        if !self.quiet {
            let _ = writeln!(std::io::stdout(), "{verdict}");
        }
    }

    /// Why there is no answer.
    fn failure(&self, err: &anyhow::Error) {
        #[cfg(feature = "progress")]
        self.clear_progress();
        if !self.quiet {
            // Alternate formatting so the whole context chain arrives, not just
            // the outermost sentence: git's own stderr is carried in the causes
            // and is usually the only part that says what actually went wrong.
            let _ = writeln!(std::io::stderr(), "{}: error: {err:#}", self.tool);
        }
    }
}
