//! `glance` — a light macOS menu bar monitor of the CPU share and the memory
//! pressure.

#![deny(unsafe_code)]

use std::process::ExitCode;

use buildinfo::version_string;
use clap::Parser;

/// The one-line description that `--help` shows.
const ABOUT: &str = "A light menu bar monitor of the CPU share and the memory pressure";

/// The text that `glance` gives on a platform other than macOS.
#[cfg(not(target_os = "macos"))]
const UNSUPPORTED_PLATFORM: &str = "glance supports macOS only.";

/// The exit code on a platform other than macOS.
#[cfg(not(target_os = "macos"))]
const EXIT_UNSUPPORTED: u8 = 1;

/// The command line of `glance`.
#[derive(Parser)]
#[command(name = "glance", version = version_string!(), about = ABOUT)]
struct Cli {}

fn main() -> ExitCode {
    let _cli = Cli::parse();
    run()
}

/// Runs `glance` on macOS.
#[cfg(target_os = "macos")]
fn run() -> ExitCode {
    ExitCode::SUCCESS
}

/// Refuses to run on a platform other than macOS.
///
/// The refusal comes after clap parses the command line, so `--version` and
/// `--help` work on every platform.
#[cfg(not(target_os = "macos"))]
fn run() -> ExitCode {
    eprintln!("{UNSUPPORTED_PLATFORM}");
    ExitCode::from(EXIT_UNSUPPORTED)
}
