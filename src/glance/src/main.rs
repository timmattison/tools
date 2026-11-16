//! `glance` — a light macOS menu bar monitor of the CPU share and the memory
//! pressure.

#![deny(unsafe_code)]

#[cfg(target_os = "macos")]
use std::io::{self, Write};
use std::num::NonZeroU32;
use std::process::ExitCode;

use buildinfo::version_string;
use clap::Parser;
use glance::connection::{Destination, RemoteBinary, DEFAULT_REMOTE_BINARY};
#[cfg(target_os = "macos")]
use glance::connection::{HostNaming, Remote};
use glance::flags;
use glance::interval::Interval;
#[cfg(target_os = "macos")]
use glance::launch::{self, Mode, Settings};
#[cfg(target_os = "macos")]
use glance::reader::Reader;
#[cfg(target_os = "macos")]
use glance::sample::{Reading, Sample};
#[cfg(target_os = "macos")]
use glance::text::{error_json, sample_json};

/// The one-line description that `--help` shows.
const ABOUT: &str = "A light menu bar monitor of the CPU share and the memory pressure";

/// The text that `--help` shows after the options.
const AFTER_HELP: &str = "\
Each target has one status item: this Mac, and each destination of --host.
When the status item of the target runs already, glance stops it and starts
no new one. Thus a second run of the same command closes the status item.
A new status item runs in the background, and glance returns to the shell
when it is up. -f keeps the status item in the foreground.
--sample and --stream never stop a status item.";

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
#[command(name = "glance", version = version_string!(), about = ABOUT, after_help = AFTER_HELP)]
struct Cli {
    /// The time between two samples, in seconds, at least 0.25.
    #[arg(long = flags::INTERVAL, value_name = "SECONDS", default_value_t = Interval::DEFAULT)]
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

    /// Show the charts of a second Mac. glance opens one ssh connection to
    /// DESTINATION and runs `glance --stream` there. The status item names
    /// the Mac DESTINATION, as you typed it.
    #[arg(
        long = flags::HOST,
        value_name = "DESTINATION",
        conflicts_with_all = ["stream", "sample"]
    )]
    host: Option<Destination>,

    /// The path of glance on the second Mac. A leading ~/ names the home
    /// directory of the remote user, so put quotes around it.
    #[arg(
        long = flags::REMOTE_BINARY,
        value_name = "PATH",
        requires = "host",
        default_value = DEFAULT_REMOTE_BINARY
    )]
    remote_binary: RemoteBinary,

    /// Show the name that the second Mac reports, as `hostname -s` gives it
    /// there, in place of DESTINATION. Before the first line arrives, the
    /// status item shows DESTINATION. Needs --host.
    #[arg(long = flags::RESOLVE, requires = "host")]
    resolve: bool,

    /// Run the status item in this process, which keeps the terminal until
    /// the status item stops. Give it to a LaunchAgent. With no -f, glance
    /// starts the status item in the background and returns to the shell.
    #[arg(short, long, conflicts_with_all = ["stream", "sample"])]
    foreground: bool,

    /// The mark of the copy that a background start makes. A person never
    /// gives it.
    #[arg(
        long = flags::SPAWNED_COPY,
        hide = true,
        conflicts_with_all = ["stream", "sample", "foreground"]
    )]
    spawned_copy: bool,

    /// End the status item this many seconds after it is up. The tests give
    /// it to each status item that they start, so that a status item that a
    /// failed test leaves behind ends by itself.
    #[arg(long = flags::EXIT_AFTER, hide = true, value_name = "SECONDS")]
    exit_after: Option<NonZeroU32>,
}

#[cfg(target_os = "macos")]
impl Cli {
    /// The second Mac of this command line, or `None` for this Mac.
    fn remote(&self) -> Option<Remote> {
        self.host.clone().map(|destination| Remote {
            destination,
            remote_binary: self.remote_binary.clone(),
            naming: if self.resolve {
                HostNaming::Reported
            } else {
                HostNaming::Destination
            },
        })
    }
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
    let remote = cli.remote();
    let mode = if cli.spawned_copy {
        Mode::SpawnedCopy
    } else if cli.foreground {
        Mode::Foreground
    } else {
        Mode::Background
    };
    launch::run(
        mode,
        Settings {
            interval: cli.interval,
            remote,
            exit_after: cli.exit_after,
        },
    )
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

    #[test]
    fn host_with_no_remote_binary_takes_the_default_of_the_connection_module() {
        let cli = Cli::try_parse_from(["glance", "--host", "m1-ts"]).unwrap();
        assert_eq!(cli.host.as_ref().map(Destination::as_str), Some("m1-ts"));
        assert_eq!(cli.remote_binary, RemoteBinary::default());
    }

    #[test]
    fn host_takes_the_remote_binary_that_the_user_gives() {
        let cli = Cli::try_parse_from([
            "glance",
            "--host",
            "m1-ts",
            "--remote-binary",
            "~/bin/glance",
        ])
        .unwrap();
        assert_eq!(cli.remote_binary.as_str(), "~/bin/glance");
    }

    /// Parses the arguments that a background start gives to its copy of
    /// `settings`, as the copy parses them.
    #[cfg(target_os = "macos")]
    fn parse_child(settings: &Settings) -> Cli {
        let arguments = launch::child_arguments(settings);
        Cli::try_parse_from(std::iter::once("glance".to_owned()).chain(arguments))
            .expect("the copy parses its arguments")
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_copy_of_a_local_start_gets_the_same_settings() {
        let interval: Interval = "0.25".parse().unwrap();
        let cli = parse_child(&Settings {
            interval,
            remote: None,
            exit_after: None,
        });
        assert!(cli.spawned_copy);
        assert!(!cli.foreground);
        assert_eq!(cli.interval, interval);
        assert_eq!(cli.host, None);
        assert_eq!(cli.exit_after, None);
        assert_eq!(cli.remote(), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_copy_of_a_host_start_gets_the_same_settings() {
        let remote = Remote {
            destination: "tim@m1-ts".parse().unwrap(),
            remote_binary: "-odd bin/glance".parse().unwrap(),
            naming: HostNaming::Destination,
        };
        let cli = parse_child(&Settings {
            interval: Interval::DEFAULT,
            remote: Some(remote.clone()),
            exit_after: NonZeroU32::new(7),
        });
        assert!(cli.spawned_copy);
        assert_eq!(cli.host.as_ref(), Some(&remote.destination));
        assert_eq!(cli.remote_binary, remote.remote_binary);
        assert_eq!(cli.exit_after, NonZeroU32::new(7));
        assert_eq!(cli.remote(), Some(remote));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_copy_of_a_resolve_start_gets_the_same_settings() {
        let remote = Remote {
            destination: "tim@m1-ts".parse().unwrap(),
            remote_binary: "-odd bin/glance".parse().unwrap(),
            naming: HostNaming::Reported,
        };
        let cli = parse_child(&Settings {
            interval: Interval::DEFAULT,
            remote: Some(remote.clone()),
            exit_after: NonZeroU32::new(7),
        });
        assert_eq!(cli.remote(), Some(remote));
    }

    #[test]
    fn foreground_with_stream_is_a_usage_error() {
        let refused = Cli::try_parse_from(["glance", "--foreground", "--stream"]);
        assert!(refused.is_err(), "the parse took --foreground --stream");
    }

    #[test]
    fn foreground_with_the_mark_of_the_copy_is_a_usage_error() {
        let refused = Cli::try_parse_from(["glance", "--foreground", "--spawned-copy"]);
        assert!(
            refused.is_err(),
            "the parse took --foreground --spawned-copy"
        );
    }
}
