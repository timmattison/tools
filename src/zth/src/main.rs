//! `zth` - Zero the Hero.
//!
//! Recursively hunts down files that are nothing but zero bytes and prints
//! their absolute paths. Everything interesting lives in the [`zth`] library;
//! this binary is the argument parsing, the progress bar, and the printing.

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use buildinfo::version_string;
use clap::Parser;
use indicatif::{HumanCount, ProgressBar, ProgressStyle};
use termbar::{calculate_bar_width, TerminalWidth, PROGRESS_CHARS};
use zth::{find_all_zero_files, Jobs, ScanProgress};

/// Progress bar layout. The bar's own width is filled in at runtime.
///
/// The two counts arrive through `{msg}` rather than through `{human_len}` and
/// `{human_pos}` so the bar's length can start at [`INITIAL_BAR_LENGTH`]: a
/// zero-length bar reads as 100% complete in indicatif, which would greet a slow
/// first `readdir` with a full bar next to "discovered 0".
///
/// `{eta}` is indicatif's own estimate. It is re-derived on every redraw from
/// the rate files are being scanned at, so it keeps up as discovery pushes the
/// denominator higher.
const PROGRESS_TEMPLATE: &str = "{spinner:.green} [{bar:BAR_WIDTH.cyan/blue}] {msg} · ETA {eta}";

/// Placeholder inside [`PROGRESS_TEMPLATE`] replaced by the computed bar width.
const BAR_WIDTH_PLACEHOLDER: &str = "BAR_WIDTH";

/// Columns [`PROGRESS_TEMPLATE`] needs for everything that is not the bar.
///
/// Covers the spinner, the brackets, the labels, and generous room for the two
/// counts and the ETA. Overshooting only shortens the bar; undershooting lets
/// indicatif truncate the line, so this leans high.
const PROGRESS_OVERHEAD: u16 = 62;

/// Bar length before the first file turns up, chosen so the bar starts empty
/// rather than full. Overwritten by the first discovery.
const INITIAL_BAR_LENGTH: u64 = 1;

/// How often the spinner and the ETA redraw while nothing else changes.
const TICK_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Parser)]
#[command(name = "zth")]
#[command(version = version_string!())]
#[command(
    about = "Zero the Hero - find files that contain nothing but zero bytes",
    long_about = "Zero the Hero recursively scans PATH for files that are larger than zero bytes \
                  and contain nothing but zero bytes, printing the absolute path of each one.\n\n\
                  Files are read only until their first non-zero byte, symlinks are never \
                  followed, and anything that cannot be read is skipped without a word."
)]
struct Cli {
    /// Directory (or single file) to scan recursively.
    #[arg(value_name = "PATH")]
    path: PathBuf,

    /// Number of files to read at once [default: the machine's core count].
    #[arg(short, long, value_name = "N")]
    jobs: Option<usize>,
}

/// Drives an [`indicatif`] progress bar from the scan's running totals.
///
/// The bar's length is the discovered count and its position is the scanned
/// count, so the filled portion, the ETA, and the remaining count all move as
/// discovery and reading race each other. Both totals are kept here because
/// each callback only learns about its own.
struct BarProgress {
    bar: ProgressBar,
    discovered: AtomicU64,
    scanned: AtomicU64,
}

impl BarProgress {
    /// Wraps a progress bar and paints the zeroed counts onto it, so the line
    /// reads the same before the first file turns up as it does after.
    fn new(bar: ProgressBar) -> Self {
        let progress = Self {
            bar,
            discovered: AtomicU64::new(0),
            scanned: AtomicU64::new(0),
        };
        progress.refresh_counts();
        progress
    }

    /// Rewrites the counts from whichever total just changed.
    ///
    /// The two counters are read independently, so a concurrent update can make
    /// `scanned` momentarily exceed the `discovered` value seen here;
    /// `saturating_sub` renders that as zero rather than as a huge number.
    fn refresh_counts(&self) {
        let discovered = self.discovered.load(Ordering::Relaxed);
        let scanned = self.scanned.load(Ordering::Relaxed);
        self.bar.set_message(format!(
            "discovered {} · remaining {}",
            HumanCount(discovered),
            HumanCount(discovered.saturating_sub(scanned))
        ));
    }
}

impl ScanProgress for BarProgress {
    fn files_discovered(&self, total: u64) {
        self.discovered.store(total, Ordering::Relaxed);
        self.bar.set_length(total);
        self.refresh_counts();
    }

    fn files_scanned(&self, total: u64) {
        self.scanned.store(total, Ordering::Relaxed);
        self.bar.set_position(total);
        self.refresh_counts();
    }
}

/// Builds the progress bar, sized to the terminal.
///
/// The bar draws to stderr, which keeps it clear of the path list on stdout, and
/// indicatif hides it entirely when stderr is not a terminal - so a piped or
/// redirected run emits nothing but results. A styling failure is not worth
/// aborting a scan over: the bar falls back to indicatif's default look.
fn build_progress_bar() -> ProgressBar {
    let bar_width = calculate_bar_width(TerminalWidth::get_or_default(), PROGRESS_OVERHEAD);
    let template = PROGRESS_TEMPLATE.replace(BAR_WIDTH_PLACEHOLDER, &bar_width.to_string());

    let bar = ProgressBar::new(INITIAL_BAR_LENGTH);
    if let Ok(style) = ProgressStyle::with_template(&template) {
        bar.set_style(style.progress_chars(PROGRESS_CHARS));
    }
    bar.enable_steady_tick(TICK_INTERVAL);
    bar
}

/// Writes one path per line to stdout.
///
/// Paths go out as their own bytes rather than through `Path::display`, which
/// would replace anything invalid in UTF-8 with `U+FFFD` and hand back a path
/// that no longer names the file it came from.
///
/// # Errors
///
/// Returns any [`io::Error`] from writing to stdout. A broken pipe means the
/// caller stopped reading early; every other kind means the list stdout received
/// is shorter than what was found, which [`main`] turns into a failing status.
fn print_paths(paths: &[PathBuf]) -> io::Result<()> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();

    for path in paths {
        stdout.write_all(path.as_os_str().as_encoded_bytes())?;
        stdout.write_all(b"\n")?;
    }

    stdout.flush()
}

/// Scans, prints, and fails only when the result list could not be delivered.
///
/// The scan itself has nothing to report: unreadable files and directories are
/// skipped silently and never reach the exit status. Writing the results is the
/// one step that can fail meaningfully, and since `zth` never writes to stderr,
/// the status is the only channel there is to say the list came out short.
fn main() -> ExitCode {
    let cli = Cli::parse();
    let jobs = cli.jobs.map_or_else(Jobs::default, Jobs::new);

    let bar = build_progress_bar();
    let progress = BarProgress::new(bar.clone());

    let found = find_all_zero_files(&cli.path, jobs, &progress);

    bar.finish_and_clear();

    match print_paths(&found) {
        Ok(()) => ExitCode::SUCCESS,
        // A closed pipe (`zth /data | head`) is the caller's business, not an error.
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        // Any other write failure truncated the list; the exit status is the only
        // way to say so, because zth never writes to stderr.
        Err(_) => ExitCode::FAILURE,
    }
}
