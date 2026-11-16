//! The screen of cctop: the table of sessions, the change log, and the
//! status line.

use chrono::TimeZone;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, HighlightSpacing, Paragraph, Row, Table, TableState};
use ratatui::text::{Line, Span, Text};
use ratatui::Frame;

use crate::app::App;
use crate::board::Change;
use crate::state::{EpochMillis, PaneState};

/// Draw the screen: the table of sessions, the change log, and the status
/// line. `now` gives the time that the elapsed times count to.
pub fn draw(frame: &mut Frame, app: &App, now: EpochMillis) {
    let [table_area, log_area, status_area] = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(LOG_HEIGHT),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    draw_table(frame, table_area, app, now);
    draw_log(frame, log_area, app);
    draw_status(frame, status_area, app);
}

/// The text of the bottom line when there is no status.
const KEY_HELP: &str = "j/k move · enter focus · n next needs-you · q quit";

/// Draw the status text, or the help for the keys, in `area`.
fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    let text = app.status().unwrap_or(KEY_HELP);
    frame.render_widget(Paragraph::new(text), area);
}

/// The number of changes that the log shows.
const LOG_LINES: usize = 8;

/// The height of the log block: its lines and its two borders.
#[allow(clippy::cast_possible_truncation, reason = "LOG_LINES is a small constant")]
const LOG_HEIGHT: u16 = LOG_LINES as u16 + 2;

/// Draw the newest changes in `area`, the newest first.
fn draw_log(frame: &mut Frame, area: Rect, app: &App) {
    let lines: Vec<Line> = app
        .board()
        .changes()
        .take(LOG_LINES)
        .map(|change| change_line(change, &chrono::Local))
        .collect();
    let block = Block::bordered().title(" Changes ");
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// The line of the log for one change: `HH:MM:SS  <label>  <old> → <new>`.
fn change_line<Tz: TimeZone>(change: &Change, zone: &Tz) -> Line<'static>
where
    Tz::Offset: std::fmt::Display,
{
    Line::from(vec![
        Span::raw(clock_time(change.at, zone)),
        Span::raw(format!("  {}  ", change.label)),
        state_span(change.old),
        Span::raw(" → "),
        state_span(change.new),
    ])
}

/// The label of a state in its color, or a dash when there is no state.
fn state_span(state: Option<PaneState>) -> Span<'static> {
    match state {
        Some(state) => Span::styled(state.label(), Style::new().fg(state_color(state))),
        None => Span::raw(NONE_TEXT),
    }
}

/// The text of the marker in front of the selected row.
const HIGHLIGHT_SYMBOL: &str = "› ";

/// The text of the color block in the first column.
const STATE_BLOCK: &str = "██";

/// The text of the table when no session wrote a state file.
const EMPTY_HINT: &str = "No Claude Code session wrote a state file.";

/// The text that stands for a missing value.
const NONE_TEXT: &str = "—";

/// The color of a state, as a color of the terminal.
fn state_color(state: PaneState) -> Color {
    let rgb = state.color();
    Color::Rgb(rgb.red, rgb.green, rgb.blue)
}

/// Draw the table of sessions in `area`.
fn draw_table(frame: &mut Frame, area: Rect, app: &App, now: EpochMillis) {
    let board = app.board();
    let count = board.len();
    let noun = if count == 1 { "session" } else { "sessions" };
    let block = Block::bordered().title(format!(" cctop · {count} {noun} "));
    if count == 0 {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(Paragraph::new(EMPTY_HINT), inner);
        return;
    }
    let rows: Vec<Row> = board
        .rows()
        .into_iter()
        .map(|session| {
            let state = session.file.state;
            Row::new([
                Span::styled(STATE_BLOCK, Style::new().fg(state_color(state)))
                    .into(),
                Text::from(state.label()),
                Text::from(format_elapsed(now.saturating_since(session.since))),
                Text::from(
                    session
                        .file
                        .zellij_session
                        .clone()
                        .unwrap_or_else(|| NONE_TEXT.to_string()),
                ),
                Text::from(session.file.directory_name()),
                Text::from(session.name.clone().unwrap_or_default()),
            ])
        })
        .collect();
    let widths = [
        Constraint::Length(2),
        Constraint::Length(10),
        Constraint::Length(8),
        Constraint::Length(14),
        Constraint::Length(24),
        Constraint::Min(8),
    ];
    let header = Row::new(["", "STATE", "TIME", "ZELLIJ", "DIRECTORY", "NAME"])
        .style(Style::new().add_modifier(Modifier::BOLD));
    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol(HIGHLIGHT_SYMBOL)
        .highlight_spacing(HighlightSpacing::Always);
    let mut state = TableState::default().with_selected(app.selected_index());
    frame.render_stateful_widget(table, area, &mut state);
}

