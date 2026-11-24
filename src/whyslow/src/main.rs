//! `whyslow` — "why slow": one sample of this Mac, and a short summary of the
//! likely cause when it is slow.

use std::process::ExitCode;

use buildinfo::version_string;
use clap::Parser;
use faulte::duration::Span;
#[cfg(target_os = "macos")]
use whyslow::machine::macos::Mac;
#[cfg(target_os = "macos")]
use whyslow::report::report;

/// The one-line description that `--help` shows.
const ABOUT: &str = "Why slow: one sample of this Mac, and a short summary of the likely cause";

/// The default time of the sample.
const DEFAULT_INTERVAL: &str = "1s";

/// The name that `--help` gives to the value of the interval flag.
const DURATION_VALUE: &str = "DURATION";

/// The text that `whyslow` gives on a platform other than macOS.
#[cfg(not(target_os = "macos"))]
const UNSUPPORTED_PLATFORM: &str = "whyslow supports macOS only.";

/// The exit code on a platform other than macOS.
#[cfg(not(target_os = "macos"))]
const EXIT_UNSUPPORTED: u8 = 1;

/// The command line.
#[derive(Parser)]
#[command(name = "whyslow", version = version_string!(), about = ABOUT)]
struct Cli {
    /// The time between the two samples of the process table, for example 1s,
    /// 5s, or 2m. A bare number is a number of seconds.
    // A value that starts with a minus sign goes to the parser of the duration,
    // so that the refusal of `-1` names the flag.
    #[arg(long, value_name = DURATION_VALUE, default_value = DEFAULT_INTERVAL, allow_negative_numbers = true)]
    interval: Span,
}

fn main() -> ExitCode {
    run(&Cli::parse())
}

/// Reads this Mac, prints the summary, and gives the exit code of the run.
///
/// A source that fails gives its reason on the error output and the code 2.
#[cfg(target_os = "macos")]
fn run(cli: &Cli) -> ExitCode {
    let outcome = report(&Mac::new(), cli.interval);
    for line in &outcome.stdout {
        println!("{line}");
    }
    if let Some(message) = &outcome.stderr {
        eprintln!("{message}");
    }
    ExitCode::from(outcome.code)
}

/// Says that `whyslow` supports macOS only.
///
/// The process table, the CPU ticks, and the footprint of each process come
/// from tools and calls of macOS. No other platform has them.
#[cfg(not(target_os = "macos"))]
fn run(_cli: &Cli) -> ExitCode {
    eprintln!("{UNSUPPORTED_PLATFORM}");
    ExitCode::from(EXIT_UNSUPPORTED)
}
