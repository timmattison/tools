//! portcul - A pretty TUI for viewing and killing processes listening on ports.
//!
//! Displays an interactive table of all processes with open listening sockets.
//! Navigate with arrow keys, kill with 'k' or Enter, refresh with 'r'.

use std::io::{self, Write};
use std::time::Duration;

use anyhow::Result;
use buildinfo::version_string;
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

mod process;
mod ui;

use process::{collect_listeners, kill_process};
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
            let _ = io::stdout().write_all(b"\x1B[?25h");
            let _ = io::stdout().flush();
        }
    }
}

/// A pretty TUI for viewing and killing processes listening on ports.
///
/// Navigate with arrow keys, kill selected process with 'k' or Enter,
/// refresh with 'r', quit with 'q' or Esc.
#[derive(Parser)]
#[command(
    name = "portcul",
    version = version_string!(),
    about = "A pretty TUI for viewing and killing processes listening on ports"
)]
struct Args {
    /// Refresh interval in seconds
    #[arg(short, long, default_value = "2.0")]
    refresh: f64,
}

fn main() -> Result<()> {
    let args = Args::parse();

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

    let result = run_app(&mut terminal, args, listeners);

    guard.disarm();

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

/// Runs the main application loop.
///
/// # Errors
///
/// Returns an error if terminal drawing or event polling fails.
fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    args: Args,
    initial_listeners: Vec<process::ListeningProcess>,
) -> Result<()> {
    let tick_rate = Duration::from_secs_f64(args.refresh);
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
                        _ => {
                            state.kill_confirm = KillConfirm::None;
                        }
                    }
                    continue;
                }

                // Clear result messages on any keypress
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
