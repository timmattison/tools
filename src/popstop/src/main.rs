//! `popstop` — keeps the default audio output device awake.
//!
//! The binary parses its command line on every platform, so `--help` and
//! `--version` work everywhere. It plays audio on macOS only. On another
//! platform it says so and exits with status 1.

use clap::Parser;
use std::path::PathBuf;
use std::process::ExitCode;

/// The name of the group of flags that act instead of a start. One of them
/// at most can be there, because each one asks popstop for another thing.
const ACTION_GROUP: &str = "action";

/// The command line of `popstop`.
#[derive(Parser)]
#[command(
    name = "popstop",
    version = buildinfo::version_string!(),
    about = "Keeps the default audio output device awake with a signal that is too small to hear, so USB speakers do not pop",
    after_help = popstop::exit_status::help_section()
)]
struct Cli {
    /// Start a copy that has no terminal, and then return
    #[arg(long, group = ACTION_GROUP)]
    background: bool,
    /// Run the life cycle as the copy that `--background` started, and report
    /// to that command through stdout. `--background` passes this flag to the
    /// copy that it starts, and a user never needs it.
    #[arg(long, hide = true, group = ACTION_GROUP)]
    background_child: bool,
    /// Stop the copy that runs
    #[arg(long, group = ACTION_GROUP)]
    stop: bool,
    /// Show the copy that runs
    #[arg(long, group = ACTION_GROUP)]
    status: bool,
    /// The directory that holds the lock file and the log. The tests give
    /// each copy a directory of its own.
    #[arg(long, hide = true, value_name = "PATH")]
    state_dir: Option<PathBuf>,
    /// Stop after this number of seconds, as if a signal arrived. The tests
    /// give each copy a bound, so a test that fails leaves no copy that plays
    /// for ever.
    #[arg(long, hide = true, value_name = "SECONDS", value_parser = clap::value_parser!(u64).range(1..))]
    exit_after: Option<u64>,
}

fn main() -> ExitCode {
    run(&Cli::parse())
}

/// Writes the answer of a command that acts on the copy that runs, and gives
/// the exit status of popstop.
///
/// A report goes to stdout, because it is the answer to the question that the
/// user asked. A failure goes to stderr.
#[cfg(target_os = "macos")]
fn report(answer: Result<popstop::control::Report, popstop::life_cycle::Failure>) -> ExitCode {
    use std::io::Write;

    match answer {
        // A write that fails has no other place to report.
        Ok(report) => {
            let _ = writeln!(std::io::stdout(), "{}", report.text());
            ExitCode::from(report.status())
        }
        Err(failure) => {
            let _ = writeln!(std::io::stderr(), "{}", failure.message());
            ExitCode::from(failure.status())
        }
    }
}

/// Runs the command line on macOS.
#[cfg(target_os = "macos")]
fn run(cli: &Cli) -> ExitCode {
    use std::io::Write;
    use std::time::Duration;

    use popstop::control;
    use popstop::life_cycle::{run_foreground, Settings};

    let settings = Settings {
        state_dir: cli.state_dir.clone(),
        exit_after: cli.exit_after.map(Duration::from_secs),
    };
    if cli.background {
        return report(popstop::background::start(&settings));
    }
    if cli.background_child {
        // The stderr of this copy is its log, thus the reason of a failure
        // stays for the user to read after the start that made the copy ended.
        return match popstop::background::run_child(&settings) {
            Ok(()) => ExitCode::from(popstop::exit_status::SUCCESS),
            Err(failure) => {
                // A write to stderr that fails has no other place to report.
                let _ = writeln!(std::io::stderr(), "{}", failure.message());
                ExitCode::from(failure.status())
            }
        };
    }
    if cli.stop {
        return report(control::stop(&settings));
    }
    if cli.status {
        return report(control::status(&settings));
    }
    match run_foreground(&settings) {
        Ok(()) => ExitCode::from(popstop::exit_status::SUCCESS),
        Err(failure) => {
            // A write to stderr that fails has no other place to report.
            let _ = writeln!(std::io::stderr(), "{}", failure.message());
            ExitCode::from(failure.status())
        }
    }
}

/// Says that popstop runs on macOS only.
#[cfg(not(target_os = "macos"))]
fn run(cli: &Cli) -> ExitCode {
    use std::io::Write;

    let _ = (
        &cli.background,
        &cli.background_child,
        &cli.stop,
        &cli.status,
        &cli.state_dir,
        &cli.exit_after,
    );
    let _ = writeln!(
        std::io::stderr(),
        "{}",
        popstop::message::problem_line(&"popstop runs on macOS only")
    );
    ExitCode::from(popstop::exit_status::ERROR)
}
