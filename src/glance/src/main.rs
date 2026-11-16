//! `glance` — a light macOS menu bar monitor of the CPU share and the memory
//! pressure.

#![deny(unsafe_code)]

use std::io::{self, Write};
use std::process::ExitCode;

use buildinfo::version_string;
use clap::Parser;
use glance::interval::Interval;
#[cfg(target_os = "macos")]
use glance::reader::Reader;
#[cfg(target_os = "macos")]
use glance::sample::{Reading, Sample};
#[cfg(target_os = "macos")]
use glance::text::{error_json, sample_json};

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

/// The exit code of a failed write of the stream.
#[cfg(target_os = "macos")]
const EXIT_WRITE_FAILED: u8 = 1;

/// The command line of `glance`.
#[derive(Parser)]
#[command(name = "glance", version = version_string!(), about = ABOUT)]
struct Cli {
    /// The time between two samples, in seconds, at least 0.25.
    #[arg(long, value_name = "SECONDS", default_value_t = Interval::DEFAULT)]
    interval: Interval,

    /// Take two readings one interval apart, print one JSON object, and stop.
    #[arg(long)]
    sample: bool,

    /// Print one JSON line for each interval until the standard output closes.
    #[arg(long, conflicts_with = "sample")]
    stream: bool,

    /// Stop the stream after N lines.
    #[arg(long, value_name = "N", requires = "stream")]
    count: Option<u64>,
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
    if cli.stream {
        return stream(cli.interval, cli.count);
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
            let sample = Sample::between(&first, &second);
            println!("{}", sample_json(reader.host_name(), &sample));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("glance: {error}");
            ExitCode::from(EXIT_READ_FAILED)
        }
    }
}

/// Prints one JSON line for each interval, at most `count` lines.
///
/// The stream takes a reading at the start. After each interval, it takes a
/// reading again and prints the sample between the last two good readings. A
/// failed read prints an error line, and the stream continues. A closed
/// standard output stops the stream with status 0 and no text.
#[cfg(target_os = "macos")]
fn stream(interval: Interval, count: Option<u64>) -> ExitCode {
    let reader = Reader::new();
    let host = reader.host_name();
    let mut previous: Option<Reading> = None;
    let mut written: u64 = 0;
    let mut first = true;
    loop {
        if count.is_some_and(|limit| written >= limit) {
            return ExitCode::SUCCESS;
        }
        let line = if first {
            first = false;
            match reader.read() {
                Ok(reading) => {
                    previous = Some(reading);
                    None
                }
                Err(error) => Some(error_json(host, &error.to_string())),
            }
        } else {
            std::thread::sleep(interval.duration());
            match reader.read() {
                Ok(reading) => {
                    let line = previous
                        .as_ref()
                        .map(|earlier| sample_json(host, &Sample::between(earlier, &reading)));
                    previous = Some(reading);
                    line
                }
                Err(error) => Some(error_json(host, &error.to_string())),
            }
        };
        if let Some(line) = line {
            if let Err(error) = write_line(&line) {
                return end_of_failed_write(&error);
            }
            written += 1;
        }
    }
}

/// Writes one line to the standard output and flushes it at once.
#[cfg(target_os = "macos")]
fn write_line(line: &str) -> io::Result<()> {
    let mut out = io::stdout().lock();
    writeln!(out, "{line}")?;
    out.flush()
}

/// Gives the exit code for a failed write of the stream.
///
/// A closed pipe is the normal end of a stream, so it prints nothing.
#[cfg(target_os = "macos")]
fn end_of_failed_write(error: &io::Error) -> ExitCode {
    if error.kind() == io::ErrorKind::BrokenPipe {
        return ExitCode::SUCCESS;
    }
    eprintln!("glance: cannot write the stream: {error}");
    ExitCode::from(EXIT_WRITE_FAILED)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_interval_is_the_constant_of_the_interval_module() {
        let cli = Cli::try_parse_from(["glance"]).unwrap();
        assert_eq!(cli.interval, Interval::DEFAULT);
    }
}
