//! Draws the board of `ha` and the text formats of its columns.

use std::fmt::Display;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::Frame;
use textfit::pad_right;

use crate::board::{Board, Row, Section};
use crate::run::{Run, RunState};
use crate::settings::format_duration_setting;
use crate::status::Status;

/// The width of the marker column.
const MARKER_WIDTH: usize = 1;
/// The width of the started column.
const STARTED_WIDTH: usize = 11;
/// The width of the time column.
const TIME_WIDTH: usize = 7;
/// The width of the finished column.
const FINISHED_WIDTH: usize = 8;
/// The lines that are not board rows: the column header and the status line.
const FIXED_LINES: usize = 2;
/// The text that stands between two parts of the status line.
const STATUS_SEPARATOR: &str = "  ·  ";
/// The key hints at the end of the status line.
const KEY_HINTS: &str = "↑↓ select  ⏎ open  r refresh  q quit";

/// The widths of the four text columns.
#[derive(Debug, Clone, Copy)]
struct Widths {
    repo: usize,
    workflow: usize,
    branch: usize,
    title: usize,
}

impl Widths {
    fn fixed() -> Widths {
        Widths {
            repo: 21,
            workflow: 17,
            branch: 17,
            title: 31,
        }
    }
}

/// Joins the cells of one row into one line. One space stands between cells.
fn cells(
    widths: Widths,
    marker: &str,
    texts: [&str; 4],
    started: &str,
    time: &str,
    finished: &str,
) -> String {
    let [repo, workflow, branch, title] = texts;
    [
        pad_right(marker, MARKER_WIDTH),
        pad_right(repo, widths.repo),
        pad_right(workflow, widths.workflow),
        pad_right(branch, widths.branch),
        pad_right(title, widths.title),
        pad_right(started, STARTED_WIDTH),
        pad_right(time, TIME_WIDTH),
        pad_right(finished, FINISHED_WIDTH),
    ]
    .join(" ")
}

/// The marker glyph of a run state.
fn marker(state: RunState) -> &'static str {
    match state {
        RunState::Running => "●",
        RunState::Queued => "○",
        RunState::Passed => "✓",
        RunState::Failed => "✗",
        RunState::Other => " ",
    }
}

/// The time between two instants. A negative span gives the absolute value.
fn span(from: DateTime<Utc>, to: DateTime<Utc>) -> Duration {
    Duration::from_secs((to - from).num_seconds().unsigned_abs())
}

/// The started column: the clock for today, the date and the clock before.
fn format_started(started: DateTime<Utc>, now: DateTime<Utc>) -> String {
    if started.date_naive() == now.date_naive() {
        started.format("%H:%M:%S").to_string()
    } else {
        started.format("%m-%d %H:%M").to_string()
    }
}

/// The line of one run.
fn run_line(run: &Run, now: DateTime<Utc>, widths: Widths) -> String {
    let (time, finished) = if run.state.is_active() {
        (format_elapsed(span(run.started_at, now)), String::new())
    } else {
        (
            format_elapsed(span(run.started_at, run.updated_at)),
            format_ago(span(run.updated_at, now)),
        )
    };
    cells(
        widths,
        marker(run.state),
        [
            &run.repo.to_string(),
            &run.workflow,
            &run.branch,
            &run.title,
        ],
        &format_started(run.started_at, now),
        &time,
        &finished,
    )
}

/// The line of a section header.
fn header_line(
    section: Section,
    count: usize,
    passed: usize,
    failed: usize,
    window: Duration,
) -> String {
    match section {
        Section::Active => format!("Running · {count}"),
        Section::Recent => format!(
            "Last {} · {passed} passed · {failed} failed",
            format_duration_setting(window)
        ),
    }
}

/// The line that stands in an empty section.
fn empty_line(section: Section, window: Duration) -> String {
    match section {
        Section::Active => "  No runs in progress or in the queue.".to_owned(),
        Section::Recent => format!(
            "  No runs finished in the last {}.",
            format_duration_setting(window)
        ),
    }
}

/// Writes one line of text at row `y` of the area and returns the buffer row.
fn put(buffer: &mut Buffer, y: usize, text: &str, style: Style) -> u16 {
    let area = buffer.area;
    let y = area.y + u16::try_from(y).unwrap_or(u16::MAX);
    buffer.set_string(area.x, y, text, style);
    y
}

