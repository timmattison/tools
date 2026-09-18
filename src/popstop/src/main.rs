//! `popstop` — keeps the default audio output device awake.
//!
//! The binary parses its command line on every platform, so `--help` and
//! `--version` work everywhere. It plays audio on macOS only. On another
//! platform it says so and exits with status 1.

use clap::Parser;
use std::path::PathBuf;
use std::process::ExitCode;

/// The command line of `popstop`.
#[derive(Parser)]
#[command(
    name = "popstop",
    version = buildinfo::version_string!(),
    about = "Keeps the default audio output device awake with a signal that is too small to hear, so USB speakers do not pop",
    after_help = popstop::exit_status::help_section()
)]
struct Cli {
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

/// Runs the command line on macOS.
#[cfg(target_os = "macos")]
fn run(cli: &Cli) -> ExitCode {
    use std::io::Write;
    use std::time::Duration;

    use popstop::life_cycle::{run_foreground, Settings};

    let settings = Settings {
        state_dir: cli.state_dir.clone(),
        exit_after: cli.exit_after.map(Duration::from_secs),
    };
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

    let _ = (&cli.state_dir, &cli.exit_after);
    let _ = writeln!(
        std::io::stderr(),
        "{}",
        popstop::message::problem_line(&"popstop runs on macOS only")
    );
    ExitCode::from(popstop::exit_status::ERROR)
}
