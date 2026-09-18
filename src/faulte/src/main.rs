//! `faulte` — "fault rate": rank the processes of this Mac by page faults per
//! second, and stop the old idle Claude Code sessions.

use std::process::ExitCode;

use buildinfo::version_string;
use clap::{Parser, Subcommand};
use faulte::duration::Span;
#[cfg(target_os = "macos")]
use faulte::machine::{macos::Mac, observe, MachineError};
#[cfg(target_os = "macos")]
use faulte::pid::Pid;
#[cfg(target_os = "macos")]
use faulte::plan::{self, Candidate, PlanInput, Rules};
#[cfg(target_os = "macos")]
use faulte::render;
#[cfg(target_os = "macos")]
use faulte::stop::{self, Decision};
#[cfg(target_os = "macos")]
use std::io::{self, BufRead, IsTerminal, Write};
#[cfg(target_os = "macos")]
use std::time::Duration;

/// The one-line description that `--help` shows.
const ABOUT: &str =
    "Fault rate: rank processes by page faults per second, and stop old idle Claude Code sessions";

/// The default time of the sample.
const DEFAULT_INTERVAL: &str = "5s";

/// The default number of processes that the ranking shows.
const DEFAULT_LIMIT: usize = 25;

/// The default age that a session must pass before `faulte kill` selects it.
const DEFAULT_OLDER_THAN: &str = "7d";

/// The default idle time that a session must pass before `faulte kill`
/// selects it.
const DEFAULT_IDLE_FOR: &str = "10m";

/// The name that `--help` gives to the value of a flag that takes a duration.
const DURATION_VALUE: &str = "DURATION";

/// The name that `--help` gives to the value of a flag that takes a count.
const COUNT_VALUE: &str = "N";

/// The text that `faulte` gives on a platform other than macOS.
#[cfg(not(target_os = "macos"))]
const UNSUPPORTED_PLATFORM: &str = "faulte supports macOS only.";

/// The exit code on a platform other than macOS.
#[cfg(not(target_os = "macos"))]
const EXIT_UNSUPPORTED: u8 = 1;

/// The exit code when `faulte` cannot do what the person asked for.
///
/// The issue gives this code to a source that failed and to output that the
/// parser refused. `faulte` prints the reason and never an empty ranking.
/// `faulte kill` gives the same code when it signalled a session and the
/// stop did not do what the report of the plan said that it would do.
#[cfg(target_os = "macos")]
const EXIT_ERROR: u8 = 2;

/// The exit code when `faulte kill` has candidates and no person to ask.
///
/// The issue states this code. A run through a pipe stops nothing, and it did
/// not do the work that the command line asked for, so it is not a success.
#[cfg(target_os = "macos")]
const EXIT_NOT_A_TERMINAL: u8 = 1;

/// The name that each message on the error output starts with.
#[cfg(target_os = "macos")]
const TOOL: &str = "faulte";

/// The variable that states the width of the terminal, in columns.
///
/// POSIX says a value here overrides the width that the system selects, and
/// `ls`, `git` and `less` obey that rule.
#[cfg(target_os = "macos")]
const WIDTH_VARIABLE: &str = "COLUMNS";

/// What `faulte kill` says when the plan names no session.
#[cfg(target_os = "macos")]
const STOPS_NOTHING: &str = "faulte stops nothing.";

/// What `faulte kill` says when it has candidates and no person to ask.
#[cfg(target_os = "macos")]
const NOT_A_TERMINAL: &str =
    "faulte asks nothing, because the input is not a terminal. faulte stopped nothing.";

/// What `faulte kill` says when the answer to the question is not a
/// confirmation.
#[cfg(target_os = "macos")]
const STOPPED_NOTHING: &str = "faulte stopped nothing.";

/// What `faulte kill` says when it cannot write the question out.
#[cfg(target_os = "macos")]
const NO_QUESTION: &str = "the question did not reach the output, so faulte stopped nothing.";

