use std::cmp::Reverse;
use std::collections::HashMap;
use std::io::{self, Write};
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

/// RAII guard that restores terminal state on drop.
///
/// This ensures the terminal is properly restored even if a panic occurs,
/// preventing the user from being left with a broken terminal (raw mode,
/// alternate screen, etc.).
struct TerminalGuard {
    /// Whether the terminal has been initialized (raw mode enabled, alternate screen entered).
    initialized: bool,
}

impl TerminalGuard {
    /// Creates a new guard, marking the terminal as initialized.
    fn new() -> Self {
        Self { initialized: true }
    }

    /// Marks the terminal as successfully cleaned up, preventing double-cleanup on drop.
    fn disarm(&mut self) {
        self.initialized = false;
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.initialized {
            // Best-effort cleanup on panic - ignore errors since we're already in trouble
            let _ = disable_raw_mode();
            let _ = execute!(
                io::stdout(),
                LeaveAlternateScreen,
                DisableMouseCapture
            );
            // Try to show the cursor
            let _ = io::stdout().write_all(b"\x1B[?25h");
            let _ = io::stdout().flush();
        }
    }
}

mod collector;
mod model;
mod ui;

use collector::bandwidth::BandwidthCollector;
use collector::iops::IOPSCollector;
use model::{BytesPerSec, OpsPerSec, ProcessIOStats};
use ui::AppState;

/// Minimum allowed refresh rate in seconds.
const MIN_REFRESH_SECS: f64 = 0.1;

/// Maximum allowed refresh rate in seconds.
const MAX_REFRESH_SECS: f64 = 60.0;

/// Parses and validates the refresh rate argument.
///
/// Ensures the value is:
/// - A valid finite positive number
/// - Within the allowed range (0.1 to 60 seconds)
///
/// This validation prevents panics from `Duration::from_secs_f64()` which
/// would occur with negative, NaN, or infinite values.
fn parse_refresh_rate(s: &str) -> Result<f64, String> {
    let rate: f64 = s
        .parse()
        .map_err(|_| format!("'{s}' is not a valid number"))?;

    if !rate.is_finite() {
        return Err("refresh rate must be a finite number".to_string());
    }
    if rate < MIN_REFRESH_SECS {
        return Err(format!(
            "refresh rate must be at least {MIN_REFRESH_SECS} seconds"
        ));
    }
    if rate > MAX_REFRESH_SECS {
        return Err(format!(
            "refresh rate must be at most {MAX_REFRESH_SECS} seconds"
        ));
    }
    Ok(rate)
}

#[derive(Parser)]
#[command(
    name = "disk-hog",
    about = "Show per-process disk I/O usage on macOS",
    long_about = "disk-hog displays per-process disk bandwidth and IOPS in a continuously updating terminal UI.\n\nBandwidth monitoring works without root. IOPS monitoring requires running with sudo."
)]
struct Args {
    /// Refresh interval in seconds (supports decimals, e.g., 0.5). Range: 0.1-60.
    #[arg(short, long, default_value = "1.0", value_parser = parse_refresh_rate)]
    refresh: f64,

    /// Number of processes to show per pane
    #[arg(short = 'n', long, default_value = "10")]
    count: usize,

    /// Only show bandwidth, skip IOPS even with sudo
    #[arg(short, long)]
    bandwidth_only: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Check if running as root
    let is_root = IOPSCollector::is_root();

    if !is_root {
        eprintln!("Note: Running without sudo - only bandwidth data will be shown.");
        eprintln!("Run with sudo to enable IOPS monitoring.\n");
    }

    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    // Create guard AFTER terminal is set up - it will restore on panic
    let mut guard = TerminalGuard::new();

    // Run the app
    let result = run_app(&mut terminal, args, is_root).await;

