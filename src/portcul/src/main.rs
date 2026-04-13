//! portcul - A pretty TUI for viewing and killing processes listening on ports.
//!
//! Displays an interactive table of all processes with open listening sockets.
//! Navigate with arrow keys, kill with 'd' or Enter, refresh with 'r'.

use std::io::{self, Write};
use std::time::Duration;

use anyhow::Result;
use buildinfo::version_string;
use clap::{Parser, Subcommand};
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

mod process;
mod ui;

use process::{
    collect_listeners, filter_by_port, format_kill_header, format_kill_result, format_process_line,
    kill_process,
};
use ui::{AppState, KillConfirm};

/// RAII guard that restores terminal state on drop.
///
/// Ensures the terminal is properly restored even if a panic occurs,
/// preventing the user from being left with a broken terminal.
struct TerminalGuard {
    initialized: bool,
}

impl TerminalGuard {
    fn new() -> Self {
        Self { initialized: true }
    }

    fn disarm(&mut self) {
        self.initialized = false;
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.initialized {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
            // No access to Terminal here, so use raw ANSI to show cursor
            let _ = io::stdout().write_all(b"\x1B[?25h");
            let _ = io::stdout().flush();
        }
    }
}

/// A pretty TUI for viewing and killing processes listening on ports.
///
/// Run with no subcommand to launch the interactive TUI.
/// Use subcommands for non-interactive CLI operations.
#[derive(Parser)]
#[command(
    name = "portcul",
    version = version_string!(),
    about = "A pretty TUI for viewing and killing processes listening on ports"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Refresh interval in seconds (TUI mode only)
    #[arg(short, long, default_value = "2.0", global = true)]
    refresh: f64,
}

#[derive(Subcommand)]
enum Command {
    /// Kill all processes listening on a port
    Kill {
        /// Port number to kill processes on
        port: u16,
        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },
    /// List processes listening on ports
    List {
        /// Only show processes on this port
        port: Option<u16>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Kill { port, yes }) => cmd_kill(port, yes),
        Some(Command::List { port }) => cmd_list(port),
        None => cmd_tui(cli.refresh),
    }
}

/// Runs the TUI (default when no subcommand given).
fn cmd_tui(refresh: f64) -> Result<()> {
    anyhow::ensure!(
        refresh > 0.0 && refresh.is_finite(),
        "refresh interval must be a positive finite number, got {}",
        refresh
    );

    // Collect initial data before entering TUI
    let listeners = match collect_listeners() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let mut guard = TerminalGuard::new();

    let result = run_app(&mut terminal, refresh, listeners);

    guard.disarm();

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

/// Kills all processes on a given port (CLI mode).
fn cmd_kill(port: u16, yes: bool) -> Result<()> {
    let listeners = collect_listeners()?;
    let targets = filter_by_port(&listeners, port);

    if targets.is_empty() {
        eprintln!("No processes found on port {port}");
        std::process::exit(1);
    }

    if yes {
        // Non-interactive: kill immediately
        for listener in &targets {
            match kill_process(listener.pid) {
                Ok(()) => println!("{}", format_kill_result(listener)),
                Err(e) => eprintln!("Failed to kill PID {}: {e}", listener.pid),
            }
        }
    } else {
        // Interactive: show processes and ask for confirmation
        println!("{}", format_kill_header(targets.len(), port));
        for listener in &targets {
            println!("{}", format_process_line(listener));
        }
        println!();

        eprint!("Kill these processes? [y/n] ");
        io::stderr().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if input.trim().eq_ignore_ascii_case("y") {
            for listener in &targets {
                match kill_process(listener.pid) {
                    Ok(()) => println!("{}", format_kill_result(listener)),
                    Err(e) => eprintln!("Failed to kill PID {}: {e}", listener.pid),
                }
            }
        } else {
            eprintln!("Cancelled.");
        }
    }

    Ok(())
}

/// Lists processes listening on ports (CLI mode).
fn cmd_list(port: Option<u16>) -> Result<()> {
    let listeners = collect_listeners()?;

    let to_display: Vec<&process::ListeningProcess> = match port {
        Some(p) => filter_by_port(&listeners, p),
        None => listeners.iter().collect(),
    };

    if to_display.is_empty() {
        match port {
            Some(p) => eprintln!("No processes found on port {p}"),
            None => eprintln!("No processes listening on any ports"),
        }
        std::process::exit(1);
    }

    for listener in &to_display {
        println!("{}", format_process_line(listener));
    }

    Ok(())
}

/// Runs the main application loop.
///
/// # Errors
///
/// Returns an error if terminal drawing or event polling fails.
fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    refresh: f64,
    initial_listeners: Vec<process::ListeningProcess>,
) -> Result<()> {
    let tick_rate = Duration::from_secs_f64(refresh);
    let mut state = AppState::new(initial_listeners);

    loop {
        terminal.draw(|f| ui::render(f, &mut state))?;

        if event::poll(tick_rate)? {
            if let Event::Key(key) = event::read()? {
                // Handle kill confirmation mode separately
                if let KillConfirm::Pending { pid, name, .. } = &state.kill_confirm {
                    let pid = *pid;
                    let name = name.clone();
                    match key.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') => {
                            match kill_process(pid) {
                                Ok(()) => {
                                    state.kill_confirm = KillConfirm::Result {
                                        message: format!("Sent SIGTERM to {name} (PID {pid})"),
                                        is_error: false,
                                    };
                                    // Refresh after kill
                                    refresh_listeners(&mut state);
                                }
                                Err(e) => {
                                    state.kill_confirm = KillConfirm::Result {
                                        message: format!("Failed to kill PID {pid}: {e}"),
                                        is_error: true,
                                    };
                                }
                            }
                        }
                        // Ctrl+C exits the app even during confirmation
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            break
                        }
                        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                            state.kill_confirm = KillConfirm::None;
                        }
                        _ => {
                            state.kill_confirm = KillConfirm::None;
                        }
                    }
                    continue;
                }

                // Clear result messages, then fall through to process the key normally
                if matches!(state.kill_confirm, KillConfirm::Result { .. }) {
                    state.kill_confirm = KillConfirm::None;
                }

                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                    KeyCode::Esc => break,
                    KeyCode::Up | KeyCode::Char('k') => state.select_previous(),
                    KeyCode::Down | KeyCode::Char('j') => state.select_next(),
                    KeyCode::Enter | KeyCode::Char('d') | KeyCode::Delete => {
                        if let Some(listener) = state.selected_listener() {
                            state.kill_confirm = KillConfirm::Pending {
                                pid: listener.pid,
                                name: listener.name.clone(),
                                port: listener.port,
                            };
                        }
                    }
                    KeyCode::Char('r') => {
                        refresh_listeners(&mut state);
                    }
                    _ => {}
                }
            }
        } else {
            // Tick elapsed without input - auto-refresh
            refresh_listeners(&mut state);
        }
    }

    Ok(())
}

/// Refreshes the listener list in the app state.
fn refresh_listeners(state: &mut AppState) {
    match collect_listeners() {
        Ok(listeners) => {
            state.refresh(listeners);
            state.refresh_error = None;
        }
        Err(e) => {
            state.refresh_error = Some(format!("{e}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use buildinfo::version_string;

    #[test]
    fn test_version_string_format() {
        let version = version_string!();
        assert!(
            version.contains('.'),
            "Version string should contain version number: {version}"
        );
        assert!(
            version.contains('(') && version.contains(')'),
            "Version string should contain git info in parentheses: {version}"
        );
        assert!(
            version.contains("clean") || version.contains("dirty"),
            "Version string should contain clean/dirty status: {version}"
        );
    }
}