/// The time that `faulte kill` gives a session between `SIGTERM` and
/// `SIGKILL`.
///
/// The issue states 30 seconds, and the Mac that this tool is for is the
/// reason. A Mac that is short of memory is slow to page a process in, and a
/// process handles no signal until it is in memory. A shorter grace period
/// sends `SIGKILL` to a session that was on its way to closing its transcript.
#[cfg(target_os = "macos")]
const GRACE: Duration = Duration::from_secs(30);

/// The time between two reads of the process table inside [`GRACE`].
///
/// One second is short against the grace period, so a session that stopped
/// early ends the wait early. It is long against a read of the table, so the
/// waiting costs almost nothing.
#[cfg(target_os = "macos")]
const POLL: Duration = Duration::from_secs(1);

/// The command line.
#[derive(Parser)]
#[command(name = "faulte", version = version_string!(), about = ABOUT)]
struct Cli {
    /// The time to sample the page faults of each process, for example 5s, 10m,
    /// or 2h. A bare number is a number of seconds.
    #[arg(long, global = true, value_name = DURATION_VALUE, default_value = DEFAULT_INTERVAL)]
    interval: Span,

    /// The number of processes that the ranking shows. One line gives the count
    /// of the other processes.
    ///
    /// The flag belongs to the ranking alone. `faulte kill` shows every
    /// candidate of its plan, and `--max` is what limits that plan.
    #[arg(long, value_name = COUNT_VALUE, default_value_t = DEFAULT_LIMIT)]
    limit: usize,

    /// With no command, `faulte` ranks the processes.
    #[command(subcommand)]
    command: Option<Command>,
}

/// The commands other than the ranking.
#[derive(Subcommand)]
enum Command {
    /// Stop the old idle Claude Code sessions. faulte shows the plan and asks
    /// before it stops a session.
    Kill {
        /// Select a session only when it is older than this time.
        #[arg(long, value_name = DURATION_VALUE, default_value = DEFAULT_OLDER_THAN)]
        older_than: Span,

        /// Select a session only when it became idle more than this time ago.
        #[arg(long, value_name = DURATION_VALUE, default_value = DEFAULT_IDLE_FOR)]
        idle_for: Span,

        /// Stop no more than N sessions. faulte keeps the N oldest.
        #[arg(long, value_name = COUNT_VALUE)]
        max: Option<usize>,
    },
}

fn main() -> ExitCode {
    run(Cli::parse())
}

/// Runs the command that the person gave.
#[cfg(target_os = "macos")]
fn run(cli: Cli) -> ExitCode {
    let Cli {
        interval,
        limit,
        command,
    } = cli;
    match command {
        None => rank(interval, limit),
        Some(Command::Kill {
            older_than,
            idle_for,
            max,
        }) => kill(
            interval,
            Rules {
                older_than,
                idle_for,
                max,
            },
        ),
    }
}

/// Ranks the processes of this Mac and prints the ranking.
///
/// The header comes first, then a blank line, then the table. The table takes
/// the width that `COLUMNS` states, or the width of the window that it prints
/// into, or a default. A run through a pipe therefore stays bounded: a table
/// of no width is as wide as its widest cell, and one command line of this Mac
/// carries more than 3,000 characters.
#[cfg(target_os = "macos")]
fn rank(interval: Span, limit: usize) -> ExitCode {
    let observed = match observe(&Mac::new(), interval) {
        Ok(observed) => observed,
        Err(error) => return failed(&error),
    };
    for line in render::header(&observed.measurement()) {
        println!("{line}");
    }
    println!();
    println!(
        "{}",
        render::rows(
            &observed.ranking,
            &observed.ranking.rows,
            Some(limit),
            &observed.accounts,
            observed.now,
            Some(table_width()),
        )
    );
    ExitCode::SUCCESS
}

