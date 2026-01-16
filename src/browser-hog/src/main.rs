//! browser-hog - Identify which Chrome processes are using the most CPU
//!
//! Shows high-CPU Chrome processes and lists open tabs to help identify
//! problematic tabs causing high CPU usage.

use anyhow::{Context, Result};
use chrono::Local;
use clap::Parser;
use colored::Colorize;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{self, ClearType},
};
use human_bytes::human_bytes;
use serde::{Deserialize, Serialize};
use std::io::{stdout, Write};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use sysinfo::{ProcessRefreshKind, RefreshKind, System};

/// Identify which Chrome processes are using the most CPU
#[derive(Parser)]
#[command(name = "browser-hog")]
#[command(
    about = "Identify which Chrome processes are using the most CPU",
    long_about = "Shows high-CPU Chrome processes and lists open tabs to help identify \
                  problematic tabs causing high CPU usage."
)]
struct Args {
    /// Maximum number of processes to show
    #[arg(short = 'n', long, default_value = "10")]
    limit: usize,

    /// Number of CPU samples to take (more = more accurate but slower)
    #[arg(short, long, default_value = "3")]
    samples: u32,

    /// Output as JSON
    #[arg(short, long)]
    json: bool,

    /// Skip showing open tabs
    #[arg(long)]
    no_tabs: bool,

    /// Watch mode: continuously update like 'top'
    #[arg(short, long)]
    watch: bool,

    /// Refresh interval in seconds for watch mode
    #[arg(short, long, default_value = "2")]
    interval: u64,
}

/// Type of Chrome process
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProcessType {
    Main,
    Renderer,
    Gpu,
    Network,
    Plugin,
    Utility,
    Unknown,
}

impl ProcessType {
    /// Parse process type from process name
    fn from_name(name: &str) -> Self {
        if name.contains("(Renderer)") {
            Self::Renderer
        } else if name.contains("(GPU)") {
            Self::Gpu
        } else if name.contains("(Network)") || name.contains("Network Service") {
            Self::Network
        } else if name.contains("(Plugin)") {
            Self::Plugin
        } else if name.contains("(Utility)") || name.contains("Helper") {
            Self::Utility
        } else if name == "Google Chrome" {
            Self::Main
        } else {
            Self::Unknown
        }
    }
}

impl std::fmt::Display for ProcessType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Main => write!(f, "Main"),
            Self::Renderer => write!(f, "Renderer"),
            Self::Gpu => write!(f, "GPU"),
            Self::Network => write!(f, "Network"),
            Self::Plugin => write!(f, "Plugin"),
            Self::Utility => write!(f, "Utility"),
            Self::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Information about a Chrome process
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChromeProcess {
    pid: u32,
    name: String,
    cpu_percent: f32,
    memory_bytes: u64,
    process_type: ProcessType,
}

/// Information about a Chrome tab
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TabInfo {
    window_index: u32,
    tab_index: u32,
    url: String,
    title: String,
}

/// Combined output for JSON mode
#[derive(Debug, Serialize)]
struct Output {
    processes: Vec<ChromeProcess>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tabs: Option<Vec<TabInfo>>,
    sample_count: u32,
    sample_duration_ms: u64,
}

fn main() -> Result<()> {
    let args = Args::parse();

    if args.watch {
        run_watch_mode(&args)
    } else {
        run_once(&args)
    }
}

