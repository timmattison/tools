//! `prgz` (Progress Gzip): compress one file with gzip.
//!
//! This file holds the three parts of the tool that need a process of their
//! own: the command line, the progress bar, and the answer to a stop signal.
//! The library beside it holds the compression and the closing report.
//!
//! A run that the user stops leaves no output file behind. The Go tool that
//! this binary replaces sent Ctrl-C to the quit path of its terminal library,
//! thus it left a part of a gzip stream on the disk. A part of a gzip stream
//! looks like a whole one, thus the user got a broken file.

#![warn(clippy::unwrap_used)]
#![warn(clippy::expect_used)]

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use buildinfo::version_string;
use clap::{CommandFactory, Parser};
use indicatif::ProgressBar;
use prgz::{compress_file, default_output_path, format_report, CompressError, Locale, Stats};
use termbar::{ProgressStyleBuilder, TerminalWidth};
use thiserror::Error;

/// The name of the tool. It starts every line that reports a failure.
const PROGRAM_NAME: &str = "prgz";

/// The locale that sets the separators of every number of the report.
const REPORT_LOCALE: Locale = Locale::en;

/// The text that joins an error to the cause under it.
const CAUSE_SEPARATOR: &str = ": ";

/// The length that the progress bar takes for an input that the run cannot
/// measure. A bar of this length shows the count of the read bytes and no
/// percentage, because the run does not know the whole yet.
const UNKNOWN_LENGTH: u64 = 0;

/// Compress one file with gzip and show the progress of the run.
#[derive(Parser)]
#[command(name = PROGRAM_NAME, version = version_string!(), about, long_about = None)]
struct Cli {
    /// The file to compress
    #[arg(long, value_name = "PATH")]
    input: Option<PathBuf>,

    /// The file to write. A run that gets no output name adds `.gz` to the
    /// input name
    #[arg(long, value_name = "PATH")]
    output: Option<PathBuf>,
}

/// Why a run of the tool stopped short.
#[derive(Debug, Error)]
enum RunError {
    /// The process could not ask the system for the stop signals.
    #[error("could not listen for the stop signals")]
    Signal(#[source] io::Error),
    /// The compression stopped short.
    #[error(transparent)]
    Compress(#[from] CompressError),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let Some(input) = cli.input else {
        // The Go tool that this binary replaces prints its usage text and
        // exits with a status of 1 when the command line names no input.
        eprint!("{}", Cli::command().render_long_help());
        return ExitCode::FAILURE;
    };
    let output = cli
        .output
        .unwrap_or_else(|| default_output_path(input.as_path()));
    match compress_with_progress(input.as_path(), output.as_path()) {
        Ok(stats) => {
            println!("{}", format_report(&stats, &REPORT_LOCALE));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{PROGRAM_NAME}{CAUSE_SEPARATOR}{}", chain(&error));
            ExitCode::FAILURE
        }
    }
}

/// Compress the input into the output and draw a progress bar while the run
/// goes on.
///
/// The bar goes away when the run ends, whether the run was a success or not.
/// The library removes a part of an output file, thus a run that stops short
/// leaves the disk as it found it.
///
/// # Errors
///
/// Returns [`RunError::Signal`] when the system refuses the stop signals, and
/// [`RunError::Compress`] when the compression stops short.
fn compress_with_progress(input: &Path, output: &Path) -> Result<Stats, RunError> {
    let stop = stop_flag().map_err(RunError::Signal)?;
    let bar = progress_bar(input);
    let result = compress_file(
        input,
        output,
        &|| stop.load(Ordering::Relaxed),
        &mut |bytes| bar.set_position(bytes),
    );
    bar.finish_and_clear();
    Ok(result?)
}

/// Make the progress bar of one run.
///
/// The bar carries the name of the input file, and the count of the bytes of
/// that file sets its length. `termbar` measures the name in the columns that
/// the name takes, thus a name of many bytes per character fits the bar.
fn progress_bar(input: &Path) -> ProgressBar {
    let length = fs::metadata(input).map_or(UNKNOWN_LENGTH, |data| data.len());
    let bar = ProgressBar::new(length);
    let name = input
        .file_name()
        .unwrap_or(input.as_os_str())
        .to_string_lossy();
    if let Ok(style) = ProgressStyleBuilder::copy(&name).build(TerminalWidth::get_or_default()) {
        bar.set_style(style);
    }
    bar
}

/// Render an error and every cause under it as one line.
///
/// The message of the tool names what the run could not do. The cause under it
/// carries the reason of the file system, thus the user reads both.
fn chain(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut cause = error.source();
    while let Some(next) = cause {
        message.push_str(CAUSE_SEPARATOR);
        message.push_str(&next.to_string());
        cause = next.source();
    }
    message
}

/// The signals that stop a run.
///
/// SIGINT is the Ctrl-C of the terminal, and SIGTERM is the polite kill.
/// SIGKILL is the one signal that no program handles, and this tool does not
/// pretend otherwise.
#[cfg(unix)]
const TERMINATION_SIGNALS: [std::os::raw::c_int; 2] =
    [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM];

/// A flag that a termination signal sets.
///
/// The compression reads the flag before each block of the input. A run that
/// the user stops therefore ends between two blocks, and the library then
/// removes the part of the output file that the run wrote.
///
/// # Errors
///
/// Returns the reason when the platform refuses the registration.
#[cfg(unix)]
fn stop_flag() -> io::Result<Arc<AtomicBool>> {
    let flag = Arc::new(AtomicBool::new(false));
    for signal in TERMINATION_SIGNALS {
        signal_hook::flag::register(signal, Arc::clone(&flag))?;
    }
    Ok(flag)
}

/// A flag that a termination signal sets.
///
/// This platform registers no handler, thus nothing sets the flag. A user of
/// this platform who stops a run ends the process where it stands, and the
/// part of the output file stays on the disk.
///
/// # Errors
///
/// Returns no reason. The result holds the shape of the unix build, thus one
/// call site serves both platforms.
#[cfg(not(unix))]
fn stop_flag() -> io::Result<Arc<AtomicBool>> {
    Ok(Arc::new(AtomicBool::new(false)))
}
