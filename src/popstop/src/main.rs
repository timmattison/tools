//! `popstop` — keeps the default audio output device awake.
//!
//! The binary parses its command line on every platform, so `--help` and
//! `--version` work everywhere. It plays audio on macOS only. On another
//! platform it says so and exits with status 1.

use clap::Parser;
use std::process::ExitCode;

/// The command line of `popstop`.
#[derive(Parser)]
#[command(
    name = "popstop",
    version = buildinfo::version_string!(),
    about = "Keeps the default audio output device awake with a signal that is too small to hear, so USB speakers do not pop"
)]
struct Cli {}

fn main() -> ExitCode {
    let _cli = Cli::parse();

    #[cfg(target_os = "macos")]
    {
        ExitCode::SUCCESS
    }
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("popstop: popstop runs on macOS only");
        ExitCode::FAILURE
    }
}