/// Stops the old idle Claude Code sessions of this Mac.
///
/// The order of the steps is the design, and every rule is in the library:
///
/// 1. One read of this Mac, the same read that the ranking makes. The plan
///    shows the same columns, and it judges the process table that the read
///    ranked.
/// 2. The plan, and the text of the plan on the standard output. A person
///    reads it before anything gets a signal.
/// 3. The decision, which the count of the candidates and the input make.
/// 4. The question, on the same line, and one line of the input as the answer.
/// 5. The stop sequence, and the report of what it did.
///
/// The exit code is 0 for a run that stopped every session that it signalled,
/// and for a run that stopped nothing because the person said so. It is 1 when
/// the plan names a session and no person can answer the question. It is 2
/// when a source of this Mac failed, and when the stop left a session that is
/// still the same process after `SIGKILL` or that `faulte` could not signal or
/// could not read again, because such a run did not do what the plan said that
/// it would do. [`stop::StopReport::did_what_the_plan_said`] holds that rule.
#[cfg(target_os = "macos")]
fn kill(interval: Span, rules: Rules) -> ExitCode {
    let machine = Mac::new();
    let observed = match observe(&machine, interval) {
        Ok(observed) => observed,
        Err(error) => return failed(&error),
    };
    let plan = plan::plan(&PlanInput {
        ranking: &observed.ranking,
        table: &observed.table,
        rules,
        faulte: Pid::new(std::process::id()),
        now: observed.now,
    });
    println!(
        "{}",
        render::plan(
            &plan,
            &observed.ranking,
            &observed.accounts,
            observed.now,
            Some(table_width()),
        )
    );
    match stop::decide(plan.candidates.len(), io::stdin().is_terminal()) {
        Decision::NothingToStop => {
            println!("{STOPS_NOTHING}");
            ExitCode::SUCCESS
        }
        Decision::NotATerminal => {
            eprintln!("{NOT_A_TERMINAL}");
            ExitCode::from(EXIT_NOT_A_TERMINAL)
        }
        Decision::Ask => ask_and_stop(&machine, &plan.candidates),
    }
}

/// Asks the question, reads one answer, and stops the sessions of
/// `candidates` when the answer confirms.
///
/// The question ends with a space and no line break, so the output must reach
/// the terminal before the read of the answer starts. A question that stays in
/// the buffer is a question that nobody read, and the answer to it would
/// confirm a stop that the person never saw.
#[cfg(target_os = "macos")]
fn ask_and_stop(machine: &Mac, candidates: &[Candidate]) -> ExitCode {
    print!("{}", render::question(candidates.len()));
    if io::stdout().flush().is_err() {
        eprintln!("{TOOL}: {NO_QUESTION}");
        return ExitCode::from(EXIT_ERROR);
    }
    let mut answer = String::new();
    // The end of the input and a read that failed are both no answer. A stop
    // is not reversible, so neither one confirms anything.
    let read = io::stdin().lock().read_line(&mut answer);
    let answer = match read {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(answer.as_str()),
    };
    if !stop::confirms(answer) {
        println!("{STOPPED_NOTHING}");
        return ExitCode::SUCCESS;
    }
    let report = stop::stop(machine, candidates, GRACE, POLL);
    println!("{}", render::stopped(&report));
    if report.did_what_the_plan_said() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_ERROR)
    }
}

/// Prints `error` on the error output and gives the exit code of a source that
/// failed.
///
/// A ranking that is empty because a source failed looks the same as a Mac
/// that does nothing, and a plan that is empty for the same reason looks the
/// same as a Mac with no old session. Thus a failed source prints the reason
/// and ends the run.
#[cfg(target_os = "macos")]
fn failed(error: &MachineError) -> ExitCode {
    eprintln!("{TOOL}: {error}");
    ExitCode::from(EXIT_ERROR)
}

/// Gives the width that a table of this run takes.
///
/// `COLUMNS` wins, then the window that the output prints into, then a
/// default. The library holds that rule, and this function is the one place
/// that reads the two sources of it.
#[cfg(target_os = "macos")]
fn table_width() -> u16 {
    render::table_width(
        std::env::var(WIDTH_VARIABLE).ok().as_deref(),
        termsize::controlling_columns(),
    )
}

/// Says that `faulte` supports macOS only.
///
/// The fault sampler runs `/usr/bin/top` of macOS, and the counters of the
/// system come from `host_statistics64` of macOS. No other platform has them.
#[cfg(not(target_os = "macos"))]
fn run(_cli: Cli) -> ExitCode {
    eprintln!("{UNSUPPORTED_PLATFORM}");
    ExitCode::from(EXIT_UNSUPPORTED)
}
