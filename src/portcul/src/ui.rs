use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Row, Table, TableState},
    Frame,
};
use unicode_width::UnicodeWidthChar;

use crate::process::ListeningProcess;

/// Whether a kill confirmation dialog is active.
#[derive(Debug, Clone)]
pub enum KillConfirm {
    /// No confirmation dialog shown.
    None,
    /// Asking user to confirm killing a process.
    Pending {
        pid: u32,
        name: String,
        port: u16,
    },
    /// Kill was attempted, showing result.
    Result {
        message: String,
        is_error: bool,
    },
}

/// Application state for rendering.
pub struct AppState {
    /// Currently discovered listening processes.
    pub listeners: Vec<ListeningProcess>,
    /// Table selection state for ratatui.
    pub table_state: TableState,
    /// Current kill confirmation state.
    pub kill_confirm: KillConfirm,
    /// Error message from last refresh, if any.
    pub refresh_error: Option<String>,
}

impl AppState {
    /// Creates a new app state with an initial list of listeners.
    pub fn new(listeners: Vec<ListeningProcess>) -> Self {
        let mut table_state = TableState::default();
        if !listeners.is_empty() {
            table_state.select(Some(0));
        }
        Self {
            listeners,
            table_state,
            kill_confirm: KillConfirm::None,
            refresh_error: None,
        }
    }

    /// Returns the currently selected listener, if any.
    pub fn selected_listener(&self) -> Option<&ListeningProcess> {
        self.table_state
            .selected()
            .and_then(|i| self.listeners.get(i))
    }

    /// Moves the selection up by one row.
    pub fn select_previous(&mut self) {
        if self.listeners.is_empty() {
            return;
        }
        let i = match self.table_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.listeners.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.table_state.select(Some(i));
    }

    /// Moves the selection down by one row.
    pub fn select_next(&mut self) {
        if self.listeners.is_empty() {
            return;
        }
        let i = match self.table_state.selected() {
            Some(i) => {
                if i >= self.listeners.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.table_state.select(Some(i));
    }

    /// Refreshes the listener list, preserving selection position where possible.
    pub fn refresh(&mut self, new_listeners: Vec<ListeningProcess>) {
        let selected_pid = self.selected_listener().map(|l| l.pid);
        self.listeners = new_listeners;

        if self.listeners.is_empty() {
            self.table_state.select(None);
        } else {
            // Try to re-select the same PID
            let new_index = selected_pid
                .and_then(|pid| self.listeners.iter().position(|l| l.pid == pid))
                .unwrap_or(0)
                .min(self.listeners.len() - 1);
            self.table_state.select(Some(new_index));
        }
    }
}

/// Height of the help footer.
const HELP_FOOTER_HEIGHT: u16 = 1;

/// Height of the status bar (for kill confirmation / errors).
const STATUS_BAR_HEIGHT: u16 = 1;

/// Renders the full UI.
pub fn render(frame: &mut Frame, state: &mut AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(STATUS_BAR_HEIGHT),
            Constraint::Length(HELP_FOOTER_HEIGHT),
        ])
        .split(frame.area());

    render_table(frame, chunks[0], state);
    render_status_bar(frame, chunks[1], state);
    render_help_footer(frame, chunks[2], state);
}

