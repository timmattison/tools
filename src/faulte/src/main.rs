//! `faulte` — "fault rate": rank the processes of this Mac by page faults per
//! second, and stop the old idle Claude Code sessions.

use std::process::ExitCode;

use buildinfo::version_string;
use clap::{Parser, Subcommand};
use faulte::duration::Span;

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

/// The text that a command gives when it is not implemented yet.
#[cfg(target_os = "macos")]
const NOT_IMPLEMENTED: &str = "this command is not implemented yet.";

/// The exit code of a command that is not implemented yet.
#[cfg(target_os = "macos")]
const EXIT_NOT_IMPLEMENTED: u8 = 2;

/// The command line. Only macOS reads the options, so another platform does
/// not read the fields.
#[derive(Parser)]
#[command(name = "faulte", version = version_string!(), about = ABOUT)]
#[cfg_attr(
    not(target_os = "macos"),
    allow(dead_code, reason = "only the macOS build reads the options")
)]
struct Cli {
    /// The time to sample the page faults of each process, for example 5s, 10m,
    /// or 2h. A bare number is a number of seconds.
    #[arg(long, global = true, value_name = DURATION_VALUE, default_value = DEFAULT_INTERVAL)]
    interval: Span,

    /// The number of processes that the ranking shows. One line gives the count
    /// of the other processes.
    #[arg(long, global = true, value_name = COUNT_VALUE, default_value_t = DEFAULT_LIMIT)]
    limit: usize,

    /// With no command, `faulte` ranks the processes.
    #[command(subcommand)]
    command: Option<Command>,
}

/// The commands other than the ranking.
#[derive(Subcommand)]
#[cfg_attr(
    not(target_os = "macos"),
    allow(dead_code, reason = "only the macOS build reads the options")
)]
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
    let sample = format!("--interval {interval} --limit {limit}");
    match command {
        None => not_implemented(&format!("faulte {sample}")),
        Some(Command::Kill {
            older_than,
            idle_for,
            max,
        }) => {
            let max = max.map(|count| format!(" --max {count}")).unwrap_or_default();
            not_implemented(&format!(
                "faulte kill --older-than {older_than} --idle-for {idle_for}{max} {sample}"
            ))
        }
    }
}

/// Says that `command_line` names a command that is not implemented yet.
///
/// The command line gives each value that the parser read, so a person can see
/// what the command received.
#[cfg(target_os = "macos")]
fn not_implemented(command_line: &str) -> ExitCode {
    eprintln!("{command_line}: {NOT_IMPLEMENTED}");
    ExitCode::from(EXIT_NOT_IMPLEMENTED)
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
