//! The screen of cctop: the table of sessions, the change log, and the
//! status line.

use chrono::TimeZone;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph, Row, Table, TableState};
use ratatui::Frame;

use crate::app::{App, PageSize};
use crate::board::{
    Board, Change, Session, ShownState, FLAG_SEPARATOR, FROZEN_LABEL, HANDLED_SUFFIX,
};
use crate::state::{EpochMillis, PaneState, Rgb, FROZEN_COLOR, FROZEN_MARKER};

/// Draw the screen: the table of sessions, the change log, and the status
/// line. `now` gives the time that the elapsed times count to.
pub fn draw(frame: &mut Frame, app: &App, now: EpochMillis) {
    let [table_area, log_area, status_area] = screen_areas(frame.area());
    draw_table(frame, table_area, app, now);
    draw_log(frame, log_area, app);
    draw_status(frame, status_area, app);
}

/// The height of the status line.
const STATUS_HEIGHT: u16 = 1;

/// The height of the header row of the table.
const HEADER_HEIGHT: u16 = 1;

/// The areas of the table, the log, and the status line in `area`.
fn screen_areas(area: Rect) -> [Rect; 3] {
    Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(LOG_HEIGHT),
        Constraint::Length(STATUS_HEIGHT),
    ])
    .areas(area)
}

/// The block that borders the table.
fn table_block() -> Block<'static> {
    Block::bordered()
}

/// The page size for a terminal `terminal_height` rows high: the number of
/// session rows that [`draw`] shows in the table at that height, and at
/// least 1. The header row and the borders of the table do not count. The
/// function reads the layout of [`draw`], so the two cannot disagree.
#[must_use]
pub fn page_size(terminal_height: u16) -> PageSize {
    // The width does not change a vertical layout.
    let [table_area, _, _] = screen_areas(Rect::new(0, 0, 1, terminal_height));
    let rows = table_block()
        .inner(table_area)
        .height
        .saturating_sub(HEADER_HEIGHT);
    PageSize::new(usize::from(rows))
}

/// The text of the bottom line when there is no status.
const KEY_HELP: &str = "j/k move · enter focus · n next needs-you · h handled · q quit";

/// Draw the status text, or the help for the keys, in `area`.
fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    let text = app.status().unwrap_or(KEY_HELP);
    frame.render_widget(Paragraph::new(text), area);
}

/// The number of changes that the log shows.
const LOG_LINES: u16 = 8;

/// The height of the log block: its lines and its two borders.
const LOG_HEIGHT: u16 = LOG_LINES + 2;

/// Draw the newest changes in `area`, the newest first.
fn draw_log(frame: &mut Frame, area: Rect, app: &App) {
    let lines: Vec<Line> = app
        .board()
        .changes()
        .take(usize::from(LOG_LINES))
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

/// The label of a shown state in the color of its state, or a dash when there is no state.
fn state_span(shown: Option<ShownState>) -> Span<'static> {
    match shown {
        Some(shown) => Span::styled(shown.label(), Style::new().fg(state_color(shown.state()))),
        None => Span::raw(NONE_TEXT),
    }
}

/// The text of the color block in the first column.
const STATE_BLOCK: &str = "██";

/// The text of the table when no session wrote a state file.
const EMPTY_HINT: &str = "No Claude Code session wrote a state file.";

/// The text that stands for a missing value.
const NONE_TEXT: &str = "—";

/// The narrowest width of the state column. It fits each state that has no
/// handled mark.
const MIN_STATE_WIDTH: u16 = 10;

/// The width of the state column: the longest state text of `labels`, and at
/// least [`MIN_STATE_WIDTH`].
fn state_width<'a>(labels: impl Iterator<Item = &'a str>) -> u16 {
    labels
        .map(|label| u16::try_from(label.chars().count()).unwrap_or(u16::MAX))
        .fold(MIN_STATE_WIDTH, u16::max)
}

/// A color of the palette, as a color of the terminal.
fn rgb_color(rgb: Rgb) -> Color {
    Color::Rgb(rgb.red, rgb.green, rgb.blue)
}

/// The color of a state, as a color of the terminal.
fn state_color(state: PaneState) -> Color {
    rgb_color(state.color())
}