    // Normal cleanup path - disarm the guard since we'll clean up explicitly
    guard.disarm();

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    args: Args,
    is_root: bool,
) -> Result<()> {
    let tick_rate = Duration::from_secs_f64(args.refresh);

    // Initialize collectors
    let mut bandwidth_collector = BandwidthCollector::new();

    // IOPS collector (only if root and not bandwidth_only mode)
    let iops_collector = if is_root && !args.bandwidth_only {
        let mut collector = IOPSCollector::new();
        collector.start().await?;
        Some(collector)
    } else {
        None
    };

    // App state
    let mut state = AppState::new(args.count, is_root && !args.bandwidth_only);

    // Do an initial collection to prime the previous readings.
    // This establishes the baseline for calculating deltas. We discard these
    // results since they represent cumulative totals, not per-interval rates.
    let _ = bandwidth_collector.collect(tick_rate);

    // Track actual elapsed time for accurate rate calculation.
    // Initialize this BEFORE the sleep so the first iteration correctly measures
    // the full interval (sleep time + any overhead). This prevents inflated rates
    // that would occur if we only measured from after the sleep completes.
    let mut last_collection = Instant::now();

    // Wait for the first tick interval before starting the main loop.
    // This ensures the first displayed rates are based on a full interval,
    // not the tiny amount of time between priming and the first loop iteration.
    tokio::time::sleep(tick_rate).await;

    loop {
        // Calculate actual elapsed time since last collection
        let elapsed = last_collection.elapsed();
        last_collection = Instant::now();

        // Collect bandwidth data using actual elapsed time
        state.bandwidth_stats = bandwidth_collector.collect(elapsed);

        // Collect IOPS data if available
        if let Some(ref iops_collector) = iops_collector {
            // Check for parser errors
            if iops_collector.has_parser_error() {
                state.iops_error = true;
            }

            let iops_data = iops_collector.snapshot_and_reset();

            // Convert to ProcessIOStats using elapsed time for rate calculation.
            // Reuse the bandwidth_collector for process name lookups to avoid
            // creating a duplicate System instance.
            state.iops_stats = Some(convert_iops_to_stats(
                &iops_data,
                &bandwidth_collector,
                elapsed,
            ));
        }

        // Render
        terminal.draw(|f| ui::render(f, &state))?;

        // Handle input with timeout
        if event::poll(tick_rate)? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                    KeyCode::Esc => break,
                    _ => {}
                }
            }
        }
    }

    // Stop IOPS collector
    if let Some(mut collector) = iops_collector {
        collector.stop().await;
    }

    Ok(())
}

/// Converts IOPS counter data to `ProcessIOStats`.
///
/// The `elapsed` parameter specifies the actual time since the last collection,
/// used to calculate accurate ops-per-second rates.
///
/// Uses the `bandwidth_collector` for process name lookups since it already
/// maintains a `System` instance with the process list refreshed.
fn convert_iops_to_stats(
    iops_data: &HashMap<u32, model::IOPSCounter>,
    bandwidth_collector: &BandwidthCollector,
    elapsed: Duration,
) -> Vec<ProcessIOStats> {
    let mut stats: Vec<ProcessIOStats> = iops_data
        .iter()
        .filter(|(_, counter)| counter.total() > 0)
        .map(|(pid, counter)| {
            // Look up process name using the shared collector
            let name = bandwidth_collector.lookup_process_name(*pid);

            // Convert raw counts to rates using actual elapsed time
            let read_ops_rate = OpsPerSec::from_ops_and_duration(counter.read_ops, elapsed);
            let write_ops_rate = OpsPerSec::from_ops_and_duration(counter.write_ops, elapsed);

            ProcessIOStats {
                pid: *pid,
                name,
                read_bytes_per_sec: BytesPerSec(0),
                write_bytes_per_sec: BytesPerSec(0),
                read_ops_per_sec: Some(read_ops_rate),
                write_ops_per_sec: Some(write_ops_rate),
            }
        })
        .collect();

    // Sort by total IOPS, descending
    stats.sort_by_key(|s| Reverse(s.total_iops().unwrap_or(OpsPerSec(0))));

    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_refresh_rate_valid() {
        assert_eq!(parse_refresh_rate("1.0").unwrap(), 1.0);
        assert_eq!(parse_refresh_rate("0.5").unwrap(), 0.5);
        assert_eq!(parse_refresh_rate("0.1").unwrap(), 0.1);
        assert_eq!(parse_refresh_rate("60").unwrap(), 60.0);
        assert_eq!(parse_refresh_rate("30.5").unwrap(), 30.5);
    }

    #[test]
    fn test_parse_refresh_rate_below_minimum() {
        let result = parse_refresh_rate("0.05");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("at least"));
    }

    #[test]
    fn test_parse_refresh_rate_above_maximum() {
        let result = parse_refresh_rate("100");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("at most"));
    }

    #[test]
    fn test_parse_refresh_rate_negative() {
        let result = parse_refresh_rate("-1");
        assert!(result.is_err());
        // Negative values are below minimum, so they get that error
        assert!(result.unwrap_err().contains("at least"));
    }

    #[test]
    fn test_parse_refresh_rate_zero() {
        let result = parse_refresh_rate("0");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("at least"));
    }

    #[test]
    fn test_parse_refresh_rate_infinity() {
        let result = parse_refresh_rate("inf");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("finite"));
    }

    #[test]
    fn test_parse_refresh_rate_nan() {
        let result = parse_refresh_rate("NaN");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("finite"));
    }

    #[test]
    fn test_parse_refresh_rate_invalid_string() {
        let result = parse_refresh_rate("abc");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not a valid number"));
    }

    #[test]
    fn test_parse_refresh_rate_empty() {
        let result = parse_refresh_rate("");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not a valid number"));
    }

    #[test]
    fn test_parse_refresh_rate_boundary_values() {
        // Exact minimum should pass
        assert!(parse_refresh_rate("0.1").is_ok());
        // Exact maximum should pass
        assert!(parse_refresh_rate("60").is_ok());
        // Just below minimum should fail
        assert!(parse_refresh_rate("0.09").is_err());
        // Just above maximum should fail
        assert!(parse_refresh_rate("60.1").is_err());
    }
}
