//! `prgz` (Progress Gzip): compress one file with gzip.
//!
//! This file holds the three parts of the tool that need a process of their
//! own: the command line, the progress bar, and the answer to a stop signal.
//! The library beside it holds the compression and the closing report.
//!
//! A run that the user stops once leaves no output file behind, when the run
//! made that file itself. The Go tool that this binary replaces sent Ctrl-C
//! to the quit path of its terminal library, thus it left a part of a gzip
//! stream on the disk. A part of a gzip stream looks like a whole one, thus
//! the user got a broken file. A run that replaced an output file that was
//! already there, and that stops after the replace, leaves the part of that
//! file that it had written in place instead: that file was already the
//! user's before the run began.
//!
//! A run that the user stops twice ends at once, on the second signal, and
//! it leaves the part of the output file that it had written. The second
//! signal exists for the run whose read never returns, thus the run never
//! reads the stop flag a second time and the graceful stop above never
//! happens. A user who meets that run gets the process back, at the cost of
//! the part file that the graceful stop would have removed.
//!
//! A run whose output file is already there asks, on standard error, whether
//! it may replace that file, unless `--yes` already answered the question.
//! `prcp`, the sibling tool that copies files with a progress bar, asks the
//! same way; this binary copies its model rather than invent a second one.

#![warn(clippy::unwrap_used)]
#![warn(clippy::expect_used)]

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use buildinfo::version_string;
use clap::{CommandFactory, Parser};
use indicatif::ProgressBar;
use prgz::{
    compress_file, default_output_path, format_report, locale_from_lang, CompressError, Stats,
};
use termbar::{ProgressStyleBuilder, TerminalWidth};
use thiserror::Error;

/// The name of the tool. It starts every line that reports a failure.
const PROGRAM_NAME: &str = "prgz";

/// The names of the environment variables that carry the locale of the user,
/// in the order of POSIX precedence. `LC_ALL` sets the locale of the whole
/// session. `LC_NUMERIC` sets the locale of numbers alone. `LANG` sets the
/// locale of last resort. The report of this tool holds only numbers as
/// locale-dependent text, thus `LC_NUMERIC` is the right variable for this
/// tool, second after `LC_ALL` and before `LANG`. A process that carries none
/// of the three variables gets the locale of an empty value, which is
/// American English.
const LOCALE_VARIABLES: [&str; 3] = ["LC_ALL", "LC_NUMERIC", "LANG"];

/// The text that joins an error to the cause under it.
const CAUSE_SEPARATOR: &str = ": ";

/// The length that the progress bar takes for an input that the run cannot
/// measure. A bar of this length shows the count of the read bytes and no
/// percentage, because the run does not know the whole yet.
const UNKNOWN_LENGTH: u64 = 0;

/// The placeholder inside [`REPLACE_PROMPT`] that a run replaces with the
/// path of the output file before it prints the prompt.
const REPLACE_PROMPT_PLACEHOLDER: &str = "{path}";

/// The prompt that a run prints, to standard error, when the output file is
/// already there and `--yes` did not already answer the question. The
/// pattern matches the confirmation prompt of `prcp`: a blank line first,
/// the path in quotes, and the `(y/N)` convention of a default no.
const REPLACE_PROMPT: &str = "\nOutput file '{path}' already exists. Overwrite? (y/N): ";

/// The one answer, in either case, that means yes.
const YES_ANSWER: &str = "y";

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

    /// Replace the output file when it is already there, with no prompt
    #[arg(long, short = 'y')]
    yes: bool,
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
    let locale = locale_from_lang(&locale_value());
    match compress_with_progress(input.as_path(), output.as_path(), cli.yes) {
        Ok(stats) => {
            println!("{}", format_report(&stats, &locale));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{PROGRAM_NAME}{CAUSE_SEPARATOR}{}", chain(&error));
            ExitCode::FAILURE
        }
    }
}

/// Read the locale value of the environment.
///
/// The function checks [`LOCALE_VARIABLES`] in order and answers the value of
/// the first variable that is set and not empty. A variable that the
/// environment carries with an empty value does not count as set, thus the
/// function moves on to the next name of the list. The function answers an
/// empty string when the environment carries none of the three variables,
/// which gives the caller the locale of American English.
fn locale_value() -> String {
    LOCALE_VARIABLES
        .into_iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
        .unwrap_or_default()
}

