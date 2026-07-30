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

#[cfg(test)]
mod tests {
    use super::*;

    /// A [`BarProgress`] over a bar that never draws.
    ///
    /// The bar still tracks every value handed to it, so the assertions below
    /// read exactly the state a visible bar would render from - without a
    /// terminal, and without the integration tests' problem that piping stderr
    /// makes indicatif hide the line and its behavior with it.
    ///
    /// Nothing here touches the filesystem, the network, or a shared name, so
    /// the module is safe to run alongside another copy of itself.
    fn hidden_progress() -> BarProgress {
        BarProgress::new(ProgressBar::hidden())
    }

    /// A slow first `readdir` can leave the bar on screen for seconds before a
    /// single file turns up, and the line has to say something during that time.
    #[test]
    fn a_fresh_bar_already_reads_as_zeroes() {
        let progress = hidden_progress();

        assert_eq!(
            progress.bar.message(),
            "discovered 0 · remaining 0",
            "the counts are painted at construction, so the line before the first \
             file looks the same as the line during the scan rather than blank"
        );
    }

    #[test]
    fn discovery_sets_the_denominator_and_the_outstanding_count() {
        let progress = hidden_progress();

        progress.files_discovered(3);

        assert_eq!(
            progress.bar.length(),
            Some(3),
            "the discovered total is the bar's denominator, so the filled fraction \
             is measured against what the walk has actually turned up"
        );
        assert_eq!(
            progress.bar.message(),
            "discovered 3 · remaining 3",
            "no worker has reached any of these files yet, so all three are outstanding"
        );
    }

    #[test]
    fn scanning_advances_the_bar_and_drains_the_remainder() {
        let progress = hidden_progress();

        progress.files_discovered(10);
        progress.files_scanned(4);

        assert_eq!(
            progress.bar.position(),
            4,
            "the scanned total is the bar's position, which is what fills the bar \
             and what indicatif derives the ETA from"
        );
        assert_eq!(
            progress.bar.message(),
            "discovered 10 · remaining 6",
            "remaining is the work discovery has found and the workers have not reached"
        );
    }

    /// Discovery and scanning run at the same time and each callback is told only
    /// its own total, so whichever one fires has to remember the other - which is
    /// the entire reason both counters live on this struct.
    #[test]
    fn either_callback_leaves_the_other_total_standing() {
        let progress = hidden_progress();

        progress.files_discovered(5);
        progress.files_scanned(2);
        progress.files_discovered(9);

        assert_eq!(
            progress.bar.message(),
            "discovered 9 · remaining 7",
            "a discovery arriving mid-scan must widen the remainder, \
             not forget the files already read"
        );
        assert_eq!(
            progress.bar.length(),
            Some(9),
            "the newest discovered total is the denominator"
        );
        assert_eq!(
            progress.bar.position(),
            2,
            "a discovery must not disturb how far along the bar is"
        );
    }

    /// The two counters are loaded one after the other, so a worker reporting in
    /// between can make `scanned` read as larger than the `discovered` value this
    /// call saw. That is a torn read of a scan that is briefly a file ahead of
    /// itself - a wrapping subtraction would put `remaining
    /// 18,446,744,073,709,551,615` on the line over it.
    #[test]
    fn a_scanned_total_ahead_of_discovery_renders_as_nothing_remaining() {
        let progress = hidden_progress();

        progress.files_discovered(1);
        progress.files_scanned(2);

        assert_eq!(
            progress.bar.message(),
            "discovered 1 · remaining 0",
            "a momentarily inverted pair of totals must round down to no work left, \
             never underflow into a twenty-digit count"
        );
    }

    /// The scans worth running are hundreds of thousands of files deep, and at
    /// that size an unseparated run of digits cannot be read at a glance.
    #[test]
    fn large_totals_carry_thousands_separators() {
        let progress = hidden_progress();

        progress.files_discovered(1_234_567);
        progress.files_scanned(1_000);

        assert_eq!(
            progress.bar.message(),
            "discovered 1,234,567 · remaining 1,233,567",
            "both counts go through HumanCount, so seven-digit totals stay legible"
        );
    }
}
