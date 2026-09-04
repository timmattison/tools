//! `grime` - Git ReadIness for Merging Externally: would merging a branch into
//! HEAD conflict, and by how much?

use std::io::Write;
use std::process::ExitCode;

use anyhow::{bail, Result};
use buildinfo::version_string;
use clap::Parser;

/// The tool's own name, on every line it prints and on the report it renders.
///
/// One constant rather than a literal per call site, so the prefix a script
/// greps for and the prefix the report indents its summary under cannot end up
/// disagreeing.
const TOOL: &str = "grime";

/// Exit code for a run that could not answer the question at all - a branch
/// that does not resolve, a directory that is not a repository, or git failing
/// in a way that left no conflict to measure.
///
/// Deliberately not `1`. "The merge would conflict" and "I could not tell you"
/// are different answers, and the shell function this tool replaces reported
/// both as the same number - which is how a typo'd branch name came to be
/// reported as a conflict.
const ERROR: u8 = 2;

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

/// Everything `grime` says, and the one switch that can silence it.
///
/// The switch reaches what this type writes, and nothing before it.
/// `Args::parse` runs ahead of the `Console`, so clap owns two writes of its
/// own. One is the usage error for a command line it refuses, and the other is
/// the version line. Both answer about the tool rather than about a merge.
/// Silencing the refusal leaves a caller with a bare exit code and no word
/// about which argument is missing.
///
/// Routing the writes through one type is also what makes them unable to
/// *panic*, which matters more here than in most tools. `eprintln!` panics when
/// the write fails, and a reader that closes early - a pipeline whose consumer
/// exits first, a terminal that went away - makes it fail with `EPIPE`, because
/// Rust ignores `SIGPIPE` and hands the error back rather than letting the
/// signal end the process. A panic there unwinds straight past [`main`]'s
/// hand-mapped codes and exits **101**: a fourth code nothing documents,
/// produced by the one tool whose entire contract is that its exit code is the
/// answer. So every write here goes through `writeln!` with the result
/// deliberately discarded - the words are what a broken pipe costs, never the
/// answer - and the discarding lives at the sites this type already owns rather
/// than being a rule every future caller has to know.
struct Console {
    quiet: bool,
}

impl Console {
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
/// this tool uses to mean "the merge would conflict". Every failure would then
/// be indistinguishable from a conflict to the script reading the number, which
/// is precisely the defect `grime` was written to fix. Mapping the codes by
/// hand is the only way to keep "conflicts" and "could not tell" apart.
fn main() -> ExitCode {
    let args = Args::parse();
    let console = Console { quiet: args.quiet };

    match run(&args) {
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
/// Returns an error every time, because the replay this tool exists to perform
/// is not wired up yet. The command line is read and nothing else happens, so
/// the behavior tests that follow fail on the answer rather than on a symbol
/// that does not exist.
fn run(args: &Args) -> Result<ExitCode> {
    bail!("merging {} into HEAD is not wired up yet", args.branch)
}
