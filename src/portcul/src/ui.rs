use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Row, Table, TableState},
    Frame,
};
use unicode_width::UnicodeWidthChar;

use crate::process::{format_not_listening, still_listening, ListeningProcess, Pid};

/// Whether a kill confirmation dialog is active.
#[derive(Debug, Clone)]
pub enum KillConfirm {
    /// No confirmation dialog shown.
    None,
    /// Asking user to confirm killing a process.
    Pending { pid: Pid, name: String, port: u16 },
    /// Kill was attempted, showing result.
    Result { message: String, is_error: bool },
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

    /// Sends the kill that the confirmation dialog asked for.
    ///
    /// `fresh` is the list of listeners, read after the user confirmed. The
    /// signal goes out through `kill` only when `fresh` still has a row with
    /// the PID, the name, and the port of the dialog. Else the process exited,
    /// and the OS can give its PID to a different process, so nothing is sent.
    /// The state takes `fresh` as its list, and the dialog shows the result.
    ///
    /// Returns `true` when the signal went out.
    pub fn confirm_kill(
        &mut self,
        fresh: Vec<ListeningProcess>,
        kill: impl FnOnce(Pid) -> anyhow::Result<()>,
    ) -> bool {
        let KillConfirm::Pending { pid, name, port } = &self.kill_confirm else {
            return false;
        };
        let (pid, name, port) = (*pid, name.clone(), *port);
        let listens = still_listening(&fresh, pid, &name, port);
        self.refresh(fresh);
        if !listens {
            self.kill_confirm = KillConfirm::Result {
                message: format_not_listening(&name, pid, port),
                is_error: true,
            };
            return false;
        }
        match kill(pid) {
            Ok(()) => {
                self.kill_confirm = KillConfirm::Result {
                    message: format!("Sent SIGTERM to {name} (PID {pid})"),
                    is_error: false,
                };
                true
            }
            Err(e) => {
                self.kill_confirm = KillConfirm::Result {
                    message: format!("Failed to kill PID {pid}: {e}"),
                    is_error: true,
                };
                false
            }
        }
    }

    /// Refreshes the listener list, and keeps the selection on the same row.
    ///
    /// A row is the same row when it has the same PID and port, because one
    /// process can listen on more than one port. When the selected row is
    /// gone, the selection stays at the same index, or moves to the last row
    /// when the list is now shorter.
    pub fn refresh(&mut self, new_listeners: Vec<ListeningProcess>) {
        let old_index = self.table_state.selected();
        let selected_row = self.selected_listener().map(|l| (l.pid, l.port));
        self.listeners = new_listeners;

        let Some(last) = self.listeners.len().checked_sub(1) else {
            self.table_state.select(None);
            return;
        };
        let new_index = selected_row
            .and_then(|(pid, port)| {
                self.listeners
                    .iter()
                    .position(|l| l.pid == pid && l.port == port)
            })
            .unwrap_or_else(|| old_index.unwrap_or(0).min(last));
        self.table_state.select(Some(new_index));
    }
}

/// Width of the address column: the tree prefix `└─ ` (3 columns) and the
/// longest IPv6 address (39 columns).
const ADDRESS_COLUMN_WIDTH: u16 = 42;

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
                listener.pid.as_u32().to_string(),
                truncate_to_width(&listener.name, 30),
                format_address_cell(&listener.addresses),
            ])
            .height(address_cell_height(listener.addresses.len()))
        })
        .collect();

    let widths = [
        Constraint::Length(7), // Port
        Constraint::Length(8), // PID
        Constraint::Min(15),   // Process
        Constraint::Length(ADDRESS_COLUMN_WIDTH),
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
            Line::from(Span::styled(
                format!(" {message}"),
                Style::default().fg(color),
            ))
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
            Span::styled(
                "y",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" confirm  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "n",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("/", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" cancel", Style::default().fg(Color::DarkGray)),
        ],
        _ => vec![
            Span::styled(" ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "Up/Down",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" navigate  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "d/Enter",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" kill  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "r",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" refresh  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "q",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("/", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" quit", Style::default().fg(Color::DarkGray)),
        ],
    };

    let paragraph = Paragraph::new(Line::from(help_spans));
    frame.render_widget(paragraph, area);
}

/// Renders the address column content for a `ListeningProcess`.
///
/// For a single address, returns the address directly.
/// For multiple addresses, returns a multi-line tree with "Multiple" on the
/// first line and each address on its own line prefixed by tree characters
/// (`├─` for middle entries, `└─` for the last).
fn format_address_cell(addresses: &[String]) -> String {
    if addresses.len() <= 1 {
        return addresses.first().cloned().unwrap_or_default();
    }

    let mut out = String::from("Multiple");
    let last_idx = addresses.len() - 1;
    for (i, addr) in addresses.iter().enumerate() {
        let prefix = if i == last_idx { "└─" } else { "├─" };
        out.push('\n');
        out.push_str(prefix);
        out.push(' ');
        out.push_str(addr);
    }
    out
}