/// Run once and exit
fn run_once(args: &Args) -> Result<()> {
    // Sample CPU usage
    let sample_duration_ms = u64::from(args.samples) * 300;
    let processes = sample_chrome_processes(args.samples)?;

    // Get tabs unless disabled
    let tabs = if args.no_tabs {
        None
    } else {
        match get_chrome_tabs() {
            Ok(t) => Some(t),
            Err(e) => {
                if !args.json {
                    eprintln!("{} Could not get tabs: {}", "Warning:".yellow(), e);
                }
                None
            }
        }
    };

    // Limit and sort processes by CPU
    let mut processes = processes;
    processes.sort_by(|a, b| {
        b.cpu_percent
            .partial_cmp(&a.cpu_percent)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    processes.truncate(args.limit);

    if args.json {
        let output = Output {
            processes,
            tabs,
            sample_count: args.samples,
            sample_duration_ms,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        print_human_readable(&processes, &tabs, args.samples, sample_duration_ms, false);
    }

    Ok(())
}

/// Run in continuous watch mode like 'top'
fn run_watch_mode(args: &Args) -> Result<()> {
    // Set up Ctrl+C handler
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    ctrlc_handler(r);

    // Enable raw mode to capture key presses
    terminal::enable_raw_mode()?;

    let result = watch_loop(args, &running);

    // Restore terminal state
    terminal::disable_raw_mode()?;

    // Show cursor and clear any remaining state
    let mut stdout = stdout();
    execute!(stdout, cursor::Show)?;

    result
}

/// Set up Ctrl+C handler
fn ctrlc_handler(running: Arc<AtomicBool>) {
    // Handle Ctrl+C signal
    let _ = ctrlc::set_handler(move || {
        running.store(false, Ordering::SeqCst);
    });
}

/// Main watch loop
fn watch_loop(args: &Args, running: &Arc<AtomicBool>) -> Result<()> {
    let mut stdout = stdout();
    let sample_duration_ms = u64::from(args.samples) * 300;

    // Keep a persistent System instance for more accurate CPU readings
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::everything()),
    );

    // Initial refresh to establish baseline
    sys.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::everything(),
    );

    while running.load(Ordering::SeqCst) {
        // Check for 'q' key press (non-blocking)
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key_event) = event::read()? {
                if key_event.code == KeyCode::Char('q')
                    || key_event.code == KeyCode::Char('Q')
                    || (key_event.code == KeyCode::Char('c')
                        && key_event.modifiers.contains(KeyModifiers::CONTROL))
                {
                    break;
                }
            }
        }

        // Sample CPU
        for _ in 0..args.samples {
            thread::sleep(Duration::from_millis(300));
            sys.refresh_processes_specifics(
                sysinfo::ProcessesToUpdate::All,
                true,
                ProcessRefreshKind::everything(),
            );

            // Check if we should exit during sampling
            if !running.load(Ordering::SeqCst) {
                return Ok(());
            }
        }

        // Collect Chrome processes
        let mut processes: Vec<ChromeProcess> = sys
            .processes()
            .values()
            .filter(|p| {
                let name = p.name().to_string_lossy();
                name.contains("Google Chrome") || name.contains("Chrome Helper")
            })
            .map(|p| {
                let name = p.name().to_string_lossy().to_string();
                ChromeProcess {
                    pid: p.pid().as_u32(),
                    name: name.clone(),
                    cpu_percent: p.cpu_usage(),
                    memory_bytes: p.memory(),
                    process_type: ProcessType::from_name(&name),
                }
            })
            .collect();

        // Sort and limit
        processes.sort_by(|a, b| {
            b.cpu_percent
                .partial_cmp(&a.cpu_percent)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        processes.truncate(args.limit);

        // Get tabs unless disabled
        let tabs = if args.no_tabs {
            None
        } else {
            get_chrome_tabs().ok()
        };

        // Clear screen and move cursor to top
        execute!(
            stdout,
            terminal::Clear(ClearType::All),
            cursor::MoveTo(0, 0),
            cursor::Hide
        )?;

        // Print output
        print_human_readable(&processes, &tabs, args.samples, sample_duration_ms, true);
        stdout.flush()?;

        // Brief pause before checking for quit again
        thread::sleep(Duration::from_millis(
            args.interval.saturating_sub(1) * 1000,
        ));
    }

    Ok(())
}

/// Sample Chrome process CPU usage over multiple intervals
///
/// # Errors
///
/// Returns an error if the system info cannot be retrieved.
fn sample_chrome_processes(samples: u32) -> Result<Vec<ChromeProcess>> {
    let interval = Duration::from_millis(300);

    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::everything()),
    );

    // First refresh establishes baseline (CPU will be 0)
    sys.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::everything(),
    );

    // Take additional samples to get accurate CPU readings
    for _ in 0..samples {
        thread::sleep(interval);
        sys.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::everything(),
        );
    }

    // Now collect Chrome processes with accurate CPU readings
    let processes: Vec<ChromeProcess> = sys
        .processes()
        .values()
        .filter(|p| {
            let name = p.name().to_string_lossy();
            name.contains("Google Chrome") || name.contains("Chrome Helper")
        })
        .map(|p| {
            let name = p.name().to_string_lossy().to_string();
            ChromeProcess {
                pid: p.pid().as_u32(),
                name: name.clone(),
                cpu_percent: p.cpu_usage(),
                memory_bytes: p.memory(),
                process_type: ProcessType::from_name(&name),
            }
        })
        .collect();

    Ok(processes)
}