/// The color of the marker of a run state.
fn marker_color(state: RunState) -> Color {
    match state {
        RunState::Running => Color::Yellow,
        RunState::Queued => Color::Blue,
        RunState::Passed => Color::Green,
        RunState::Failed => Color::Red,
        RunState::Other => Color::Reset,
    }
}

/// Draws one frame: the column header, the board rows, and the status line.
///
/// The board rows come from [`Board::view`] with the frame height minus the
/// two fixed lines. Times show in the time zone `tz`. This function never
/// panics, whatever the size of the frame.
pub fn draw<Tz: TimeZone>(
    frame: &mut Frame,
    board: &mut Board,
    status: &Status,
    now: DateTime<Utc>,
    tz: &Tz,
) where
    Tz::Offset: Display,
{
    let _ = (status, tz);
    let area = frame.area();
    let widths = Widths::fixed();
    let height = usize::from(area.height) - FIXED_LINES;
    let window = board.window();
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let buffer = frame.buffer_mut();
    put(
        buffer,
        0,
        &cells(
            widths,
            " ",
            ["REPO", "WORKFLOW", "BRANCH", "TITLE"],
            "STARTED",
            "TIME",
            "FINISHED",
        ),
        bold,
    );
    let view = board.view(now, height);
    for (index, row) in view.rows.iter().enumerate() {
        match row {
            Row::Header {
                section,
                count,
                passed,
                failed,
            } => {
                let line = header_line(*section, *count, *passed, *failed, window);
                put(buffer, index + 1, &line, bold);
            }
            Row::Empty(section) => {
                let line = empty_line(*section, window);
                put(buffer, index + 1, &line, Style::default());
            }
            Row::Run { run, selected } => {
                let line = run_line(run, now, widths);
                let y = put(buffer, index + 1, &line, Style::default());
                buffer.set_style(
                    Rect::new(area.x, y, 1, 1),
                    Style::default().fg(marker_color(run.state)),
                );
                if *selected {
                    buffer.set_style(
                        Rect::new(area.x, y, area.width, 1),
                        Style::default().add_modifier(Modifier::REVERSED),
                    );
                }
            }
        }
    }
    put(
        buffer,
        usize::from(area.height) - 1,
        &format!("loading…{STATUS_SEPARATOR}{KEY_HINTS}"),
        Style::default(),
    );
}

/// Writes how long a run took, in at most 7 columns.
///
/// Examples are `45s`, `12m 03s`, `2h 14m`, and `3d 04h`. A duration of 100
/// days or more gives `>99d`.
#[must_use]
pub fn format_elapsed(d: Duration) -> String {
    let total = d.as_secs();
    let (minute, hour, day) = (60, 3_600, 86_400);
    if total < minute {
        format!("{total}s")
    } else if total < hour {
        format!("{}m {:02}s", total / minute, total % minute)
    } else if total < day {
        format!("{}h {:02}m", total / hour, total % hour / minute)
    } else if total < 100 * day {
        format!("{}d {:02}h", total / day, total % day / hour)
    } else {
        ">99d".to_owned()
    }
}