/// Compress the input into the output and draw a progress bar while the run
/// goes on.
///
/// The bar goes away when the run ends, whether the run was a success or not.
/// The library removes an output file that the run made itself when the run
/// fails, but it keeps an output file that was already there and that the
/// run replaced, thus a run that stops short of an existing output leaves
/// that file, though not necessarily whole, on the disk.
///
/// A run whose output file is already there asks, on standard error, whether
/// it may replace that file, unless `yes` already answers the question. The
/// function settles that question before it makes the progress bar: the bar
/// comes into being on the first byte of progress, which follows every check
/// inside the library, thus a run that stops short of any progress — on the
/// question above or on an earlier check — never draws a bar for the prompt
/// to interleave with.
///
/// # Errors
///
/// Returns [`RunError::Signal`] when the system refuses the stop signals, and
/// [`RunError::Compress`] when the compression stops short.
fn compress_with_progress(input: &Path, output: &Path, yes: bool) -> Result<Stats, RunError> {
    let stop = stop_flag().map_err(RunError::Signal)?;
    let mut bar: Option<ProgressBar> = None;
    let result = compress_file(
        input,
        output,
        &|path| yes || ask_to_replace(path),
        &|| stop.load(Ordering::Relaxed),
        &mut |bytes| {
            bar.get_or_insert_with(|| progress_bar(input))
                .set_position(bytes);
        },
    );
    if let Some(bar) = bar {
        bar.finish_and_clear();
    }
    Ok(result?)
}

/// Ask, on standard error, whether a run may replace the output file at
/// `path`, and answer what the user typed.
///
/// The function prints [`REPLACE_PROMPT`] with the path of `path` in place
/// of [`REPLACE_PROMPT_PLACEHOLDER`], then reads one line from standard
/// input. An answer of [`YES_ANSWER`], in either case, means yes. Every
/// other answer means no, thus an answer that the read cannot get — the end
/// of the input, or a failure of the read — also means no. The function
/// never unwraps a failure of the terminal, because a run that meets such a
/// failure must still get a clear no rather than a crash.
fn ask_to_replace(path: &Path) -> bool {
    let prompt = REPLACE_PROMPT.replace(REPLACE_PROMPT_PLACEHOLDER, &path.display().to_string());
    eprint!("{prompt}");
    let _ = io::stderr().flush();
    let mut answer = String::new();
    match io::stdin().read_line(&mut answer) {
        Ok(_) => answer.trim().eq_ignore_ascii_case(YES_ANSWER),
        Err(_) => false,
    }
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
/// pretend otherwise. A second signal off this list, of either kind, ends the
/// run at once; see [`stop_flag`].
#[cfg(unix)]
const TERMINATION_SIGNALS: [std::os::raw::c_int; 2] =
    [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM];

/// The number that a shell adds to a signal number to report a process that
/// the signal ended. A second stop signal ends this process with the status
/// that the shell reports for the signal itself.
#[cfg(unix)]
const SIGNAL_EXIT_BASE: std::os::raw::c_int = 128;

/// A flag that a termination signal sets.
///
/// The compression reads the flag before each block of the input, thus a run
/// that the user stops once ends between two blocks and the library removes
/// the part of the output file that the run wrote.
///
/// A run that blocks inside the read of one block, such as a read from a
/// stalled network mount, never reaches that check again, thus the first
/// stop signal goes unanswered for as long as the read blocks. A second stop
/// signal, of either kind that [`TERMINATION_SIGNALS`] lists, then ends the
/// process at once. No cleanup runs on that path, thus the part of the
/// output file that the run had written stays on the disk. The order of
/// registration makes this so: the function registers the immediate stop
/// before the flag-setting one, for each signal, thus the first signal only
/// sets the flag and the second one finds it already set.
///
/// # Errors
///
/// Returns the reason when the platform refuses the registration.
#[cfg(unix)]
fn stop_flag() -> io::Result<Arc<AtomicBool>> {
    let flag = Arc::new(AtomicBool::new(false));
    for signal in TERMINATION_SIGNALS {
        // The immediate stop must register before the flag-setting one. On
        // the first signal the flag still reads false, thus this action does
        // nothing and the second action below sets the flag. On a second
        // signal the flag already reads true, thus this action ends the
        // process before the second action runs at all.
        signal_hook::flag::register_conditional_shutdown(
            signal,
            SIGNAL_EXIT_BASE + signal,
            Arc::clone(&flag),
        )?;
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