/// Get Chrome tabs using AppleScript
///
/// # Errors
///
/// Returns an error if Chrome is not running or AppleScript fails.
fn get_chrome_tabs() -> Result<Vec<TabInfo>> {
    let script = r#"
        tell application "System Events"
            if not (exists process "Google Chrome") then
                return "NOT_RUNNING"
            end if
        end tell

        tell application "Google Chrome"
            set output to ""
            set winIdx to 0
            repeat with w in windows
                set winIdx to winIdx + 1
                set tabList to tabs of w
                set tabIdx to 0
                repeat with t in tabList
                    set tabIdx to tabIdx + 1
                    set tabUrl to URL of t
                    set tabTitle to title of t
                    set output to output & winIdx & "|" & tabIdx & "|" & tabUrl & "|" & tabTitle & linefeed
                end repeat
            end repeat
            return output
        end tell
    "#;

    let output = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .context("Failed to run AppleScript")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not allowed") || stderr.contains("not permitted") {
            return Err(anyhow::anyhow!(
                "Automation permission denied. Enable in: System Settings > Privacy & Security > Automation"
            ));
        }
        return Err(anyhow::anyhow!("AppleScript failed: {}", stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stdout = stdout.trim();

    if stdout == "NOT_RUNNING" {
        return Err(anyhow::anyhow!("Google Chrome is not running"));
    }

    let mut tabs = Vec::new();
    for line in stdout.lines() {
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.splitn(4, '|').collect();
        if parts.len() >= 4 {
            if let (Ok(win), Ok(tab)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
                tabs.push(TabInfo {
                    window_index: win,
                    tab_index: tab,
                    url: parts[2].to_string(),
                    title: parts[3].to_string(),
                });
            }
        }
    }

    Ok(tabs)
}

/// Print human-readable output
fn print_human_readable(
    processes: &[ChromeProcess],
    tabs: &Option<Vec<TabInfo>>,
    samples: u32,
    duration_ms: u64,
    watch_mode: bool,
) {
    if processes.is_empty() {
        println!(
            "{} No Chrome processes found. Is Chrome running?",
            "Note:".yellow()
        );
        if watch_mode {
            println!("\n{}", "Press 'q' to quit".dimmed());
        }
        return;
    }

    // Header with timestamp in watch mode
    let duration_secs = duration_ms as f64 / 1000.0;
    if watch_mode {
        let now = Local::now();
        println!(
            "{} - {} ({} samples over {:.1}s)\n",
            "browser-hog".bold(),
            now.format("%H:%M:%S").to_string().dimmed(),
            samples,
            duration_secs
        );
    } else {
        println!(
            "\n{} ({} samples over {:.1}s)\n",
            "Chrome CPU Usage".bold(),
            samples,
            duration_secs
        );
    }

    // Table header
    println!(
        "   {:>6}  {:>6}  {:>9}  {}",
        "PID".bold(),
        "CPU%".bold(),
        "MEM".bold(),
        "TYPE".bold()
    );
    println!("{}", "─".repeat(45));

    // Process rows
    for p in processes {
        let cpu_str = format!("{:.1}%", p.cpu_percent);
        let mem_str = human_bytes(p.memory_bytes as f64);
        let type_str = format!("{}", p.process_type);

        // Color CPU based on usage
        let cpu_colored = if p.cpu_percent > 50.0 {
            cpu_str.red().bold()
        } else if p.cpu_percent > 20.0 {
            cpu_str.yellow()
        } else {
            cpu_str.normal()
        };

        println!(
            "   {:>6}  {:>6}  {:>9}  {}",
            p.pid, cpu_colored, mem_str, type_str
        );
    }

    // Tabs section
    if let Some(tabs) = tabs {
        println!("\n{} ({}):\n", "Open Tabs".bold(), tabs.len());

        for tab in tabs {
            // Extract domain from URL for display
            let domain = extract_domain(&tab.url);
            let title = truncate_string(&tab.title, 50);

            println!(
                "  {} {} ({})",
                format!("[{}:{}]", tab.window_index, tab.tab_index).dimmed(),
                title,
                domain.cyan()
            );
        }
    }

    // Footer
    if watch_mode {
        println!(
            "\n{} (Window → Task Manager in Chrome for PID→tab mapping)",
            "Press 'q' to quit".dimmed()
        );
    } else {
        println!(
            "\n{} Use Chrome's Task Manager ({}) to see which tab uses which PID.\n",
            "Tip:".green(),
            "Window → Task Manager".bold()
        );
    }
}

/// Extract domain from URL
fn extract_domain(url: &str) -> String {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .map(|s| s.split('/').next().unwrap_or(s))
        .unwrap_or(url)
        .to_string()
}

/// Truncate string to max length with ellipsis
fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_type_from_name() {
        assert_eq!(
            ProcessType::from_name("Google Chrome Helper (Renderer)"),
            ProcessType::Renderer
        );
        assert_eq!(
            ProcessType::from_name("Google Chrome Helper (GPU)"),
            ProcessType::Gpu
        );
        assert_eq!(ProcessType::from_name("Google Chrome"), ProcessType::Main);
        assert_eq!(
            ProcessType::from_name("Google Chrome Helper"),
            ProcessType::Utility
        );
    }

    #[test]
    fn test_extract_domain() {
        assert_eq!(extract_domain("https://github.com/foo/bar"), "github.com");
        assert_eq!(extract_domain("http://example.com"), "example.com");
        assert_eq!(extract_domain("chrome://settings"), "chrome://settings");
    }

    #[test]
    fn test_truncate_string() {
        assert_eq!(truncate_string("short", 10), "short");
        assert_eq!(truncate_string("this is a long string", 10), "this is...");
    }
}