/// Writes how long ago a run finished, with one unit.
///
/// Examples are `45s ago`, `12m ago`, `2h ago`, and `3d ago`. A duration of
/// 100 days or more gives `>99d ago`.
#[must_use]
pub fn format_ago(d: Duration) -> String {
    let total = d.as_secs();
    let (minute, hour, day) = (60, 3_600, 86_400);
    if total < minute {
        format!("{total}s ago")
    } else if total < hour {
        format!("{}m ago", total / minute)
    } else if total < day {
        format!("{}h ago", total / hour)
    } else if total < 100 * day {
        format!("{}d ago", total / day)
    } else {
        ">99d ago".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::{Color, Modifier};
    use ratatui::Terminal;

    use crate::run::{Run, RunId, RunState};
    use crate::target::RepoName;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("the test time is valid")
            .with_timezone(&Utc)
    }

    fn now() -> DateTime<Utc> {
        at("2026-10-14T12:00:00Z")
    }

    fn utc() -> chrono::FixedOffset {
        chrono::FixedOffset::east_opt(0).expect("zero is a valid offset")
    }

    fn repo(text: &str) -> RepoName {
        RepoName::parse(text).expect("the test repository is valid")
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "a test fixture states every field"
    )]
    fn run(
        id: u64,
        repo_text: &str,
        workflow: &str,
        branch: &str,
        title: &str,
        state: RunState,
        started: &str,
        updated: &str,
    ) -> Run {
        Run {
            id: RunId(id),
            repo: repo(repo_text),
            workflow: workflow.to_owned(),
            branch: branch.to_owned(),
            title: title.to_owned(),
            state,
            started_at: at(started),
            updated_at: at(updated),
            url: format!("https://github.com/{repo_text}/actions/runs/{id}"),
        }
    }

    /// A board with one run in each of the four states.
    fn four_run_board() -> Board {
        let mut board = Board::new(Duration::from_hours(8));
        board.replace(
            repo("timmattison/tools"),
            vec![
                run(
                    1,
                    "timmattison/tools",
                    "CI",
                    "main",
                    "Fix the thing",
                    RunState::Running,
                    "2026-10-14T11:45:30Z",
                    "2026-10-14T11:59:00Z",
                ),
                run(
                    3,
                    "timmattison/tools",
                    "CI",
                    "main",
                    "Bump deps",
                    RunState::Passed,
                    "2026-10-14T09:00:00Z",
                    "2026-10-14T09:10:05Z",
                ),
            ],
        );
        board.replace(
            repo("acme/widgets"),
            vec![
                run(
                    2,
                    "acme/widgets",
                    "Release",
                    "issue-12",
                    "Add release job",
                    RunState::Queued,
                    "2026-10-14T11:58:00Z",
                    "2026-10-14T11:58:00Z",
                ),
                run(
                    4,
                    "acme/widgets",
                    "Release",
                    "issue-11",
                    "Broken build",
                    RunState::Failed,
                    "2026-10-13T23:50:00Z",
                    "2026-10-14T04:30:00Z",
                ),
            ],
        );
        board
    }

    /// Draws one frame and returns the buffer.
    fn render_with(width: u16, height: u16, board: &mut Board, status: &Status) -> Buffer {
        let mut terminal =
            Terminal::new(TestBackend::new(width, height)).expect("a test terminal opens");
        terminal
            .draw(|frame| draw(frame, board, status, now(), &utc()))
            .expect("a test frame draws");
        terminal.backend().buffer().clone()
    }

    /// Reads each line of a buffer as text. A wide glyph fills one cell and
    /// the cell after it holds a blank that is not part of the text.
    fn lines(buffer: &Buffer) -> Vec<String> {
        let area = buffer.area;
        (0..area.height)
            .map(|y| {
                let mut line = String::new();
                let mut skip = 0_usize;
                for x in 0..area.width {
                    if skip > 0 {
                        skip -= 1;
                        continue;
                    }
                    let symbol = buffer[(x, y)].symbol();
                    skip = unicode_width::UnicodeWidthStr::width(symbol).saturating_sub(1);
                    line.push_str(symbol);
                }
                line.trim_end().to_owned()
            })
            .collect()
    }

    #[test]
    fn the_frame_holds_the_header_the_rows_and_the_status_line() {
        let mut board = four_run_board();
        let buffer = render_with(120, 10, &mut board, &Status::default());
        assert_eq!(
            lines(&buffer),
            [
                "  REPO                  WORKFLOW          BRANCH            TITLE                           STARTED     TIME    FINISHED",
                "Running · 2",
                "● timmattison/tools     CI                main              Fix the thing                   11:45:30    14m 30s",
                "○ acme/widgets          Release           issue-12          Add release job                 11:58:00    2m 00s",
                "Last 8h · 1 passed · 1 failed",
                "✓ timmattison/tools     CI                main              Bump deps                       09:00:00    10m 05s 2h ago",
                "✗ acme/widgets          Release           issue-11          Broken build                    10-13 23:50 4h 40m  7h ago",
                "",
                "",
                "loading…  ·  ↑↓ select  ⏎ open  r refresh  q quit",
            ]
        );
    }

    #[test]
    fn format_elapsed_writes_two_units_and_a_cap() {
        let cases: [(u64, &str); 13] = [
            (0, "0s"),
            (45, "45s"),
            (59, "59s"),
            (60, "1m 00s"),
            (723, "12m 03s"),
            (3_599, "59m 59s"),
            (3_600, "1h 00m"),
            (8_040, "2h 14m"),
            (86_399, "23h 59m"),
            (86_400, "1d 00h"),
            (3 * 86_400 + 4 * 3_600, "3d 04h"),
            (99 * 86_400 + 23 * 3_600 + 3_599, "99d 23h"),
            (100 * 86_400, ">99d"),
        ];
        for (seconds, expected) in cases {
            assert_eq!(format_elapsed(Duration::from_secs(seconds)), expected);
        }
    }

    #[test]
    fn format_elapsed_is_never_wider_than_seven_columns() {
        let mut seconds: u64 = 0;
        while seconds < 200 * 86_400 {
            let text = format_elapsed(Duration::from_secs(seconds));
            assert!(text.chars().count() <= 7, "{seconds}s gave {text:?}");
            seconds += 997;
        }
        let text = format_elapsed(Duration::from_secs(u64::MAX));
        assert_eq!(text, ">99d");
    }

    #[test]
    fn format_ago_writes_one_unit_and_a_cap() {
        let cases: [(u64, &str); 8] = [
            (0, "0s ago"),
            (45, "45s ago"),
            (12 * 60 + 30, "12m ago"),
            (2 * 3_600 + 59, "2h ago"),
            (3 * 86_400 + 5, "3d ago"),
            (99 * 86_400 + 86_399, "99d ago"),
            (100 * 86_400, ">99d ago"),
            (u64::MAX, ">99d ago"),
        ];
        for (seconds, expected) in cases {
            assert_eq!(format_ago(Duration::from_secs(seconds)), expected);
        }
    }

    #[test]
    fn each_marker_has_the_color_of_its_state() {
        let mut board = four_run_board();
        let buffer = render_with(120, 10, &mut board, &Status::default());
        let marks: Vec<(&str, Color)> = [2_u16, 3, 5, 6]
            .iter()
            .map(|y| (buffer[(0, *y)].symbol(), buffer[(0, *y)].fg))
            .collect();
        assert_eq!(
            marks,
            [
                ("●", Color::Yellow),
                ("○", Color::Blue),
                ("✓", Color::Green),
                ("✗", Color::Red),
            ]
        );
    }

    #[test]
    fn only_the_selected_row_is_reversed() {
        let mut board = four_run_board();
        let buffer = render_with(120, 10, &mut board, &Status::default());
        let reversed = |x: u16, y: u16| buffer[(x, y)].modifier.contains(Modifier::REVERSED);
        assert!(reversed(0, 2), "the marker of the selected row");
        assert!(reversed(60, 2), "the middle of the selected row");
        assert!(reversed(119, 2), "the end of the selected row");
        for y in [0_u16, 1, 3, 4, 5, 6, 9] {
            assert!(!reversed(0, y), "line {y} start");
            assert!(!reversed(60, y), "line {y} middle");
        }
    }

    #[test]
    fn an_empty_board_says_so_in_each_section() {
        let mut board = Board::new(Duration::from_hours(8));
        let buffer = render_with(60, 8, &mut board, &Status::default());
        let text = lines(&buffer);
        assert_eq!(
            text[1..5],
            [
                "Running · 0",
                "  No runs in progress or in the queue.",
                "Last 8h · 0 passed · 0 failed",
                "  No runs finished in the last 8h.",
            ]
        );
    }

    #[test]
    fn the_window_text_follows_the_setting() {
        let mut board = Board::new(Duration::from_mins(90));
        let buffer = render_with(60, 8, &mut board, &Status::default());
        let text = lines(&buffer);
        assert_eq!(text[3], "Last 90m · 0 passed · 0 failed");
        assert_eq!(text[4], "  No runs finished in the last 90m.");
    }

    #[test]
    fn the_column_header_and_the_section_headers_are_bold() {
        let mut board = four_run_board();
        let buffer = render_with(120, 10, &mut board, &Status::default());
        for y in [0_u16, 1, 4] {
            assert!(buffer[(0, y)].modifier.contains(Modifier::BOLD), "line {y}");
        }
        assert!(!buffer[(2, 2)].modifier.contains(Modifier::BOLD));
    }
}