/// Returns the Ratatui row height needed to render a cell produced by
/// [`format_address_cell`] with `address_count` addresses.
fn address_cell_height(address_count: usize) -> u16 {
    if address_count <= 1 {
        1
    } else {
        // header "Multiple" + one row per address
        u16::try_from(address_count + 1).unwrap_or(u16::MAX)
    }
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
    fn test_format_address_cell_single() {
        let cell = format_address_cell(&["0.0.0.0".to_string()]);
        assert_eq!(cell, "0.0.0.0");
    }

    #[test]
    fn test_format_address_cell_multiple_uses_tree() {
        let cell = format_address_cell(&[
            "0.0.0.0".to_string(),
            "127.0.0.1".to_string(),
            "::".to_string(),
        ]);
        let lines: Vec<&str> = cell.split('\n').collect();
        assert_eq!(lines.len(), 4, "expected 1 header line + 3 address lines");
        assert_eq!(lines[0], "Multiple");
        assert_eq!(lines[1], "├─ 0.0.0.0");
        assert_eq!(lines[2], "├─ 127.0.0.1");
        assert_eq!(lines[3], "└─ ::");
    }

    #[test]
    fn test_format_address_cell_two_addresses() {
        let cell = format_address_cell(&["0.0.0.0".to_string(), "::".to_string()]);
        let lines: Vec<&str> = cell.split('\n').collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "Multiple");
        assert_eq!(lines[1], "├─ 0.0.0.0");
        assert_eq!(lines[2], "└─ ::");
    }

    #[test]
    fn test_address_column_fits_the_longest_ipv6_row() {
        // The longest text of an IPv6 address: 8 groups of 4 hex digits.
        let longest_ipv6 = std::net::Ipv6Addr::from(u128::MAX).to_string();
        let cell = format_address_cell(&["0.0.0.0".to_string(), longest_ipv6]);
        let widest = cell
            .lines()
            .map(unicode_width::UnicodeWidthStr::width)
            .max()
            .unwrap();
        assert!(
            usize::from(ADDRESS_COLUMN_WIDTH) >= widest,
            "the address column is {ADDRESS_COLUMN_WIDTH} columns, but a row needs {widest}"
        );
    }

    #[test]
    fn test_address_cell_height_single() {
        assert_eq!(address_cell_height(1), 1);
    }

    #[test]
    fn test_address_cell_height_multiple() {
        // header row + one line per address
        assert_eq!(address_cell_height(2), 3);
        assert_eq!(address_cell_height(3), 4);
        assert_eq!(address_cell_height(5), 6);
    }

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
        // Each of these emoji is 2 columns wide.
        assert_eq!(truncate_to_width("🎉🎊🎁🎈🎂", 10), "🎉🎊🎁🎈🎂");

        // 12 columns cut to 8: 5 columns for the text, then "...". The third
        // emoji would need columns 5 and 6, so the cut falls after the second.
        let result = truncate_to_width("🎉🎊🎁🎈🎂🎉", 8);
        assert!(unicode_width::UnicodeWidthStr::width(result.as_str()) <= 8);
        assert_eq!(result, "🎉🎊...");
    }

    #[test]
    fn test_truncate_to_width_accented() {
        assert_eq!(truncate_to_width("café", 10), "café");
        // "café au lait" is 12 columns; with max 8, keep 5 columns + "...".
        assert_eq!(truncate_to_width("café au lait", 8), "café ...");
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
            pid: Pid::try_from(1234_u32).unwrap(),
            name: "test".to_string(),
            port: 8080,
            addresses: vec!["0.0.0.0".to_string()],
        }];
        let state = AppState::new(listeners);
        assert_eq!(state.table_state.selected(), Some(0));
    }

    #[test]
    fn test_select_next_wraps() {
        let listeners = vec![
            ListeningProcess {
                pid: Pid::try_from(1_u32).unwrap(),
                name: "a".to_string(),
                port: 80,
                addresses: vec!["0.0.0.0".to_string()],
            },
            ListeningProcess {
                pid: Pid::try_from(2_u32).unwrap(),
                name: "b".to_string(),
                port: 443,
                addresses: vec!["0.0.0.0".to_string()],
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
                pid: Pid::try_from(1_u32).unwrap(),
                name: "a".to_string(),
                port: 80,
                addresses: vec!["0.0.0.0".to_string()],
            },
            ListeningProcess {
                pid: Pid::try_from(2_u32).unwrap(),
                name: "b".to_string(),
                port: 443,
                addresses: vec!["0.0.0.0".to_string()],
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

    fn row(pid: u32, port: u16) -> ListeningProcess {
        ListeningProcess {
            pid: Pid::try_from(pid).unwrap(),
            name: format!("proc-{pid}"),
            port,
            addresses: vec!["0.0.0.0".to_string()],
        }
    }

    /// A state whose dialog asks to kill `row(100, 8080)`.
    fn pending_kill_of_100() -> AppState {
        let target = row(100, 8080);
        let mut state = AppState::new(vec![target.clone()]);
        state.kill_confirm = KillConfirm::Pending {
            pid: target.pid,
            name: target.name,
            port: target.port,
        };
        state
    }

    fn result_of(state: &AppState) -> (String, bool) {
        match &state.kill_confirm {
            KillConfirm::Result { message, is_error } => (message.clone(), *is_error),
            other => panic!("expected a result, got {other:?}"),
        }
    }

    #[test]
    fn test_confirm_kill_sends_nothing_when_the_process_exited() {
        let mut state = pending_kill_of_100();
        let called = std::cell::Cell::new(false);
        let sent = state.confirm_kill(vec![row(200, 3000)], |_| {
            called.set(true);
            Ok(())
        });
        assert!(!sent);
        assert!(
            !called.get(),
            "the PID can belong to a different process now"
        );
        let (message, is_error) = result_of(&state);
        assert!(is_error);
        assert!(message.contains("no longer listens"), "{message}");
    }

    #[test]
    fn test_confirm_kill_sends_nothing_when_the_pid_has_a_new_owner() {
        let mut state = pending_kill_of_100();
        let mut reused = row(100, 8080);
        reused.name = "other-program".to_string();
        let called = std::cell::Cell::new(false);
        let sent = state.confirm_kill(vec![reused], |_| {
            called.set(true);
            Ok(())
        });
        assert!(!sent);
        assert!(!called.get());
    }

    #[test]
    fn test_confirm_kill_sends_the_signal_when_the_row_is_still_there() {
        let mut state = pending_kill_of_100();
        let target = Pid::try_from(100_u32).unwrap();
        let called_with = std::cell::Cell::new(None);
        let sent = state.confirm_kill(vec![row(100, 8080)], |pid| {
            called_with.set(Some(pid));
            Ok(())
        });
        assert!(sent);
        assert_eq!(called_with.get(), Some(target));
        let (message, is_error) = result_of(&state);
        assert!(!is_error);
        assert_eq!(message, "Sent SIGTERM to proc-100 (PID 100)");
    }

    #[test]
    fn test_refresh_keeps_the_second_port_of_one_pid() {
        // One process that listens on two ports has two rows.
        let rows = vec![row(100, 80), row(100, 443)];
        let mut state = AppState::new(rows.clone());
        state.select_next();
        assert_eq!(state.selected_listener().unwrap().port, 443);

        state.refresh(rows);
        assert_eq!(state.table_state.selected(), Some(1));
        assert_eq!(state.selected_listener().unwrap().port, 443);
    }

    #[test]
    fn test_refresh_moves_to_the_nearest_row_when_the_selected_row_is_gone() {
        let mut state = AppState::new(vec![row(1, 80), row(2, 443), row(3, 8080)]);
        state.select_next(); // row(2, 443)

        state.refresh(vec![row(1, 80), row(3, 8080)]);
        assert_eq!(state.table_state.selected(), Some(1));
    }

    #[test]
    fn test_refresh_clamps_when_the_last_row_is_gone() {
        let mut state = AppState::new(vec![row(1, 80), row(2, 443), row(3, 8080)]);
        state.select_previous(); // wraps to row(3, 8080)

        state.refresh(vec![row(1, 80), row(2, 443)]);
        assert_eq!(state.table_state.selected(), Some(1));
    }

    #[test]
    fn test_refresh_preserves_selection_by_pid() {
        let listeners = vec![
            ListeningProcess {
                pid: Pid::try_from(100_u32).unwrap(),
                name: "nginx".to_string(),
                port: 80,
                addresses: vec!["0.0.0.0".to_string()],
            },
            ListeningProcess {
                pid: Pid::try_from(200_u32).unwrap(),
                name: "node".to_string(),
                port: 3000,
                addresses: vec!["127.0.0.1".to_string()],
            },
        ];
        let mut state = AppState::new(listeners);
        state.select_next(); // select PID 200
        assert_eq!(
            state.selected_listener().unwrap().pid,
            Pid::try_from(200_u32).unwrap()
        );

        // Refresh with new list where PID 200 is now first (new process added before it)
        let new_listeners = vec![
            ListeningProcess {
                pid: Pid::try_from(50_u32).unwrap(),
                name: "redis".to_string(),
                port: 6379,
                addresses: vec!["127.0.0.1".to_string()],
            },
            ListeningProcess {
                pid: Pid::try_from(200_u32).unwrap(),
                name: "node".to_string(),
                port: 3000,
                addresses: vec!["127.0.0.1".to_string()],
            },
        ];
        state.refresh(new_listeners);
        assert_eq!(
            state.selected_listener().unwrap().pid,
            Pid::try_from(200_u32).unwrap()
        );
        assert_eq!(state.table_state.selected(), Some(1));
    }
}
