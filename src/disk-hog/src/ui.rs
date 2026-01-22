use human_bytes::human_bytes;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Row, Table},
    Frame,
};
use unicode_width::UnicodeWidthStr;

use crate::model::{BytesPerSec, OpsPerSec, ProcessIOStats};

/// Represents the IOPS monitoring mode.
///
/// Using an enum instead of booleans makes the state explicit and ensures
/// all cases are handled in match expressions. This prevents bugs like
/// showing "run with sudo" when the user explicitly disabled IOPS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IopsMode {
    /// IOPS monitoring is enabled and running (root + not --bandwidth-only).
    Enabled,
    /// IOPS monitoring is disabled because we're not running as root.
    DisabledNoRoot,
    /// IOPS monitoring is disabled by user choice (--bandwidth-only flag).
    DisabledByFlag,
}

impl IopsMode {
    /// Returns true if IOPS monitoring is enabled.
    pub fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

/// Application state for rendering.
pub struct AppState {
    /// Bandwidth stats (always available).
    pub bandwidth_stats: Vec<ProcessIOStats>,
    /// IOPS stats (only available when IOPS mode is enabled).
    pub iops_stats: Option<Vec<ProcessIOStats>>,
    /// Maximum number of processes to show per pane.
    pub max_processes: usize,
    /// Current IOPS monitoring mode.
    pub iops_mode: IopsMode,
    /// Whether the IOPS parser encountered an error.
    pub iops_error: bool,
}

impl AppState {
    /// Creates a new app state.
    pub fn new(max_processes: usize, iops_mode: IopsMode) -> Self {
        Self {
            bandwidth_stats: Vec::new(),
            iops_stats: if iops_mode.is_enabled() {
                Some(Vec::new())
            } else {
                None
            },
            max_processes,
            iops_mode,
            iops_error: false,
        }
    }
}

/// Height of the IOPS pane when showing a status message instead of data.
/// 2 (border) + 1 (padding above) + 1 (message line) + 1 (padding below) = 5
const IOPS_MESSAGE_HEIGHT: u16 = 5;

/// Height of the help footer showing keyboard shortcuts.
const HELP_FOOTER_HEIGHT: u16 = 1;

/// Renders the UI.
pub fn render(frame: &mut Frame, state: &AppState) {
    // Reserve space for help footer at the bottom
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(HELP_FOOTER_HEIGHT)])
        .split(frame.area());

    // Split main area into two panes - IOPS pane is smaller when not showing data
    let pane_constraints = if state.iops_mode.is_enabled() {
        vec![Constraint::Percentage(50), Constraint::Percentage(50)]
    } else {
        // Give most space to bandwidth, minimal space for IOPS message
        vec![Constraint::Min(0), Constraint::Length(IOPS_MESSAGE_HEIGHT)]
    };

    let pane_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(pane_constraints)
        .split(main_chunks[0]);

    render_bandwidth_pane(frame, pane_chunks[0], state);
    render_iops_pane(frame, pane_chunks[1], state);
    render_help_footer(frame, main_chunks[1]);
}

