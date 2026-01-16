use std::cmp::Reverse;
use std::collections::HashMap;
use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

mod collector;
mod model;
mod ui;

use collector::bandwidth::BandwidthCollector;
use collector::iops::IOPSCollector;
use model::{BytesPerSec, OpsPerSec, ProcessIOStats};
use ui::AppState;

#[derive(Parser)]
#[command(
    name = "disk-hog",
    about = "Show per-process disk I/O usage on macOS",
    long_about = "disk-hog displays per-process disk bandwidth and IOPS in a continuously updating terminal UI.\n\nBandwidth monitoring works without root. IOPS monitoring requires running with sudo."
)]
struct Args {
    /// Refresh interval in seconds (supports decimals, e.g., 0.5)
    #[arg(short, long, default_value = "1.0")]
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

    // Run the app
    let result = run_app(&mut terminal, args, is_root).await;

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

    // Track actual elapsed time for accurate rate calculation
    let mut last_collection = Instant::now();

    // Do an initial collection to prime the previous readings
    // Use tick_rate for the first interval since we don't have a real elapsed time yet
    state.bandwidth_stats = bandwidth_collector.collect(tick_rate);

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
