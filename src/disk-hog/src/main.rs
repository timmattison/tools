use std::cmp::Reverse;
use std::collections::HashMap;
use std::io;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

mod collector;
mod model;
mod ui;

use collector::bandwidth::BandwidthCollector;
use collector::iops::IOPSCollector;
use model::ProcessIOStats;
use ui::AppState;

#[derive(Parser)]
#[command(
    name = "disk-hog",
    about = "Show per-process disk I/O usage on macOS",
    long_about = "disk-hog displays per-process disk bandwidth and IOPS in a continuously updating terminal UI.\n\nBandwidth monitoring works without root. IOPS monitoring requires running with sudo."
)]
struct Args {
    /// Refresh interval in seconds
    #[arg(short, long, default_value = "1")]
    refresh: u64,

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
    let tick_rate = Duration::from_secs(args.refresh);

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

    // System for process name lookups (for IOPS data)
    let refresh_kind =
        RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing());
    let mut system = System::new_with_specifics(refresh_kind);

    // App state
    let mut state = AppState::new(args.count, is_root && !args.bandwidth_only);

    loop {
        // Collect bandwidth data
        state.bandwidth_stats = bandwidth_collector.collect();

        // Collect IOPS data if available
        if let Some(ref iops_collector) = iops_collector {
            let iops_data = iops_collector.snapshot_and_reset().await;

            // Refresh process list for name lookups
            system.refresh_processes_specifics(
                ProcessesToUpdate::All,
                true,
                ProcessRefreshKind::nothing(),
            );

            // Convert to ProcessIOStats
            state.iops_stats = Some(convert_iops_to_stats(&iops_data, &system));
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
fn convert_iops_to_stats(
    iops_data: &HashMap<u32, model::IOPSCounter>,
    system: &System,
) -> Vec<ProcessIOStats> {
    let mut stats: Vec<ProcessIOStats> = iops_data
        .iter()
        .filter(|(_, counter)| counter.total() > 0)
        .map(|(pid, counter)| {
            // Look up process name
            let name = system
                .process(sysinfo::Pid::from_u32(*pid))
                .map(|p| p.name().to_string_lossy().to_string())
                .unwrap_or_else(|| format!("pid:{pid}"));

            ProcessIOStats {
                pid: *pid,
                name,
                read_bytes_per_sec: 0,
                write_bytes_per_sec: 0,
                read_ops_per_sec: Some(counter.read_ops),
                write_ops_per_sec: Some(counter.write_ops),
            }
        })
        .collect();

    // Sort by total IOPS, descending
    stats.sort_by_key(|s| Reverse(s.total_iops().unwrap_or(0)));

    stats
}