/// The two spans of the frozen flag: the separator with no color, then the
/// word `frozen` in blue.
fn frozen_flag() -> [Span<'static>; 2] {
    [
        Span::raw(FLAG_SEPARATOR),
        Span::styled(FROZEN_LABEL, Style::new().fg(rgb_color(FROZEN_COLOR))),
    ]
}

/// The state cell of a row. It has the same text and the same order as
/// [`Session::state_label`]. The state has no color, because the block has it.
/// The word `frozen` is blue.
fn state_line(session: &Session) -> Line<'static> {
    let mut spans = vec![Span::raw(session.state().label())];
    if session.is_handled() {
        spans.push(Span::raw(HANDLED_SUFFIX));
    }
    if session.is_frozen() {
        spans.extend(frozen_flag());
    }
    Line::from(spans)
}

/// The text between two parts of the title of the table.
const TITLE_SEPARATOR: &str = " · ";

/// The title of the table: the number of sessions, then the count of each
/// state in the sort order of the rows, then the count of frozen rows. A count
/// of a state reads `unmarked/handled` and the marker of its state follows it.
/// It holds only live rows. The last count has one number, the number of
/// frozen rows, because a frozen row takes no handled mark.
fn table_title(board: &Board) -> String {
    let count = board.len();
    let noun = if count == 1 { "session" } else { "sessions" };
    let counts: Vec<String> = board
        .state_counts()
        .iter()
        .map(|state| {
            format!(
                "{}/{} {}",
                state.unmarked,
                state.handled,
                state.state.marker()
            )
        })
        .collect();
    format!(
        " cctop{TITLE_SEPARATOR}{count} {noun}{TITLE_SEPARATOR}{}{TITLE_SEPARATOR}{} {FROZEN_MARKER} ",
        counts.join(TITLE_SEPARATOR),
        board.frozen_count()
    )
}

