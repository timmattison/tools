//! `glance` — a light macOS menu bar monitor of the CPU share and the memory
//! pressure.

#![deny(unsafe_code)]

use std::process::ExitCode;

use buildinfo::version_string;
use clap::Parser;
use glance::interval::Interval;
#[cfg(target_os = "macos")]
use glance::reader::Reader;
#[cfg(target_os = "macos")]
use glance::sample::Sample;
#[cfg(target_os = "macos")]
use glance::text::sample_json;

/// The one-line description that `--help` shows.
const ABOUT: &str = "A light menu bar monitor of the CPU share and the memory pressure";

/// The text that `glance` gives on a platform other than macOS.
#[cfg(not(target_os = "macos"))]
const UNSUPPORTED_PLATFORM: &str = "glance supports macOS only.";

/// The exit code on a platform other than macOS.
#[cfg(not(target_os = "macos"))]
const EXIT_UNSUPPORTED: u8 = 1;

/// The exit code of a failed read of the kernel.
#[cfg(target_os = "macos")]
const EXIT_READ_FAILED: u8 = 1;

/// The command line of `glance`.
#[derive(Parser)]
#[command(name = "glance", version = version_string!(), about = ABOUT)]
struct Cli {
    /// The time between two samples, in seconds, at least 0.25.
    #[arg(long, value_name = "SECONDS", default_value = "1")]
    interval: Interval,

    /// Take two readings one interval apart, print one JSON object, and stop.
    #[arg(long)]
    sample: bool,
}

fn main() -> ExitCode {
    run(Cli::parse())
}

/// Runs `glance` on macOS.
#[cfg(target_os = "macos")]
fn run(cli: Cli) -> ExitCode {
    if cli.sample {
        return sample(cli.interval);
    }
    glance::shell::run(cli.interval)
}

/// Takes two readings one interval apart and prints the sample as JSON.
#[cfg(target_os = "macos")]
fn sample(interval: Interval) -> ExitCode {
    let reader = Reader::new();
    let readings = reader.read().and_then(|first| {
        std::thread::sleep(interval.duration());
        reader.read().map(|second| (first, second))
    });
    match readings {
        Ok((first, second)) => {
            println!("{}", sample_json(&Sample::between(&first, &second)));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("glance: {error}");
            ExitCode::from(EXIT_READ_FAILED)
        }
    }
}

/// Refuses to run on a platform other than macOS.
///
/// The refusal comes after clap parses the command line, so `--version` and
/// `--help` work on every platform.
#[cfg(not(target_os = "macos"))]
fn run(_cli: Cli) -> ExitCode {
    eprintln!("{UNSUPPORTED_PLATFORM}");
    ExitCode::from(EXIT_UNSUPPORTED)
}