/// Renders the process table.
fn render_table(frame: &mut Frame, area: Rect, state: &mut AppState) {
    let block = Block::default()
        .title(" Listening Ports ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    if state.listeners.is_empty() {
        let message = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "No processes listening on any ports",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            )),
        ])
        .block(block)
        .alignment(ratatui::layout::Alignment::Center);

        frame.render_widget(message, area);
        return;
    }

    let header = Row::new(vec!["Port", "PID", "Process", "Address"])
        .style(
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .bottom_margin(1);

    let rows: Vec<Row> = state
        .listeners
        .iter()
        .map(|listener| {
            Row::new(vec![
                listener.port.to_string(),
                listener.pid.to_string(),
                truncate_to_width(&listener.name, 30),
                listener.address.clone(),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(7),  // Port
        Constraint::Length(8),  // PID
        Constraint::Min(15),    // Process
        Constraint::Length(40), // Address
    ];

    let highlight_style = Style::default()
        .bg(Color::DarkGray)
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(highlight_style)
        .highlight_symbol("> ");

    frame.render_stateful_widget(table, area, &mut state.table_state);
}

/// Renders the status bar for kill confirmations and errors.
fn render_status_bar(frame: &mut Frame, area: Rect, state: &AppState) {
    let line = match &state.kill_confirm {
        KillConfirm::None => {
            if let Some(err) = &state.refresh_error {
                Line::from(Span::styled(
                    format!(" [ERROR] {err}"),
                    Style::default().fg(Color::Red),
                ))
            } else {
                Line::from("")
            }
        }
        KillConfirm::Pending { pid, name, port } => Line::from(vec![
            Span::styled(" Kill ", Style::default().fg(Color::Yellow)),
            Span::styled(
                format!("{name} (PID {pid}, port {port})"),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("? ", Style::default().fg(Color::Yellow)),
            Span::styled(
                "[y/n]",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        KillConfirm::Result { message, is_error } => {
            let color = if *is_error { Color::Red } else { Color::Green };
            Line::from(Span::styled(format!(" {message}"), Style::default().fg(color)))
        }
    };

    let paragraph = Paragraph::new(line);
    frame.render_widget(paragraph, area);
}

/// Renders the help footer with keyboard shortcuts.
fn render_help_footer(frame: &mut Frame, area: Rect, state: &AppState) {
    let help_spans = match &state.kill_confirm {
        KillConfirm::Pending { .. } => vec![
            Span::styled(" ", Style::default().fg(Color::DarkGray)),
            Span::styled("y", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled(" confirm  ", Style::default().fg(Color::DarkGray)),
            Span::styled("n", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled("/", Style::default().fg(Color::DarkGray)),
            Span::styled("Esc", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled(" cancel", Style::default().fg(Color::DarkGray)),
        ],
        _ => vec![
            Span::styled(" ", Style::default().fg(Color::DarkGray)),
            Span::styled("Up/Down", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled(" navigate  ", Style::default().fg(Color::DarkGray)),
            Span::styled("d/Enter", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled(" kill  ", Style::default().fg(Color::DarkGray)),
            Span::styled("r", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled(" refresh  ", Style::default().fg(Color::DarkGray)),
            Span::styled("q", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled("/", Style::default().fg(Color::DarkGray)),
            Span::styled("Esc", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled(" quit", Style::default().fg(Color::DarkGray)),
        ],
    };

    let paragraph = Paragraph::new(Line::from(help_spans));
    frame.render_widget(paragraph, area);
}

/// Minimum width for truncation (ellipsis + at least 1 char).
const MIN_TRUNCATION_WIDTH: usize = 4;

/// Truncates a string to fit within a maximum display width.
///
/// Uses Unicode display width to correctly handle CJK and emoji characters.
/// If truncation is needed, appends "..." and ensures the result fits.
///
/// # Panics (debug builds only)
///
/// Debug-asserts that `max_width >= MIN_TRUNCATION_WIDTH`.
fn truncate_to_width(name: &str, max_width: usize) -> String {
    debug_assert!(
        max_width >= MIN_TRUNCATION_WIDTH,
        "truncate_to_width requires max_width >= {MIN_TRUNCATION_WIDTH}, got {max_width}"
    );

    let current_width = unicode_width::UnicodeWidthStr::width(name);
    if current_width <= max_width {
        return name.to_string();
    }

    let ellipsis = "...";
    let ellipsis_width = unicode_width::UnicodeWidthStr::width(ellipsis);
    let target_width = max_width.saturating_sub(ellipsis_width);

    let mut result = String::new();
    let mut width = 0;

    for c in name.chars() {
        let char_width = UnicodeWidthChar::width(c).unwrap_or(0);
        if width + char_width > target_width {
            break;
        }
        result.push(c);
        width += char_width;
    }

    result.push_str(ellipsis);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_to_width_short() {
        assert_eq!(truncate_to_width("short", 10), "short");
    }

    #[test]
    fn test_truncate_to_width_exact() {
        assert_eq!(truncate_to_width("exactly10!", 10), "exactly10!");
    }

    #[test]
    fn test_truncate_to_width_long() {
        let result = truncate_to_width("this is a very long name", 10);
        assert!(unicode_width::UnicodeWidthStr::width(result.as_str()) <= 10);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_truncate_to_width_cjk() {
        // CJK characters are 2 columns wide each
        assert_eq!(truncate_to_width("日本語", 10), "日本語");

        // "日本語プロセス" = 14 cols, truncate to 10
        let result = truncate_to_width("日本語プロセス", 10);
        assert!(unicode_width::UnicodeWidthStr::width(result.as_str()) <= 10);
        assert!(result.ends_with("..."));
        assert_eq!(result, "日本語...");
    }

    #[test]
    fn test_truncate_to_width_emoji() {
        assert_eq!(truncate_to_width("rocket", 10), "rocket");

        let result = truncate_to_width("long-process-name-here", 10);
        assert!(unicode_width::UnicodeWidthStr::width(result.as_str()) <= 10);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_truncate_to_width_at_minimum() {
        let result = truncate_to_width("abcdef", 4);
        assert!(unicode_width::UnicodeWidthStr::width(result.as_str()) <= 4);
        assert_eq!(result, "a...");
    }

    #[test]
    fn test_app_state_new_empty() {
        let state = AppState::new(vec![]);
        assert!(state.listeners.is_empty());
        assert!(state.table_state.selected().is_none());
    }

    #[test]
    fn test_app_state_new_with_items() {
        let listeners = vec![ListeningProcess {
            pid: 1234,
            name: "test".to_string(),
            port: 8080,
            address: "0.0.0.0".to_string(),
        }];
        let state = AppState::new(listeners);
        assert_eq!(state.table_state.selected(), Some(0));
    }

    #[test]
    fn test_select_next_wraps() {
        let listeners = vec![
            ListeningProcess {
                pid: 1,
                name: "a".to_string(),
                port: 80,
                address: "0.0.0.0".to_string(),
            },
            ListeningProcess {
                pid: 2,
                name: "b".to_string(),
                port: 443,
                address: "0.0.0.0".to_string(),
            },
        ];
        let mut state = AppState::new(listeners);
        assert_eq!(state.table_state.selected(), Some(0));
        state.select_next();
        assert_eq!(state.table_state.selected(), Some(1));
        state.select_next();
        assert_eq!(state.table_state.selected(), Some(0)); // wraps
    }

    #[test]
    fn test_select_previous_wraps() {
        let listeners = vec![
            ListeningProcess {
                pid: 1,
                name: "a".to_string(),
                port: 80,
                address: "0.0.0.0".to_string(),
            },
            ListeningProcess {
                pid: 2,
                name: "b".to_string(),
                port: 443,
                address: "0.0.0.0".to_string(),
            },
        ];
        let mut state = AppState::new(listeners);
        assert_eq!(state.table_state.selected(), Some(0));
        state.select_previous();
        assert_eq!(state.table_state.selected(), Some(1)); // wraps
    }

    #[test]
    fn test_select_on_empty_is_noop() {
        let mut state = AppState::new(vec![]);
        state.select_next();
        assert!(state.table_state.selected().is_none());
        state.select_previous();
        assert!(state.table_state.selected().is_none());
    }

    #[test]
    fn test_refresh_preserves_selection_by_pid() {
        let listeners = vec![
            ListeningProcess {
                pid: 100,
                name: "nginx".to_string(),
                port: 80,
                address: "0.0.0.0".to_string(),
            },
            ListeningProcess {
                pid: 200,
                name: "node".to_string(),
                port: 3000,
                address: "127.0.0.1".to_string(),
            },
        ];
        let mut state = AppState::new(listeners);
        state.select_next(); // select PID 200
        assert_eq!(state.selected_listener().unwrap().pid, 200);

        // Refresh with new list where PID 200 is now first (new process added before it)
        let new_listeners = vec![
            ListeningProcess {
                pid: 50,
                name: "redis".to_string(),
                port: 6379,
                address: "127.0.0.1".to_string(),
            },
            ListeningProcess {
                pid: 200,
                name: "node".to_string(),
                port: 3000,
                address: "127.0.0.1".to_string(),
            },
        ];
        state.refresh(new_listeners);
        assert_eq!(state.selected_listener().unwrap().pid, 200);
        assert_eq!(state.table_state.selected(), Some(1));
    }
}