/// Draw the table of sessions in `area`.
fn draw_table(frame: &mut Frame, area: Rect, app: &App, now: EpochMillis) {
    let board = app.board();
    let block = table_block().title(table_title(board));
    if board.is_empty() {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(Paragraph::new(EMPTY_HINT), inner);
        return;
    }
    let sessions: Vec<_> = board
        .rows()
        .into_iter()
        .map(|session| (session, session.state_label()))
        .collect();
    let state_width = state_width(sessions.iter().map(|(_, label)| label.as_str()));
    let rows: Vec<Row> = sessions
        .into_iter()
        .map(|(session, _)| {
            let state = session.state();
            Row::new([
                Span::styled(STATE_BLOCK, Style::new().fg(state_color(state))).into(),
                Text::from(state_line(session)),
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
    // The name column keeps at least 8 columns, and it takes the spare
    // width. The directory column keeps at most 24 columns, and it gives up
    // width first when the terminal is narrow. The state column keeps its
    // width until the other columns have no more width to give.
    let widths = [
        Constraint::Length(2),
        Constraint::Length(state_width),
        Constraint::Length(8),
        Constraint::Length(14),
        Constraint::Max(24),
        Constraint::Min(8),
    ];
    let header = Row::new(["", "STATE", "TIME", "ZELLIJ", "DIRECTORY", "NAME"])
        .height(HEADER_HEIGHT)
        .style(Style::new().add_modifier(Modifier::BOLD));
    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED));
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

    use crate::liveness::RunStatus;
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
        render_at(app, now, WIDTH)
    }

    /// Draw `app` on a terminal that is `width` columns wide.
    fn render_at(app: &App, now: u64, width: u16) -> Buffer {
        render_sized(app, now, width, HEIGHT)
    }

    /// Draw `app` on a terminal that is `width` columns wide and `height`
    /// rows high.
    fn render_sized(app: &App, now: u64, width: u16, height: u16) -> Buffer {
        let mut terminal =
            Terminal::new(TestBackend::new(width, height)).expect("a test terminal opens");
        terminal
            .draw(|frame| draw(frame, app, EpochMillis::new(now)))
            .expect("the test terminal draws");
        terminal.backend().buffer().clone()
    }

    /// The text of line `y` of the buffer.
    ///
    /// Ratatui writes a wide glyph into one cell and resets the next cell to
    /// a space. The function skips the cells that a wide glyph hides.
    fn row_text(buffer: &Buffer, y: u16) -> String {
        let mut text = String::new();
        let mut skip = 0;
        for x in 0..buffer.area.width {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let symbol = buffer[(x, y)].symbol();
            skip = unicode_width::UnicodeWidthStr::width(symbol).saturating_sub(1);
            text.push_str(symbol);
        }
        text
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
            .map_or_else(
                || panic!("no line holds {needle:?}"),
                |(y, line)| (u16::try_from(y).expect("the line number fits"), line),
            )
    }

    /// The column of the first match of `needle` in `line`. It counts
    /// characters and not bytes, because the border is wider than one byte.
    fn column_of(line: &str, needle: &str) -> u16 {
        let chars: Vec<char> = line.chars().collect();
        let wanted: Vec<char> = needle.chars().collect();
        let index = chars
            .windows(wanted.len())
            .position(|window| window == wanted.as_slice())
            .unwrap_or_else(|| panic!("{needle:?} is not in {line:?}"));
        u16::try_from(index).expect("the column fits")
    }

    #[test]
    fn the_page_size_is_the_number_of_session_rows_that_draw_shows() {
        let mut app = App::new();
        for pid in 1..=100_u32 {
            app.board_mut()
                .load(file(pid, &format!("d{pid}"), PaneState::Idle, 10), None);
        }
        for height in 1..=60_u16 {
            let buffer = render_sized(&app, 1_000, WIDTH, height);
            let counted = lines(&buffer)
                .iter()
                .filter(|line| line.contains(STATE_BLOCK))
                .count();
            assert_eq!(
                page_size(height).get(),
                counted.max(1),
                "at height {height} the table shows {counted} rows"
            );
        }
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
    fn only_the_highlight_shows_the_selected_row() {
        let buffer = render(&two_session_app(), 127_000);
        let (needs_y, needs_line) = line_with(&buffer, "needs-you");
        let (working_y, working_line) = line_with(&buffer, "working");
        assert!(
            needs_line.trim_start_matches('│').starts_with("██"),
            "the selected row starts with its block: {needs_line}"
        );
        assert!(
            working_line.trim_start_matches('│').starts_with("██"),
            "the other row starts with its block: {working_line}"
        );
        let first_column = 1;
        assert!(buffer[(first_column, needs_y)]
            .modifier
            .contains(Modifier::REVERSED));
        assert!(!buffer[(first_column, working_y)]
            .modifier
            .contains(Modifier::REVERSED));
    }

    #[test]
    fn a_handled_row_shows_the_whole_state_with_the_suffix_and_sorts_last() {
        let mut app = App::new();
        app.board_mut()
            .load(file(10, "dir10", PaneState::Idle, 1_000), None);
        app.board_mut()
            .load(file(11, "dir11", PaneState::Idle, 2_000), None);
        app.board_mut()
            .load(file(12, "dir12", PaneState::NeedsYou, 3_000), None);
        app.board_mut().toggle_handled(Pid::new(10));
        app.board_mut().toggle_handled(Pid::new(12));
        let buffer = render(&app, 10_000);
        let (handled_y, handled_line) = line_with(&buffer, "dir10");
        let (plain_y, plain_line) = line_with(&buffer, "dir11");
        let (_, needs_line) = line_with(&buffer, "dir12");
        assert!(handled_line.contains("idle - handled"), "{handled_line}");
        assert!(!plain_line.contains("handled"), "{plain_line}");
        assert!(needs_line.contains("needs-you - handled"), "{needs_line}");
        assert!(plain_y < handled_y, "the handled idle row comes last");
    }

    const LONG_DIRECTORY: &str = "dir-with-a-long-name-for-the-cut";
    const LONG_NAME: &str = "a-registry-name-25-chars";

    /// A board with a handled needs-you row and an idle row that has a long
    /// directory name and a long registry name.
    fn board_with_a_handled_needs_you_row() -> App {
        let mut app = App::new();
        app.board_mut()
            .load(file(10, "dir10", PaneState::NeedsYou, 3_000), None);
        app.board_mut().load(
            file(11, LONG_DIRECTORY, PaneState::Idle, 2_000),
            Some(LONG_NAME.to_string()),
        );
        app.board_mut().toggle_handled(Pid::new(10));
        app
    }

    #[test]
    fn a_handled_needs_you_row_shows_the_whole_state_at_80_columns() {
        let app = board_with_a_handled_needs_you_row();
        let buffer = render_at(&app, 10_000, 80);
        let (_, line) = line_with(&buffer, "dir10");
        assert!(line.contains("needs-you - handled"), "{line}");
    }

    #[test]
    fn a_handled_needs_you_row_shows_the_whole_state_at_70_columns() {
        let app = board_with_a_handled_needs_you_row();
        let buffer = render_at(&app, 10_000, 70);
        let (_, line) = line_with(&buffer, "dir10");
        assert!(line.contains("needs-you - handled"), "{line}");
    }

    #[test]
    fn the_name_column_shows_at_70_columns() {
        let app = board_with_a_handled_needs_you_row();
        let buffer = render_at(&app, 10_000, 70);
        let (_, line) = line_with(&buffer, "dir-with");
        let name_start: String = LONG_NAME.chars().take(8).collect();
        assert!(line.contains(&name_start), "{line}");
    }

    #[test]
    fn the_state_column_takes_no_spare_width_at_100_columns() {
        let app = board_with_a_handled_needs_you_row();
        let buffer = render(&app, 10_000);
        let header = row_text(&buffer, 1);
        let symbols: Vec<char> = header.chars().collect();
        let time_x = symbols
            .windows(4)
            .position(|window| window.iter().collect::<String>() == "TIME")
            .expect("the header names TIME");
        // 1 border, 2 for the block, 1 gap, 19 for the label, 1 gap.
        assert_eq!(time_x, 5 + 19, "{header}");
        let (_, line) = line_with(&buffer, "dir-with-a-long");
        assert!(line.contains(LONG_NAME), "{line}");
    }

    /// The top line of the table, which holds the title.
    fn title(buffer: &Buffer) -> String {
        row_text(buffer, 0)
    }

    #[test]
    fn the_title_shows_the_count_of_each_state_in_the_sort_order() {
        let mut app = App::new();
        app.board_mut()
            .load(file(1, "a", PaneState::NeedsYou, 100), None);
        app.board_mut()
            .load(file(2, "b", PaneState::NeedsYou, 100), None);
        app.board_mut()
            .load(file(3, "c", PaneState::Idle, 100), None);
        app.board_mut()
            .load(file(4, "d", PaneState::Agents, 100), None);
        app.board_mut()
            .load(file(5, "e", PaneState::Working, 100), None);
        app.board_mut().toggle_handled(Pid::new(2));
        app.board_mut().toggle_handled(Pid::new(3));
        let text = title(&render(&app, 127_000));
        assert!(
            text.contains(
                " cctop · 5 sessions · 1/1 ❗ · 0/1 💤 · 1/0 🤖 · 0/0 🐚 · 1/0 🏃 · 0 🧊 "
            ),
            "{text}"
        );
    }

    #[test]
    fn the_title_counts_only_live_rows_and_ends_with_the_frozen_count() {
        let mut app = App::new();
        app.board_mut()
            .load(file(1, "a", PaneState::NeedsYou, 100), None);
        app.board_mut()
            .load(file(2, "b", PaneState::NeedsYou, 100), None);
        app.board_mut().toggle_handled(Pid::new(2));
        app.board_mut().load_stopped(
            file(3, "c", PaneState::NeedsYou, 100),
            None,
            EpochMillis::new(100),
        );
        app.board_mut().load_stopped(
            file(4, "d", PaneState::Idle, 100),
            None,
            EpochMillis::new(100),
        );
        app.board_mut()
            .load(file(5, "e", PaneState::Working, 100), None);
        let text = title(&render(&app, 127_000));
        assert!(
            text.contains(
                " cctop · 5 sessions · 1/1 ❗ · 0/0 💤 · 0/0 🤖 · 0/0 🐚 · 1/0 🏃 · 2 🧊 "
            ),
            "{text}"
        );
    }

    #[test]
    fn the_empty_board_shows_five_counts_of_zero_and_no_frozen_row() {
        let text = title(&render(&App::new(), 1_000));
        assert!(
            text.contains(
                " cctop · 0 sessions · 0/0 ❗ · 0/0 💤 · 0/0 🤖 · 0/0 🐚 · 0/0 🏃 · 0 🧊 "
            ),
            "{text}"
        );
    }

    #[test]
    fn the_title_moves_a_session_from_unmarked_to_handled() {
        let mut app = App::new();
        app.board_mut()
            .load(file(7, "a", PaneState::Idle, 100), None);
        let before = title(&render(&app, 1_000));
        assert!(before.contains("1/0 💤"), "{before}");
        app.board_mut().toggle_handled(Pid::new(7));
        let after = title(&render(&app, 1_000));
        assert!(after.contains("0/1 💤"), "{after}");
    }

    #[test]
    fn the_whole_title_fits_in_79_columns() {
        let mut app = App::new();
        let mut pid = 1;
        let mut add = |app: &mut App, state: PaneState, handled: bool| {
            app.board_mut().load(file(pid, "d", state, 100), None);
            if handled {
                app.board_mut().toggle_handled(Pid::new(pid));
            }
            pid += 1;
        };
        add(&mut app, PaneState::NeedsYou, false);
        add(&mut app, PaneState::NeedsYou, true);
        add(&mut app, PaneState::NeedsYou, true);
        for _ in 0..3 {
            add(&mut app, PaneState::Idle, false);
        }
        for _ in 0..7 {
            add(&mut app, PaneState::Idle, true);
        }
        add(&mut app, PaneState::Agents, false);
        let buffer = render_at(&app, 1_000, 79);
        let text = title(&buffer);
        assert!(
            text.contains(
                " cctop · 14 sessions · 1/2 ❗ · 3/7 💤 · 1/0 🤖 · 0/0 🐚 · 0/0 🏃 · 0 🧊 "
            ),
            "{text}"
        );
        assert_eq!(buffer[(78, 0)].symbol(), "┐");
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
        app.board_mut().apply(
            file(11, "dir11", PaneState::NeedsYou, 200),
            None,
            RunStatus::Running,
            EpochMillis::new(0),
        );
        let buffer = render(&app, 1_000);
        assert!(lines(&buffer)
            .iter()
            .any(|line| line.contains("dir11  idle → needs-you")));
        assert!(lines(&buffer).iter().any(|line| line.contains("Changes")));
    }

    #[test]
    fn the_log_shows_a_new_session_with_a_dash() {
        let mut app = App::new();
        app.board_mut().apply(
            file(12, "dir12", PaneState::Working, 300),
            Some("beta".to_string()),
            RunStatus::Running,
            EpochMillis::new(0),
        );
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
                RunStatus::Running,
                EpochMillis::new(0),
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
            "j/k move · enter focus · n next needs-you · h handled · q quit"
        );
    }

    #[test]
    fn the_bottom_line_shows_the_status_when_there_is_one() {
        let mut app = two_session_app();
        app.set_status("No session needs you");
        let buffer = render(&app, 127_000);
        assert_eq!(
            row_text(&buffer, HEIGHT - 1).trim_end(),
            "No session needs you"
        );
    }

    /// The color of the word `frozen`, as a color of the terminal.
    const FROZEN_BLUE: Color = Color::Rgb(0x4d, 0xab, 0xf7);

    #[test]
    fn a_frozen_row_keeps_the_color_of_its_state_and_shows_frozen_in_blue() {
        let mut app = App::new();
        app.board_mut().load_stopped(
            file(10, "dir10", PaneState::Idle, 1_000),
            None,
            EpochMillis::new(1_000),
        );
        let buffer = render(&app, 127_000);
        let (y, line) = line_with(&buffer, "dir10");
        assert!(line.contains("idle - frozen"), "{line}");
        let block = column_of(&line, "█");
        assert_eq!(buffer[(block, y)].fg, Color::Rgb(0xe0, 0x31, 0x31));
        let frozen = column_of(&line, "frozen");
        for x in frozen..frozen + 6 {
            assert_eq!(buffer[(x, y)].fg, FROZEN_BLUE, "column {x} of {line}");
        }
        let idle = column_of(&line, "idle");
        for x in idle..idle + 4 {
            assert_ne!(buffer[(x, y)].fg, FROZEN_BLUE, "column {x} of {line}");
        }
    }
}