/// Format a time span for the screen. A span under one minute shows seconds,
/// under one hour shows minutes and seconds, under one day shows hours and
/// minutes, and a longer span shows days and hours.
#[must_use]
pub fn format_elapsed(ms: u64) -> String {
    let seconds = ms / 1000;
    let (minutes, hours, days) = (seconds / 60, seconds / 3600, seconds / 86_400);
    if seconds < 60 {
        format!("{seconds}s")
    } else if minutes < 60 {
        format!("{minutes}m {:02}s", seconds % 60)
    } else if hours < 24 {
        format!("{hours}h {:02}m", minutes % 60)
    } else {
        format!("{days}d {:02}h", hours % 24)
    }
}

/// Format a time of day as `HH:MM:SS` in the time zone `zone`.
#[must_use]
pub fn clock_time<Tz: TimeZone>(at: EpochMillis, zone: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let millis = i64::try_from(at.as_u64()).unwrap_or(i64::MAX);
    zone.timestamp_millis_opt(millis)
        .single()
        .map_or_else(String::new, |time| time.format("%H:%M:%S").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proctree::Pid;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Color;
    use ratatui::Terminal;

    use crate::state::StateFile;

    const SECOND: u64 = 1000;
    const MINUTE: u64 = 60 * SECOND;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;

    #[test]
    fn format_elapsed_uses_the_unit_that_fits() {
        assert_eq!(format_elapsed(0), "0s");
        assert_eq!(format_elapsed(59_999), "59s");
        assert_eq!(format_elapsed(MINUTE), "1m 00s");
        assert_eq!(format_elapsed(3_599_000), "59m 59s");
        assert_eq!(format_elapsed(HOUR), "1h 00m");
        assert_eq!(format_elapsed(86_399_000), "23h 59m");
        assert_eq!(format_elapsed(DAY), "1d 00h");
        assert_eq!(format_elapsed(2 * DAY + 4 * HOUR), "2d 04h");
    }

    #[test]
    fn clock_time_shows_the_time_of_day_in_the_zone() {
        let at = EpochMillis::new(1_794_824_265_058);
        let expected = chrono::DateTime::from_timestamp_millis(1_794_824_265_058)
            .expect("the instant is valid")
            .format("%H:%M:%S")
            .to_string();
        assert_eq!(clock_time(at, &chrono::Utc), expected);
        assert_eq!(expected.len(), 8);
    }

    const WIDTH: u16 = 100;
    const HEIGHT: u16 = 24;

    /// A state file in the directory `/path/<dir>`.
    fn file(pid: u32, dir: &str, state: PaneState, stamp: u64) -> StateFile {
        StateFile {
            pid: Pid::new(pid),
            session_id: format!("session-{pid}"),
            started_at: EpochMillis::new(1),
            state,
            stamp: EpochMillis::new(stamp),
            cwd: std::path::PathBuf::from(format!("/path/{dir}")),
            zellij_session: None,
            zellij_pane: None,
        }
    }

    /// Draw `app` on a terminal of the fixed test size.
    fn render(app: &App, now: u64) -> Buffer {
        let mut terminal =
            Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("a test terminal opens");
        terminal
            .draw(|frame| draw(frame, app, EpochMillis::new(now)))
            .expect("the test terminal draws");
        terminal.backend().buffer().clone()
    }

    /// The text of line `y` of the buffer.
    fn row_text(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol().to_string())
            .collect()
    }

    /// The text of all lines of the buffer.
    fn lines(buffer: &Buffer) -> Vec<String> {
        (0..buffer.area.height)
            .map(|y| row_text(buffer, y))
            .collect()
    }

    /// The line that holds `needle`, with its number.
    fn line_with(buffer: &Buffer, needle: &str) -> (u16, String) {
        lines(buffer)
            .into_iter()
            .enumerate()
            .find(|(_, line)| line.contains(needle))
            .map(|(y, line)| (u16::try_from(y).expect("the line number fits"), line))
            .unwrap_or_else(|| panic!("no line holds {needle:?}"))
    }

    fn two_session_app() -> App {
        let mut app = App::new();
        let mut working = file(10, "dir10", PaneState::Working, 1_000);
        working.zellij_session = None;
        app.board_mut().load(working, None);
        let mut needs_you = file(11, "dir11", PaneState::NeedsYou, 2_000);
        needs_you.zellij_session = Some("tools".to_string());
        app.board_mut().load(needs_you, Some("alpha".to_string()));
        app
    }

    #[test]
    fn the_table_shows_the_sessions_with_the_needs_you_row_first() {
        let app = two_session_app();
        let buffer = render(&app, 127_000);
        assert!(row_text(&buffer, 0).contains("cctop · 2 sessions"));
        let (needs_y, needs_line) = line_with(&buffer, "needs-you");
        let (working_y, working_line) = line_with(&buffer, "working");
        assert!(needs_y < working_y, "the needs-you row comes first");
        assert!(needs_line.contains("██"));
        assert!(needs_line.contains("2m 05s"));
        assert!(needs_line.contains("tools"));
        assert!(needs_line.contains("dir11"));
        assert!(needs_line.contains("alpha"));
        assert!(working_line.contains("2m 06s"));
        assert!(working_line.contains('—'));
        assert!(working_line.contains("dir10"));
    }

    #[test]
    fn the_table_has_a_header() {
        let buffer = render(&two_session_app(), 127_000);
        let header = row_text(&buffer, 1);
        for title in ["STATE", "TIME", "ZELLIJ", "DIRECTORY", "NAME"] {
            assert!(header.contains(title), "the header lacks {title}");
        }
    }

    #[test]
    fn the_state_block_has_the_color_of_the_state() {
        let buffer = render(&two_session_app(), 127_000);
        let (y, line) = line_with(&buffer, "needs-you");
        let x = line
            .chars()
            .position(|symbol| symbol == '█')
            .expect("the row holds a block");
        let x = u16::try_from(x).expect("the column fits");
        assert_eq!(buffer[(x, y)].fg, Color::Rgb(0xfa, 0xb0, 0x05));
    }

    #[test]
    fn only_the_selected_row_starts_with_the_marker() {
        let buffer = render(&two_session_app(), 127_000);
        let (_, needs_line) = line_with(&buffer, "needs-you");
        let (_, working_line) = line_with(&buffer, "working");
        assert!(needs_line.trim_start_matches('│').starts_with("› "));
        assert!(!working_line.trim_start_matches('│').starts_with("› "));
    }

    #[test]
    fn an_empty_board_shows_a_hint() {
        let buffer = render(&App::new(), 1_000);
        assert!(row_text(&buffer, 0).contains("cctop · 0 sessions"));
        let (_, line) = line_with(&buffer, "No Claude Code session wrote a state file.");
        assert!(!line.contains("STATE"));
    }

    #[test]
    fn the_log_shows_a_change_of_state() {
        let mut app = App::new();
        app.board_mut()
            .load(file(11, "dir11", PaneState::Idle, 100), None);
        app.board_mut()
            .apply(file(11, "dir11", PaneState::NeedsYou, 200), None);
        let buffer = render(&app, 1_000);
        assert!(lines(&buffer)
            .iter()
            .any(|line| line.contains("dir11  idle → needs-you")));
        assert!(lines(&buffer).iter().any(|line| line.contains("Changes")));
    }

    #[test]
    fn the_log_shows_a_new_session_with_a_dash() {
        let mut app = App::new();
        app.board_mut()
            .apply(file(12, "dir12", PaneState::Working, 300), Some("beta".to_string()));
        let buffer = render(&app, 1_000);
        assert!(lines(&buffer)
            .iter()
            .any(|line| line.contains("beta  — → working")));
    }

    #[test]
    fn the_log_shows_an_ended_session_with_a_dash() {
        let mut app = App::new();
        app.board_mut()
            .load(file(13, "dir13", PaneState::Idle, 100), None);
        app.board_mut().remove(Pid::new(13), EpochMillis::new(400));
        let buffer = render(&app, 1_000);
        assert!(lines(&buffer)
            .iter()
            .any(|line| line.contains("dir13  idle → —")));
    }

    #[test]
    fn the_log_shows_at_most_eight_lines_with_the_newest_first() {
        let mut app = App::new();
        for number in 0..10_u32 {
            let pid = 20 + number;
            app.board_mut().apply(
                file(pid, &format!("d{pid}"), PaneState::Working, u64::from(pid)),
                None,
            );
        }
        let buffer = render(&app, 1_000);
        let log_lines: Vec<String> = lines(&buffer)
            .into_iter()
            .filter(|line| line.contains("— → working"))
            .collect();
        assert_eq!(log_lines.len(), 8);
        assert!(log_lines[0].contains("d29  — → working"));
        assert!(log_lines[7].contains("d22  — → working"));
    }

    #[test]
    fn the_bottom_line_shows_the_keys_when_there_is_no_status() {
        let buffer = render(&two_session_app(), 127_000);
        assert_eq!(
            row_text(&buffer, HEIGHT - 1).trim_end(),
            "j/k move · enter focus · n next needs-you · q quit"
        );
    }

    #[test]
    fn the_bottom_line_shows_the_status_when_there_is_one() {
        let mut app = two_session_app();
        app.set_status("No session needs you");
        let buffer = render(&app, 127_000);
        assert_eq!(row_text(&buffer, HEIGHT - 1).trim_end(), "No session needs you");
    }
}