/// Renders the help footer showing keyboard shortcuts.
fn render_help_footer(frame: &mut Frame, area: Rect) {
    let help_text = Line::from(vec![
        Span::styled(" Press ", Style::default().fg(Color::DarkGray)),
        Span::styled("q", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Span::styled(" or ", Style::default().fg(Color::DarkGray)),
        Span::styled("Esc", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Span::styled(" to quit ", Style::default().fg(Color::DarkGray)),
    ]);

    let paragraph = Paragraph::new(help_text);
    frame.render_widget(paragraph, area);
}

/// Renders the bandwidth pane (top).
fn render_bandwidth_pane(frame: &mut Frame, area: Rect, state: &AppState) {
    // Title doesn't include unit hint because format_bytes() uses IEC units
    // (KiB, MiB, GiB) which would be confusing with a "bytes/sec" label
    let block = Block::default()
        .title(" Disk Bandwidth ")
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
                truncate_to_width(&stat.name, 20),
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

    let table = Table::new(rows, widths).header(header).block(block);

    frame.render_widget(table, area);
}

/// Renders the IOPS pane (bottom).
fn render_iops_pane(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .title(" Disk IOPS (ops/sec) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    // Handle disabled modes with appropriate messages
    match state.iops_mode {
        IopsMode::DisabledNoRoot => {
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
        IopsMode::DisabledByFlag => {
            let message = Paragraph::new(vec![
                Line::from(""),
                Line::from(Span::styled(
                    "IOPS monitoring disabled (--bandwidth-only)",
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
        IopsMode::Enabled => {
            // Continue to render IOPS data below
        }
    }

    // Show error message if parser failed
    if state.iops_error {
        let message = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "[ERROR] IOPS collection stopped (fs_usage parser failed)",
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::BOLD),
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
                        truncate_to_width(&stat.name, 20),
                        format_ops(stat.read_ops_per_sec),
                        format_ops(stat.write_ops_per_sec),
                        format_ops(stat.total_iops()),
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

    let table = Table::new(rows, widths).header(header).block(block);

    frame.render_widget(table, area);
}

/// Formats a bytes-per-second rate as human-readable string using IEC units.
///
/// Uses IEC binary units (KiB, MiB, GiB) where 1 KiB = 1024 bytes.
/// Examples: "1 KiB", "2.5 MiB", "100 GiB".
///
/// # Precision Note
///
/// The `u64 as f64` cast can lose precision for values exceeding 2^53
/// (~9 petabytes/sec). This is acceptable for disk I/O rates which are
/// unlikely to reach such magnitudes in practice.
fn format_bytes(rate: BytesPerSec) -> String {
    let bytes = rate.as_u64();
    if bytes == 0 {
        "0 B".to_string()
    } else {
        #[expect(
            clippy::cast_precision_loss,
            reason = "Precision loss only occurs above 2^53 (~9 PB/s), far beyond realistic I/O rates"
        )]
        let bytes_f64 = bytes as f64;
        human_bytes(bytes_f64)
    }
}

/// Formats an ops-per-second rate as a string.
fn format_ops(rate: Option<OpsPerSec>) -> String {
    rate.map_or("-".to_string(), |r| r.as_u64().to_string())
}

/// Truncates a string to fit within a maximum display width.
///
/// This function uses Unicode display width (from the `unicode-width` crate)
/// to correctly handle characters that occupy different terminal column widths:
/// - ASCII characters: 1 column
/// - CJK characters (Chinese, Japanese, Korean): 2 columns
/// - Most emoji: 2 columns
///
/// If truncation is needed, appends "..." and ensures the result fits within `max_width`.
fn truncate_to_width(name: &str, max_width: usize) -> String {
    let current_width = name.width();
    if current_width <= max_width {
        return name.to_string();
    }

    // Need to truncate - reserve space for "..."
    let ellipsis = "...";
    let ellipsis_width = ellipsis.width();
    let target_width = max_width.saturating_sub(ellipsis_width);

    let mut result = String::new();
    let mut width = 0;

    for c in name.chars() {
        let char_width = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
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
        assert!(result.width() <= 10);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_truncate_to_width_cjk() {
        // CJK characters are 2 columns wide each
        // "日本語" = 3 characters, 6 columns wide
        // Should fit in width 10
        assert_eq!(truncate_to_width("日本語", 10), "日本語");

        // "日本語プロセス" = 7 characters, 14 columns wide
        // In max_width=10, we can fit 3 CJK chars (6 cols) + "..." (3 cols) = 9 cols
        let result = truncate_to_width("日本語プロセス", 10);
        assert!(result.width() <= 10);
        assert!(result.ends_with("..."));
        // Should be "日本語..." (3 CJK chars = 6 cols + 3 = 9 cols)
        assert_eq!(result, "日本語...");
    }

    #[test]
    fn test_truncate_to_width_emoji() {
        // Most emoji are 2 columns wide
        // "🚀rocket" = 1 emoji (2 cols) + 6 ASCII chars = 8 cols
        assert_eq!(truncate_to_width("🚀rocket", 10), "🚀rocket");

        // Many emoji should be truncated
        let result = truncate_to_width("🚀🚀🚀🚀🚀🚀", 10);
        assert!(result.width() <= 10);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_truncate_to_width_mixed() {
        // Mix of ASCII and CJK: "Hello世界" = 5 ASCII (5 cols) + 2 CJK (4 cols) = 9 cols
        assert_eq!(truncate_to_width("Hello世界", 10), "Hello世界");

        // Longer mixed: "Hello世界Test" = 5 + 4 + 4 = 13 cols, needs truncation
        let result = truncate_to_width("Hello世界Test", 10);
        assert!(result.width() <= 10);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(BytesPerSec(0)), "0 B");
        // human_bytes uses IEC units (KiB, MiB, etc.)
        assert_eq!(format_bytes(BytesPerSec(1024)), "1 KiB");
        assert_eq!(format_bytes(BytesPerSec(1_048_576)), "1 MiB");
    }

    #[test]
    fn test_format_ops() {
        assert_eq!(format_ops(None), "-");
        assert_eq!(format_ops(Some(OpsPerSec(0))), "0");
        assert_eq!(format_ops(Some(OpsPerSec(1234))), "1234");
    }

    #[test]
    fn test_app_state_new_with_iops_enabled() {
        let state = AppState::new(10, IopsMode::Enabled);
        assert_eq!(state.iops_mode, IopsMode::Enabled);
        assert!(state.iops_stats.is_some());
        assert!(!state.iops_error);
    }

    #[test]
    fn test_app_state_new_disabled_no_root() {
        let state = AppState::new(10, IopsMode::DisabledNoRoot);
        assert_eq!(state.iops_mode, IopsMode::DisabledNoRoot);
        assert!(state.iops_stats.is_none());
        assert!(!state.iops_error);
    }

    #[test]
    fn test_app_state_new_disabled_by_flag() {
        let state = AppState::new(10, IopsMode::DisabledByFlag);
        assert_eq!(state.iops_mode, IopsMode::DisabledByFlag);
        assert!(state.iops_stats.is_none());
        assert!(!state.iops_error);
    }

    #[test]
    fn test_iops_mode_is_enabled() {
        assert!(IopsMode::Enabled.is_enabled());
        assert!(!IopsMode::DisabledNoRoot.is_enabled());
        assert!(!IopsMode::DisabledByFlag.is_enabled());
    }
}
