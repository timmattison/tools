//! portcul - A pretty TUI for viewing and killing processes listening on ports.
//!
//! Displays an interactive table of all processes with open listening sockets.
//! Navigate with arrow keys, kill with 'd' or Enter, refresh with 'r'.

use std::io::{self, Write};
use std::time::Duration;

use anyhow::Result;
use buildinfo::version_string;
use clap::{Parser, Subcommand};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{backend::CrosstermBackend, Terminal};

mod process;
mod ui;

use process::{
    collect_listeners, filter_by_port, format_kill_header, format_kill_prompt, format_process_line,
    kill_after_confirmation, kill_process, kill_targets,
};
use ui::{AppState, KillConfirm};

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

    /// Refresh interval in seconds, 0.1 or more (TUI mode only)
    #[arg(short, long, default_value = "2.0", global = true, value_parser = parse_refresh)]
    refresh: Duration,
}

/// The shortest refresh interval that the TUI accepts.
///
/// A shorter interval makes the loop read the sockets and redraw with almost
/// no pause, and a zero interval keeps one CPU busy.
const MIN_REFRESH: Duration = Duration::from_millis(100);

/// Parses the value of `--refresh`, a count of seconds, into a `Duration`.
///
/// # Errors
///
/// Returns a message when the text is not a number, when the interval is
/// shorter than [`MIN_REFRESH`], or when it is too large for a `Duration`.
fn parse_refresh(text: &str) -> Result<Duration, String> {
    let seconds: f64 = text
        .parse()
        .map_err(|e| format!("{text:?} is not a number of seconds: {e}"))?;
    if seconds.is_nan() || seconds < MIN_REFRESH.as_secs_f64() {
        return Err(format!(
            "the interval must be at least {} s, got {text}",
            MIN_REFRESH.as_secs_f64()
        ));
    }
    Duration::try_from_secs_f64(seconds)
        .map_err(|e| format!("the interval {text} is too large: {e}"))
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
fn cmd_tui(refresh: Duration) -> Result<()> {
    // Collect initial data before entering TUI
    let listeners = match collect_listeners() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    // The session erases the screen when it enters, and it restores the
    // terminal when it drops, on an error, and on a panic.
    let mut session = altscreen::RatatuiSession::enter(altscreen::Options::default())?;
    run_app(session.terminal(), refresh, listeners)
}

/// Kills all processes on a given port (CLI mode).
///
/// After the confirmation, it reads the list again and sends SIGTERM only to
/// each process that still listens on the port. With `--yes` there is no
/// prompt, so it uses the list that it read at the start.
fn cmd_kill(port: u16, yes: bool) -> Result<()> {
    let listeners = collect_listeners()?;
    let targets = filter_by_port(&listeners, port);

    if targets.is_empty() {
        eprintln!("No processes found on port {port}");
        std::process::exit(1);
    }

    if yes {
        // Non-interactive: kill immediately
        kill_targets(&targets, &listeners, kill_process)?;
    } else {
        // Interactive: show processes and ask for confirmation.
        // Print one line per address so output is easy to grep/pipe.
        println!("{}", format_kill_header(&targets, port));
        for listener in &targets {
            for address in &listener.addresses {
                println!("{}", format_process_line(listener, address));
            }
        }
        println!();

        eprint!("{}", format_kill_prompt(&targets));
        io::stderr().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        kill_after_confirmation(&targets, &input, collect_listeners, kill_process)?;
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
        for address in &listener.addresses {
            println!("{}", format_process_line(listener, address));
        }
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
    tick_rate: Duration,
    initial_listeners: Vec<process::ListeningProcess>,
) -> Result<()> {
    let mut state = AppState::new(initial_listeners);

    loop {
        terminal.draw(|f| ui::render(f, &mut state))?;

        if event::poll(tick_rate)? {
            if let Some(key) = pressed_key(event::read()?) {
                // Handle kill confirmation mode separately
                if let KillConfirm::Pending { pid, .. } = &state.kill_confirm {
                    let pid = *pid;
                    match key.code {
                        // Read the list again, so the signal goes only to the
                        // process that the user confirmed.
                        KeyCode::Char('y') | KeyCode::Char('Y') => match collect_listeners() {
                            Ok(fresh) => {
                                if state.confirm_kill(fresh, kill_process) {
                                    refresh_listeners(&mut state);
                                }
                            }
                            Err(e) => {
                                state.kill_confirm = KillConfirm::Result {
                                    message: format!(
                                        "Cannot read the listeners, so nothing was sent to PID {pid}: {e}"
                                    ),
                                    is_error: true,
                                };
                            }
                        },
                        // Ctrl+C exits the app even during confirmation
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            break
                        }
                        // n, Esc, and each other key cancel the dialog.
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

/// Returns the key of `event` when the event is a key press.
///
/// On Windows, crossterm reports a press and a release for each key. Only the
/// press counts, so one key moves the selection one row, and the release of
/// `d` does not cancel the dialog that its press opened.
fn pressed_key(event: Event) -> Option<KeyEvent> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => Some(key),
        _ => None,
    }
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
    use std::time::Duration;

    use clap::Parser;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    use super::{pressed_key, Cli};

    #[test]
    fn test_pressed_key_ignores_a_key_release() {
        let release = KeyEvent::new_with_kind(
            KeyCode::Char('j'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        assert_eq!(pressed_key(Event::Key(release)), None);
    }

    #[test]
    fn test_pressed_key_gives_a_key_press() {
        let press =
            KeyEvent::new_with_kind(KeyCode::Char('j'), KeyModifiers::NONE, KeyEventKind::Press);
        assert_eq!(pressed_key(Event::Key(press)), Some(press));
    }

    #[test]
    fn test_refresh_refuses_an_interval_too_large_for_a_duration() {
        // Duration::from_secs_f64 panics on this value, after raw mode is on.
        assert!(Cli::try_parse_from(["portcul", "--refresh", "1e300"]).is_err());
    }

    #[test]
    fn test_refresh_refuses_an_interval_below_the_floor() {
        // A zero Duration makes event::poll return at once: 100% CPU.
        assert!(Cli::try_parse_from(["portcul", "--refresh", "1e-12"]).is_err());
        assert!(Cli::try_parse_from(["portcul", "--refresh", "0.05"]).is_err());
    }

    #[test]
    fn test_refresh_refuses_a_value_that_is_not_a_positive_number() {
        for value in ["0", "-1", "nan", "inf"] {
            let arg = format!("--refresh={value}");
            assert!(
                Cli::try_parse_from(["portcul", arg.as_str()]).is_err(),
                "{value} must be refused"
            );
        }
    }

    #[test]
    fn test_refresh_accepts_the_floor_and_the_default() {
        assert!(Cli::try_parse_from(["portcul", "--refresh", "0.1"]).is_ok());
        assert!(Cli::try_parse_from(["portcul"]).is_ok());
        let cli = Cli::try_parse_from(["portcul"]).unwrap();
        assert_eq!(cli.refresh, Duration::from_secs(2));
    }

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
