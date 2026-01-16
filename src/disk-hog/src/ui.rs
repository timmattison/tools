use human_bytes::human_bytes;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Row, Table},
    Frame,
};

use crate::model::ProcessIOStats;

/// Application state for rendering.
pub struct AppState {
    /// Bandwidth stats (always available).
    pub bandwidth_stats: Vec<ProcessIOStats>,
    /// IOPS stats (only available when running as root).
    pub iops_stats: Option<Vec<ProcessIOStats>>,
    /// Maximum number of processes to show per pane.
    pub max_processes: usize,
    /// Whether running as root.
    pub is_root: bool,
}

impl AppState {
    /// Creates a new app state.
    pub fn new(max_processes: usize, is_root: bool) -> Self {
        Self {
            bandwidth_stats: Vec::new(),
            iops_stats: if is_root { Some(Vec::new()) } else { None },
            max_processes,
            is_root,
        }
    }
}

/// Height of the IOPS pane when showing the "not root" message.
/// 2 (border) + 1 (padding above) + 1 (message line) + 1 (padding below) = 5
const IOPS_MESSAGE_HEIGHT: u16 = 5;

/// Renders the UI.
pub fn render(frame: &mut Frame, state: &AppState) {
    // Split into two panes - IOPS pane is smaller when not showing data
    let constraints = if state.is_root {
        vec![Constraint::Percentage(50), Constraint::Percentage(50)]
    } else {
        // Give most space to bandwidth, minimal space for IOPS message
        vec![Constraint::Min(0), Constraint::Length(IOPS_MESSAGE_HEIGHT)]
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(frame.area());

    render_bandwidth_pane(frame, chunks[0], state);
    render_iops_pane(frame, chunks[1], state);
}

/// Renders the bandwidth pane (top).
fn render_bandwidth_pane(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .title(" Disk Bandwidth (bytes/sec) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    // Table header
    let header = Row::new(vec!["PID", "Name", "Read/s", "Write/s", "Total/s"])
        .style(Style::default().add_modifier(Modifier::BOLD))
        .bottom_margin(1);

    // Table rows
    let rows: Vec<Row> = state
        .bandwidth_stats
        .iter()
        .take(state.max_processes)
        .map(|stat| {
            Row::new(vec![
                stat.pid.to_string(),
                truncate_name(&stat.name, 20),
                format_bytes(stat.read_bytes_per_sec),
                format_bytes(stat.write_bytes_per_sec),
                format_bytes(stat.total_bandwidth()),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(8),  // PID
        Constraint::Min(15),    // Name
        Constraint::Length(12), // Read/s
        Constraint::Length(12), // Write/s
        Constraint::Length(12), // Total/s
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(Style::default().add_modifier(Modifier::BOLD));

    frame.render_widget(table, area);
}

/// Renders the IOPS pane (bottom).
fn render_iops_pane(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .title(" Disk IOPS (ops/sec) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    if !state.is_root {
        // Show compact message when not running as root
        let message = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "Run with sudo to enable IOPS monitoring (e.g., sudo disk-hog)",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::ITALIC),
            )),
        ])
        .block(block)
        .alignment(ratatui::layout::Alignment::Center);

        frame.render_widget(message, area);
        return;
    }

    // Table header
    let header = Row::new(vec!["PID", "Name", "Read Ops/s", "Write Ops/s", "Total IOPS"])
        .style(Style::default().add_modifier(Modifier::BOLD))
        .bottom_margin(1);

    // Table rows
    let rows: Vec<Row> = state
        .iops_stats
        .as_ref()
        .map(|stats| {
            stats
                .iter()
                .take(state.max_processes)
                .map(|stat| {
                    Row::new(vec![
                        stat.pid.to_string(),
                        truncate_name(&stat.name, 20),
                        stat.read_ops_per_sec
                            .map_or("-".to_string(), |v| v.to_string()),
                        stat.write_ops_per_sec
                            .map_or("-".to_string(), |v| v.to_string()),
                        stat.total_iops().map_or("-".to_string(), |v| v.to_string()),
                    ])
                })
                .collect()
        })
        .unwrap_or_default();

    let widths = [
        Constraint::Length(8),  // PID
        Constraint::Min(15),    // Name
        Constraint::Length(12), // Read Ops/s
        Constraint::Length(12), // Write Ops/s
        Constraint::Length(12), // Total IOPS
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(Style::default().add_modifier(Modifier::BOLD));

    frame.render_widget(table, area);
}

/// Formats bytes as human-readable string (e.g., "1.5 MB").
fn format_bytes(bytes: u64) -> String {
    if bytes == 0 {
        "0 B".to_string()
    } else {
        human_bytes(bytes as f64)
    }
}

/// Truncates a name to max_len characters, adding "..." if truncated.
fn truncate_name(name: &str, max_len: usize) -> String {
    if name.len() <= max_len {
        name.to_string()
    } else {
        format!("{}...", &name[..max_len.saturating_sub(3)])
    }
}
