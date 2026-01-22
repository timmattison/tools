use std::cmp::Reverse;
use std::collections::HashMap;
use std::io::{self, Write};
use std::time::{Duration, Instant};

use anyhow::Result;
use buildinfo::version_string;
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
use ui::{AppState, IopsMode};

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
    version = version_string!(),
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

    // Determine IOPS mode based on root status and flags
    let is_root = IOPSCollector::is_root();
    let iops_mode = if args.bandwidth_only {
        IopsMode::DisabledByFlag
    } else if is_root {
        IopsMode::Enabled
    } else {
        IopsMode::DisabledNoRoot
    };

    if iops_mode == IopsMode::DisabledNoRoot {
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
    let result = run_app(&mut terminal, args, iops_mode).await;

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

    // Now that terminal is restored, log any shutdown errors
    let shutdown_error = result?;
    if let Some(error_msg) = shutdown_error {
        eprintln!("{error_msg}");
    }

    Ok(())
}

/// Runs the main application loop.
///
/// Returns `Ok(Some(error_message))` if a shutdown error occurred that should be logged
/// after the terminal is restored. Returns `Ok(None)` on clean shutdown.
async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    args: Args,
    iops_mode: IopsMode,
) -> Result<Option<String>> {
    let tick_rate = Duration::from_secs_f64(args.refresh);

    // Initialize collectors
    let mut bandwidth_collector = BandwidthCollector::new();

    // IOPS collector (only if enabled)
    let iops_collector = if iops_mode.is_enabled() {
        let mut collector = IOPSCollector::new();
        collector.start().await?;
        Some(collector)
    } else {
        None
    };

    // App state
    let mut state = AppState::new(args.count, iops_mode);

    // Establish baseline readings for bandwidth calculation.
    // Without priming, the first collect() would report cumulative totals as rates.
    bandwidth_collector.prime();

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

    // Stop IOPS collector and collect any shutdown errors
    let shutdown_error = if let Some(mut collector) = iops_collector {
        collector.stop().await
    } else {
        None
    };

    Ok(shutdown_error)
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

    // Sort by total IOPS, descending.
    // Safety: total_iops() always returns Some because we set both read_ops_per_sec
    // and write_ops_per_sec to Some above.
    stats.sort_by_key(|s| {
        Reverse(
            s.total_iops()
                .expect("IOPS stats always have both read and write ops set"),
        )
    });

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

    #[test]
    fn test_version_string_format() {
        // Verify version string follows the expected format: "X.Y.Z (hash, status)"
        // This test ensures buildinfo is properly configured and the version
        // macro produces the expected output format.
        let version = version_string!();

        // Should contain a version number (e.g., "0.1.0")
        assert!(
            version.contains('.'),
            "Version string should contain version number with dots: {version}"
        );

        // Should contain parentheses with git info
        assert!(
            version.contains('(') && version.contains(')'),
            "Version string should contain git info in parentheses: {version}"
        );

        // Should contain either "clean" or "dirty" status
        assert!(
            version.contains("clean") || version.contains("dirty"),
            "Version string should contain clean/dirty status: {version}"
        );
    }

    #[test]
    fn test_convert_iops_to_stats_always_has_total_iops() {
        // This test verifies the invariant that convert_iops_to_stats always produces
        // ProcessIOStats with both read_ops_per_sec and write_ops_per_sec set to Some,
        // ensuring total_iops() will never return None.
        //
        // This invariant is relied upon by the sorting code which uses .expect().

        use model::IOPSCounter;

        let bandwidth_collector = BandwidthCollector::new();
        let elapsed = Duration::from_secs(1);

        // Create test IOPS data with various values
        let mut iops_data = HashMap::new();
        iops_data.insert(
            1001,
            IOPSCounter {
                read_ops: 100,
                write_ops: 50,
            },
        );
        iops_data.insert(
            1002,
            IOPSCounter {
                read_ops: 0,
                write_ops: 200,
            },
        );
        iops_data.insert(
            1003,
            IOPSCounter {
                read_ops: 300,
                write_ops: 0,
            },
        );

        let stats = convert_iops_to_stats(&iops_data, &bandwidth_collector, elapsed);

        // Verify all stats have total_iops() returning Some
        for stat in &stats {
            assert!(
                stat.total_iops().is_some(),
                "ProcessIOStats from convert_iops_to_stats must always have total_iops() == Some, \
                 but PID {} has None",
                stat.pid
            );
            // Also verify the individual fields are Some
            assert!(
                stat.read_ops_per_sec.is_some(),
                "read_ops_per_sec must be Some for PID {}",
                stat.pid
            );
            assert!(
                stat.write_ops_per_sec.is_some(),
                "write_ops_per_sec must be Some for PID {}",
                stat.pid
            );
        }

        // Verify we got all 3 entries (none filtered out due to zero total)
        assert_eq!(stats.len(), 3);
    }

    #[test]
    fn test_convert_iops_to_stats_filters_zero_total() {
        // Verify that entries with zero total IOPS are filtered out
        use model::IOPSCounter;

        let bandwidth_collector = BandwidthCollector::new();
        let elapsed = Duration::from_secs(1);

        let mut iops_data = HashMap::new();
        iops_data.insert(
            1001,
            IOPSCounter {
                read_ops: 100,
                write_ops: 50,
            },
        );
        // This entry has zero total and should be filtered
        iops_data.insert(
            1002,
            IOPSCounter {
                read_ops: 0,
                write_ops: 0,
            },
        );

        let stats = convert_iops_to_stats(&iops_data, &bandwidth_collector, elapsed);

        // Should only have one entry (the non-zero one)
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].pid, 1001);
    }

    #[test]
    fn test_convert_iops_to_stats_sorted_by_total_descending() {
        // Verify that stats are sorted by total IOPS in descending order
        use model::IOPSCounter;

        let bandwidth_collector = BandwidthCollector::new();
        let elapsed = Duration::from_secs(1);

        let mut iops_data = HashMap::new();
        // Total: 50
        iops_data.insert(
            1001,
            IOPSCounter {
                read_ops: 30,
                write_ops: 20,
            },
        );
        // Total: 300 (should be first)
        iops_data.insert(
            1002,
            IOPSCounter {
                read_ops: 200,
                write_ops: 100,
            },
        );
        // Total: 100 (should be second)
        iops_data.insert(
            1003,
            IOPSCounter {
                read_ops: 60,
                write_ops: 40,
            },
        );

        let stats = convert_iops_to_stats(&iops_data, &bandwidth_collector, elapsed);

        assert_eq!(stats.len(), 3);
        // Verify descending order by total IOPS
        assert_eq!(stats[0].pid, 1002); // 300 total
        assert_eq!(stats[1].pid, 1003); // 100 total
        assert_eq!(stats[2].pid, 1001); // 50 total
    }
}
