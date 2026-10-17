//! Draws the board of `ha` and the text formats of its columns.

use std::fmt::Display;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::Frame;
use textfit::{pad_right, truncate_to_budget};

use crate::board::{Board, Row, Section};
use crate::run::{Run, RunState};
use crate::settings::format_duration_setting;
use crate::status::{Status, MESSAGE_LIFETIME};

/// The width of the marker column.
const MARKER_WIDTH: usize = 1;
/// The width of the started column.
const STARTED_WIDTH: usize = 11;
/// The width of the time column.
const TIME_WIDTH: usize = 7;
/// The width of the finished column.
const FINISHED_WIDTH: usize = 8;
/// The columns that the text columns do not share: the marker, the started,
/// time, and finished columns, and the seven spaces between the columns.
const FIXED_COLUMNS: usize = MARKER_WIDTH + STARTED_WIDTH + TIME_WIDTH + FINISHED_WIDTH + 7;
/// The lines that are not board rows: the column header and the status line.
const FIXED_LINES: usize = 2;
/// The text that stands between two parts of the status line.
const STATUS_SEPARATOR: &str = "  ·  ";
/// The key hints at the end of the left part of the status line.
const KEY_HINTS: &str = "↑↓ select  ⏎ open  r refresh  q quit";
/// The mark at the right end of the status line while a refresh cycle runs.
const REFRESH_MARK: &str = "refreshing…";
/// The least number of columns between the left part of the status line and
/// the refresh mark.
const MARK_GAP: usize = 2;

/// The widths of the four text columns.
#[derive(Debug, Clone, Copy)]
struct Widths {
    repo: usize,
    workflow: usize,
    branch: usize,
    title: usize,
}

impl Widths {
    /// Shares the columns that the fixed columns leave over: 25 percent to the
    /// repository, 20 percent to the workflow, 20 percent to the branch, and
    /// the rest to the title. A frame that is too narrow gives each 0.
    fn for_width(width: usize) -> Widths {
        let room = width.saturating_sub(FIXED_COLUMNS);
        let repo = room * 25 / 100;
        let workflow = room * 20 / 100;
        let branch = room * 20 / 100;
        Widths {
            repo,
            workflow,
            branch,
            title: room - repo - workflow - branch,
        }
    }
}

/// Cuts a text to a column width, then pads it to that width.
fn fit(text: &str, width: usize) -> String {
    pad_right(&truncate_to_budget(text, width), width)
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
        fit(repo, widths.repo),
        fit(workflow, widths.workflow),
        fit(branch, widths.branch),
        fit(title, widths.title),
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

/// The time from one instant to a later one. A clock that runs behind makes
/// a negative span, and that gives zero.
fn span(from: DateTime<Utc>, to: DateTime<Utc>) -> Duration {
    Duration::from_secs(u64::try_from((to - from).num_seconds()).unwrap_or(0))
}

/// The started column: the clock for today, the date and the clock before.
///
/// A day starts and ends at midnight in the time zone `tz`.
fn format_started<Tz: TimeZone>(started: DateTime<Utc>, now: DateTime<Utc>, tz: &Tz) -> String
where
    Tz::Offset: Display,
{
    let local = started.with_timezone(tz);
    if local.date_naive() == now.with_timezone(tz).date_naive() {
        local.format("%H:%M:%S").to_string()
    } else {
        local.format("%m-%d %H:%M").to_string()
    }
}

/// The line of one run.
fn run_line<Tz: TimeZone>(run: &Run, now: DateTime<Utc>, tz: &Tz, widths: Widths) -> String
where
    Tz::Offset: Display,
{
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
        &format_started(run.started_at, now, tz),
        &time,
        &finished,
    )
}

/// The line that stands in an empty section before the first refresh cycle
/// ends.
const SECTION_LOADING: &str = "  Loading…";

/// The line of a section header.
///
/// Before the first refresh cycle ends (`loaded` is false), the header gives
/// no counts. The board then holds only the repositories that the worker
/// fetched so far, thus a count is too low.
fn header_line(
    section: Section,
    count: usize,
    passed: usize,
    failed: usize,
    window: Duration,
    loaded: bool,
) -> String {
    let window = format_duration_setting(window);
    match (section, loaded) {
        (Section::Active, false) => "Running".to_owned(),
        (Section::Active, true) => format!("Running · {count}"),
        (Section::Recent, false) => format!("Last {window}"),
        (Section::Recent, true) => format!("Last {window} · {passed} passed · {failed} failed"),
    }
}

/// The line that stands in an empty section.
///
/// Before the first refresh cycle ends (`loaded` is false), the line is
/// [`SECTION_LOADING`]. A section that is empty then can still get runs, and
/// a text that says "no runs" makes a person think that nothing runs.
fn empty_line(section: Section, window: Duration, loaded: bool) -> String {
    if !loaded {
        return SECTION_LOADING.to_owned();
    }
    match section {
        Section::Active => "  No runs in progress or in the queue.".to_owned(),
        Section::Recent => format!(
            "  No runs finished in the last {}.",
            format_duration_setting(window)
        ),
    }
}

/// The first part of the status line, or `None` when there is nothing to say.
///
/// A live message wins over the errors. A message that is
/// [`MESSAGE_LIFETIME`] old or older is absent, thus the errors show. The
/// errors come from the board (one for each repository) and from the status
/// (one for each owner). The text names the first and counts the rest.
fn lead(board: &Board, status: &Status, now: DateTime<Utc>) -> Option<String> {
    if let Some(message) = status.message_at(now) {
        return Some(message.text.clone());
    }
    let mut errors: Vec<(String, &str)> = board
        .errors()
        .into_iter()
        .map(|(repo, error)| (repo.to_string(), error))
        .collect();
    errors.extend(
        status
            .owner_errors
            .iter()
            .map(|(owner, error)| (owner.to_string(), error.as_str())),
    );
    if let Some((name, error)) = errors.first() {
        let more = match errors.len() - 1 {
            0 => String::new(),
            count => format!(" (+{count} more)"),
        };
        return Some(format!("⚠ {name}: {error}{more}"));
    }
    None
}

/// The first index of the xterm 256-color gray ramp that a new message uses.
/// Index 252 is the color (208, 208, 208), the light gray that the push
/// status of `gsw` starts with.
const MESSAGE_START_GRAY: u8 = 252;
/// The number of steps from the first gray to the last. Index 232 is the
/// color (8, 8, 8), which is near black.
const MESSAGE_FADE_STEPS: u8 = 20;

/// The color of a status message of the age `age`.
///
/// The message fades from light gray to near black over [`MESSAGE_LIFETIME`].
/// The color comes from the gray ramp of the 256-color palette, which every
/// terminal that shows 256 colors has. Thus `ha` needs no detection of
/// truecolor support. An age of zero gives index 252, half of the lifetime
/// gives index 242, and an age of the lifetime or more gives index 232.
fn message_color(age: Duration) -> Color {
    let steps = u128::from(MESSAGE_FADE_STEPS) * age.as_millis() / MESSAGE_LIFETIME.as_millis();
    let step = u8::try_from(steps).map_or(MESSAGE_FADE_STEPS, |s| s.min(MESSAGE_FADE_STEPS));
    Color::Indexed(MESSAGE_START_GRAY - step)
}

/// The text before the age of the last refresh.
const REFRESH_AGE_PREFIX: &str = "updated ";
/// The text after the age of the last refresh.
const REFRESH_AGE_SUFFIX: &str = " ago";
/// The width of the refresh part of the status line. The age is never wider
/// than the time column.
const REFRESH_AGE_WIDTH: usize = REFRESH_AGE_PREFIX.len() + TIME_WIDTH + REFRESH_AGE_SUFFIX.len();

/// The refresh part of the status line: how long ago the last refresh cycle
/// ended, for example `updated 1m 05s ago`.
///
/// The age has the units of the time column ([`format_elapsed`]). A last
/// refresh in the future of `now` is `0s` ago. The part is padded to
/// [`REFRESH_AGE_WIDTH`], thus the parts after it keep their columns when the
/// age grows from `9s` to `10s` or from `59s` to `1m 00s`.
fn refresh_age(last_refresh: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let age = format_elapsed(span(last_refresh, now));
    pad_right(
        &format!("{REFRESH_AGE_PREFIX}{age}{REFRESH_AGE_SUFFIX}"),
        REFRESH_AGE_WIDTH,
    )
}

/// The left part of the status line: the lead, the refresh, the rate limit,
/// and the key hints.
///
/// The parts stand in this order, with [`STATUS_SEPARATOR`] between them.
/// The refresh mark is not part of it. [`layout_status`] puts the mark at the
/// right end of the line.
fn status_line<Tz: TimeZone>(board: &Board, status: &Status, now: DateTime<Utc>, tz: &Tz) -> String
where
    Tz::Offset: Display,
{
    let mut parts: Vec<String> = lead(board, status, now).into_iter().collect();
    parts.push(match status.last_refresh {
        Some(at) => refresh_age(at, now),
        None => "loading…".to_owned(),
    });
    if let Some(rate) = status.rate {
        parts.push(match status.backoff_until {
            Some(until) => format!(
                "API low {}/{}, next refresh {}",
                rate.remaining,
                rate.limit,
                until.with_timezone(tz).format("%H:%M")
            ),
            None => format!("API {}/{}", rate.remaining, rate.limit),
        });
    }
    parts.push(KEY_HINTS.to_owned());
    parts.join(STATUS_SEPARATOR)
}

/// The status line, laid out in the columns of the frame.
#[derive(Debug, PartialEq, Eq)]
struct StatusLayout {
    /// The text of the whole line. It is never wider than the frame.
    text: String,
    /// The number of columns that the left part can fill, from the first
    /// column. The fade of a message stops at this column.
    left_room: usize,
}

/// Lays out the status line in `width` columns.
///
/// The left part starts at the first column. The mark `mark`, when there is
/// one, ends at the last column. Thus a mark that comes and goes moves no part
/// of the left part. The left part gets the columns that the mark and a gap of
/// [`MARK_GAP`] columns leave, and a wider left part ends in `…`. When the
/// frame is narrower than the mark, the line holds the left part only.
fn layout_status(left: &str, mark: Option<&str>, width: usize) -> StatusLayout {
    let mark = mark.map(|mark| (mark, Span::raw(mark).width()));
    match mark {
        Some((mark, mark_width)) if mark_width <= width => {
            let mark_start = width - mark_width;
            let left_room = mark_start.saturating_sub(MARK_GAP);
            StatusLayout {
                text: format!(
                    "{}{mark}",
                    pad_right(&truncate_to_budget(left, left_room), mark_start)
                ),
                left_room,
            }
        }
        _ => StatusLayout {
            text: truncate_to_budget(left, width),
            left_room: width,
        },
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
    let area = frame.area();
    if area.is_empty() {
        return;
    }
    let widths = Widths::for_width(usize::from(area.width));
    let height = usize::from(area.height).saturating_sub(FIXED_LINES);
    let window = board.window();
    // The first refresh cycle ended. Until then, the board can miss runs.
    let loaded = status.last_refresh.is_some();
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
                let line = header_line(*section, *count, *passed, *failed, window, loaded);
                put(buffer, index + 1, &line, bold);
            }
            Row::Empty(section) => {
                let line = empty_line(*section, window, loaded);
                put(buffer, index + 1, &line, Style::default());
            }
            Row::Run { run, selected } => {
                let line = run_line(run, now, tz, widths);
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
    if usize::from(area.height) >= FIXED_LINES {
        let layout = layout_status(
            &status_line(board, status, now, tz),
            status.refreshing.then_some(REFRESH_MARK),
            usize::from(area.width),
        );
        let y = put(
            buffer,
            usize::from(area.height) - 1,
            &layout.text,
            Style::default(),
        );
        if let Some(message) = status.message_at(now) {
            let width = Span::raw(message.text.as_str())
                .width()
                .min(layout.left_room);
            let width = u16::try_from(width).unwrap_or(u16::MAX);
            buffer.set_style(
                Rect::new(area.x, y, width, 1),
                Style::default().fg(message_color(span(message.posted_at, now))),
            );
        }
    }
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
    use std::fmt::Display;

    use ratatui::Terminal;

    use crate::run::{Run, RunId, RunNumber, RunState};
    use crate::status::{Message, RateLimit};
    use crate::target::{OwnerName, RepoName};

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

    #[expect(
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
            number: RunNumber(id + 100),
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

    /// Draws one frame in the time zone `tz` and returns the buffer.
    fn render_in<Tz: TimeZone>(
        width: u16,
        height: u16,
        board: &mut Board,
        status: &Status,
        tz: &Tz,
    ) -> Buffer
    where
        Tz::Offset: Display,
    {
        let mut terminal =
            Terminal::new(TestBackend::new(width, height)).expect("a test terminal opens");
        terminal
            .draw(|frame| draw(frame, board, status, now(), tz))
            .expect("a test frame draws");
        terminal.backend().buffer().clone()
    }

    /// Draws one frame in UTC and returns the buffer.
    fn render_with(width: u16, height: u16, board: &mut Board, status: &Status) -> Buffer {
        render_in(width, height, board, status, &utc())
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

    /// A status after the first refresh cycle ended at 11:59:30.
    fn loaded() -> Status {
        Status {
            last_refresh: Some(at("2026-10-14T11:59:30Z")),
            ..Status::default()
        }
    }

    #[test]
    fn the_frame_holds_the_header_the_rows_and_the_status_line() {
        let mut board = four_run_board();
        let buffer = render_with(120, 10, &mut board, &loaded());
        assert_eq!(
            lines(&buffer),
            [
                "  REPO                WORKFLOW            RUN BRANCH          TITLE                         STARTED     TIME    FINISHED",
                "Running · 2",
                "● timmattison/tools   CI                 #101 main            Fix the thing                 11:45:30    14m 30s",
                "○ acme/widgets        Release            #102 issue-12        Add release job               11:58:00    2m 00s",
                "Last 8h · 1 passed · 1 failed",
                "✓ timmattison/tools   CI                 #103 main            Bump deps                     09:00:00    10m 05s 2h ago",
                "✗ acme/widgets        Release            #104 issue-11        Broken build                  10-13 23:50 4h 40m  7h ago",
                "",
                "",
                "updated 30s ago      ·  ↑↓ select  ⏎ open  r refresh  q quit",
            ]
        );
    }

    /// The characters of a line from column `from`, `count` columns wide.
    fn columns(line: &str, from: usize, count: usize) -> String {
        line.chars().skip(from).take(count).collect()
    }

    /// The first column of the run column in a frame of 120 columns. The
    /// marker, the repository (19), and the workflow (15) come before it.
    const RUN_COLUMN: usize = 1 + 1 + 19 + 1 + 15 + 1;

    #[test]
    fn the_run_column_shows_the_run_number_aligned_to_the_right() {
        let mut board = four_run_board();
        let text = lines(&render_with(120, 10, &mut board, &loaded()));
        assert_eq!(columns(&text[0], RUN_COLUMN, 7), "    RUN");
        assert_eq!(columns(&text[2], RUN_COLUMN, 7), "   #101");
        assert_eq!(columns(&text[3], RUN_COLUMN, 7), "   #102");
        assert_eq!(columns(&text[5], RUN_COLUMN, 7), "   #103");
        assert_eq!(columns(&text[6], RUN_COLUMN, 7), "   #104");
    }

    #[test]
    fn a_run_number_wider_than_the_column_is_cut_and_keeps_the_other_columns() {
        let mut board = Board::new(Duration::from_hours(8));
        board.replace(
            repo("acme/widgets"),
            vec![run(
                1_234_467,
                "acme/widgets",
                "CI",
                "main",
                "Wide number",
                RunState::Running,
                "2026-10-14T11:45:30Z",
                "2026-10-14T11:59:00Z",
            )],
        );
        let text = lines(&render_with(120, 10, &mut board, &loaded()));
        assert_eq!(columns(&text[2], RUN_COLUMN, 7), "#12345…");
        let started = text[0]
            .find("STARTED")
            .expect("the header names the started column");
        assert_eq!(columns(&text[2], started, 8), "11:45:30");
    }

    #[test]
    fn each_empty_section_says_loading_before_the_first_refresh_ends() {
        let mut board = Board::new(Duration::from_hours(8));
        let text = lines(&render_with(60, 8, &mut board, &Status::default()));
        assert_eq!(
            text[1..5],
            ["Running", "  Loading…", "Last 8h", "  Loading…"]
        );
    }

    #[test]
    fn the_headers_give_no_counts_before_the_first_refresh_ends() {
        let mut board = four_run_board();
        let text = lines(&render_with(120, 10, &mut board, &Status::default()));
        assert_eq!(text[1], "Running");
        assert!(text[2].starts_with("● timmattison/tools"), "{:?}", text[2]);
        assert_eq!(text[4], "Last 8h");
        assert!(text[5].starts_with("✓ timmattison/tools"), "{:?}", text[5]);
    }

    #[test]
    fn a_section_that_the_first_cycle_did_not_fill_yet_says_loading() {
        let mut board = Board::new(Duration::from_hours(8));
        board.replace(
            repo("timmattison/tools"),
            vec![run(
                1,
                "timmattison/tools",
                "CI",
                "main",
                "Fix the thing",
                RunState::Running,
                "2026-10-14T11:45:30Z",
                "2026-10-14T11:59:00Z",
            )],
        );
        let text = lines(&render_with(120, 8, &mut board, &Status::default()));
        assert_eq!(text[1], "Running");
        assert!(text[2].starts_with("● timmattison/tools"), "{:?}", text[2]);
        assert_eq!(text[3..5], ["Last 8h", "  Loading…"]);
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
        let buffer = render_with(60, 8, &mut board, &loaded());
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
        let buffer = render_with(60, 8, &mut board, &loaded());
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

    fn started_cells(text: &[String], y: usize) -> String {
        // The started column starts after 1 + 21 + 17 + 17 + 31 + 5 spaces.
        text[y]
            .chars()
            .skip(92)
            .take(11)
            .collect::<String>()
            .trim_end()
            .to_owned()
    }

    #[test]
    fn the_started_column_follows_the_time_zone() {
        let mut board = Board::new(Duration::from_hours(8));
        board.replace(
            repo("a/x"),
            vec![
                run(
                    1,
                    "a/x",
                    "CI",
                    "main",
                    "early",
                    RunState::Running,
                    "2026-10-13T16:00:00Z",
                    "2026-10-13T16:00:00Z",
                ),
                run(
                    2,
                    "a/x",
                    "CI",
                    "main",
                    "late",
                    RunState::Running,
                    "2026-10-13T14:59:00Z",
                    "2026-10-13T14:59:00Z",
                ),
            ],
        );
        let tokyo = chrono::FixedOffset::east_opt(9 * 3_600).expect("nine hours is valid");
        let in_tokyo = lines(&render_in(120, 8, &mut board, &Status::default(), &tokyo));
        let in_utc = lines(&render_with(120, 8, &mut board, &Status::default()));
        // Tokyo reads 21:00 on 10-14 now. 14:59 UTC is 23:59 on 10-13 there,
        // and 16:00 UTC is 01:00 on 10-14 there.
        assert_eq!(started_cells(&in_tokyo, 2), "10-13 23:59");
        assert_eq!(started_cells(&in_tokyo, 3), "01:00:00");
        assert_eq!(started_cells(&in_utc, 2), "10-13 14:59");
        assert_eq!(started_cells(&in_utc, 3), "10-13 16:00");
    }

    #[test]
    fn a_run_from_earlier_today_shows_the_clock_in_the_time_zone() {
        let mut board = Board::new(Duration::from_hours(8));
        board.replace(
            repo("a/x"),
            vec![run(
                1,
                "a/x",
                "CI",
                "main",
                "t",
                RunState::Running,
                "2026-10-13T16:00:00Z",
                "2026-10-13T16:00:00Z",
            )],
        );
        // 16:00 UTC on 10-13 is 01:00 on 10-14 at +09:00, and now is 21:00 on 10-14 there.
        let tokyo = chrono::FixedOffset::east_opt(9 * 3_600).expect("nine hours is valid");
        let text = lines(&render_in(120, 8, &mut board, &Status::default(), &tokyo));
        assert_eq!(started_cells(&text, 2), "01:00:00");
    }

    /// The text of the time and finished columns of line `y`.
    fn time_cells(text: &[String], y: usize) -> (String, String) {
        // After the started column: 1 + 21 + 17 + 17 + 31 + 11 + 6 spaces.
        let time: String = text[y].chars().skip(104).take(7).collect();
        let finished: String = text[y].chars().skip(112).take(8).collect();
        (time.trim_end().to_owned(), finished.trim_end().to_owned())
    }

    #[test]
    fn an_active_run_shows_the_time_since_the_start_and_no_finish() {
        let mut board = Board::new(Duration::from_hours(8));
        board.replace(
            repo("a/x"),
            vec![run(
                1,
                "a/x",
                "CI",
                "main",
                "t",
                RunState::Running,
                "2026-10-14T10:45:30Z",
                "2026-10-14T11:59:59Z",
            )],
        );
        let text = lines(&render_with(120, 8, &mut board, &Status::default()));
        assert_eq!(time_cells(&text, 2), ("1h 14m".to_owned(), String::new()));
    }

    #[test]
    fn a_finished_run_shows_its_own_duration_and_the_time_since_it_ended() {
        let mut board = Board::new(Duration::from_hours(8));
        board.replace(
            repo("a/x"),
            vec![run(
                1,
                "a/x",
                "CI",
                "main",
                "t",
                RunState::Failed,
                "2026-10-14T10:00:00Z",
                "2026-10-14T10:03:07Z",
            )],
        );
        let text = lines(&render_with(120, 8, &mut board, &Status::default()));
        assert_eq!(
            time_cells(&text, 4),
            ("3m 07s".to_owned(), "1h ago".to_owned())
        );
    }

    #[test]
    fn a_clock_that_runs_behind_gives_zero_and_no_panic() {
        let mut board = Board::new(Duration::from_hours(8));
        board.replace(
            repo("a/x"),
            vec![
                run(
                    1,
                    "a/x",
                    "CI",
                    "main",
                    "future start",
                    RunState::Running,
                    "2026-10-14T12:05:00Z",
                    "2026-10-14T12:05:00Z",
                ),
                run(
                    2,
                    "a/x",
                    "CI",
                    "main",
                    "future end",
                    RunState::Passed,
                    "2026-10-14T12:10:00Z",
                    "2026-10-14T12:02:00Z",
                ),
            ],
        );
        let text = lines(&render_with(120, 8, &mut board, &Status::default()));
        assert_eq!(time_cells(&text, 2), ("0s".to_owned(), String::new()));
        assert_eq!(time_cells(&text, 4), ("0s".to_owned(), "0s ago".to_owned()));
    }

    fn display_width(text: &str) -> usize {
        unicode_width::UnicodeWidthStr::width(text)
    }

    #[test]
    fn long_text_keeps_its_start_and_ends_with_an_ellipsis() {
        let mut board = Board::new(Duration::from_hours(8));
        board.replace(
            repo("timmattison/tools"),
            vec![run(
                1,
                "timmattison/tools",
                "Continuous Integration",
                "feature/long-branch",
                "A very long title indeed",
                RunState::Running,
                "2026-10-14T11:55:00Z",
                "2026-10-14T11:55:00Z",
            )],
        );
        let text = lines(&render_with(60, 6, &mut board, &Status::default()));
        // 60 - 34 = 26 columns for the text: 6, 5, 5, and 10.
        assert_eq!(
            text[2],
            "● timma… Cont… feat… A very lo… 11:55:00    5m 00s"
        );
    }

    #[test]
    fn wide_and_accented_branch_names_truncate_by_display_width() {
        let mut board = Board::new(Duration::from_hours(8));
        board.replace(
            repo("a/x"),
            vec![
                run(
                    1,
                    "a/x",
                    "CI",
                    "日本語のブランチ名",
                    "t",
                    RunState::Running,
                    "2026-10-14T11:51:00Z",
                    "2026-10-14T11:51:00Z",
                ),
                run(
                    2,
                    "a/x",
                    "CI",
                    "café-au-lait-branch",
                    "t",
                    RunState::Running,
                    "2026-10-14T11:52:00Z",
                    "2026-10-14T11:52:00Z",
                ),
                run(
                    3,
                    "a/x",
                    "CI",
                    "🎉🎉🎉🎉🎉🎉",
                    "t",
                    RunState::Running,
                    "2026-10-14T11:53:00Z",
                    "2026-10-14T11:53:00Z",
                ),
            ],
        );
        let text = lines(&render_with(60, 7, &mut board, &Status::default()));
        assert_eq!(
            text[2],
            "● a/x    CI    日本… t          11:51:00    9m 00s"
        );
        assert_eq!(
            text[3],
            "● a/x    CI    café… t          11:52:00    8m 00s"
        );
        assert_eq!(
            text[4],
            "● a/x    CI    🎉🎉… t          11:53:00    7m 00s"
        );
        for line in &text {
            assert!(display_width(line) <= 60, "{line:?} is too wide");
        }
    }

    #[test]
    fn the_text_columns_share_the_width_at_every_size() {
        let title = "Fix the flaky retry logic in the uploader";
        let mut board = Board::new(Duration::from_hours(8));
        board.replace(
            repo("timmattison/tools"),
            vec![run(
                1,
                "timmattison/tools",
                "CI",
                "main",
                title,
                RunState::Running,
                "2026-10-14T11:55:00Z",
                "2026-10-14T11:55:00Z",
            )],
        );
        let narrow = lines(&render_with(80, 6, &mut board, &Status::default()));
        let wide = lines(&render_with(160, 6, &mut board, &Status::default()));
        assert_eq!(display_width(&narrow[0]), 80, "the header fills 80 columns");
        assert_eq!(display_width(&wide[0]), 160, "the header fills 160 columns");
        for line in &narrow {
            assert!(display_width(line) <= 80, "{line:?} is too wide");
        }
        for line in &wide {
            assert!(display_width(line) <= 160, "{line:?} is too wide");
        }
        assert!(!narrow[2].contains(title), "80 columns cut the title");
        assert!(narrow[2].contains('…'));
        assert!(wide[2].contains(title), "160 columns show the title whole");
    }

    #[test]
    fn the_selected_run_stays_in_view_when_the_board_is_taller_than_the_frame() {
        let mut board = Board::new(Duration::from_hours(8));
        let runs: Vec<Run> = (0..12_u64)
            .map(|index| {
                let started = format!("2026-10-14T11:{:02}:00Z", 10 + index);
                run(
                    index + 1,
                    "a/x",
                    "CI",
                    "main",
                    &format!("title-{index:02}"),
                    RunState::Running,
                    &started,
                    &started,
                )
            })
            .collect();
        board.replace(repo("a/x"), runs);
        for _ in 0..9 {
            board.select_next(now());
        }
        // A frame of 8 lines leaves 6 for the board.
        let buffer = render_with(120, 8, &mut board, &Status::default());
        let text = lines(&buffer);
        let selected_line = (1..7_u16)
            .find(|y| buffer[(60, *y)].modifier.contains(Modifier::REVERSED))
            .expect("one visible row is selected");
        assert!(
            text[usize::from(selected_line)].contains("title-09"),
            "{:?}",
            text[usize::from(selected_line)]
        );
        assert!(text[0].starts_with("  REPO"), "the column header stays");
        assert!(text[7].contains("q quit"), "the status line stays");
        assert!(!text.iter().any(|line| line.contains("title-00")));
    }

    /// The status line of a frame that holds an empty board.
    fn status_line(width: u16, status: &Status) -> String {
        let mut board = Board::new(Duration::from_hours(8));
        let text = lines(&render_with(width, 8, &mut board, status));
        text[7].clone()
    }

    fn rate(remaining: u64) -> RateLimit {
        RateLimit {
            limit: 5_000,
            remaining,
            reset: at("2026-10-14T12:34:56Z"),
        }
    }

    #[test]
    fn the_status_line_says_loading_before_the_first_refresh() {
        assert_eq!(
            status_line(80, &Status::default()),
            "loading…  ·  ↑↓ select  ⏎ open  r refresh  q quit"
        );
    }

    #[test]
    fn the_status_line_shows_how_long_ago_the_last_refresh_ended() {
        let status = Status {
            last_refresh: Some(at("2026-10-14T11:59:30Z")),
            ..Status::default()
        };
        assert_eq!(
            status_line(80, &status),
            "updated 30s ago      ·  ↑↓ select  ⏎ open  r refresh  q quit"
        );
    }

    #[test]
    fn the_status_line_shows_the_rate_limit() {
        let status = Status {
            last_refresh: Some(at("2026-10-14T11:59:30Z")),
            rate: Some(rate(4_821)),
            ..Status::default()
        };
        assert_eq!(
            status_line(100, &status),
            "updated 30s ago      ·  API 4821/5000  ·  ↑↓ select  ⏎ open  r refresh  q quit"
        );
    }

    #[test]
    fn the_status_line_names_the_next_refresh_during_a_backoff() {
        let status = Status {
            last_refresh: Some(at("2026-10-14T11:59:30Z")),
            rate: Some(rate(412)),
            backoff_until: Some(at("2026-10-14T12:34:56Z")),
            ..Status::default()
        };
        assert_eq!(
            status_line(120, &status),
            "updated 30s ago      ·  API low 412/5000, next refresh 12:34  ·  ↑↓ select  ⏎ open  r refresh  q quit"
        );
    }

    /// The status line of an 80-column frame whose last refresh ended
    /// `age_seconds` before `now()`.
    fn status_line_aged(age_seconds: i64) -> String {
        let status = Status {
            last_refresh: Some(now() - chrono::Duration::seconds(age_seconds)),
            ..Status::default()
        };
        status_line(80, &status)
    }

    #[test]
    fn the_age_of_the_last_refresh_has_the_units_of_the_time_column() {
        for (age, text) in [
            (0, "updated 0s ago "),
            (59, "updated 59s ago "),
            (65, "updated 1m 05s ago "),
            (723, "updated 12m 03s ago "),
            (8_040, "updated 2h 14m ago "),
        ] {
            let line = status_line_aged(age);
            assert!(line.starts_with(text), "{age}s: {line:?}");
        }
    }

    #[test]
    fn a_last_refresh_in_the_future_is_zero_seconds_ago() {
        let line = status_line_aged(-5);
        assert!(line.starts_with("updated 0s ago "), "{line:?}");
    }

    #[test]
    fn the_parts_after_the_age_keep_their_columns_as_the_age_grows() {
        let hints_column = |line: &str| {
            line.split_once("↑↓")
                .map(|(before, _)| display_width(before))
        };
        let first = hints_column(&status_line_aged(0));
        assert!(first.is_some(), "the 80-column line holds the key hints");
        for age in [9, 10, 59, 60, 599, 600, 3_599, 3_600, 86_399, 86_400] {
            assert_eq!(hints_column(&status_line_aged(age)), first, "{age}s");
        }
    }

    #[test]
    fn the_status_line_is_cut_to_the_width() {
        assert_eq!(status_line(20, &Status::default()), "loading…  ·  ↑↓ sel…");
    }

    /// The status line of a frame that holds the board `board`.
    fn status_line_of(board: &mut Board, status: &Status) -> String {
        let text = lines(&render_with(140, 8, board, status));
        text[7].clone()
    }

    const HINTS_TAIL: &str = "loading…  ·  ↑↓ select  ⏎ open  r refresh  q quit";

    /// The mark at the right end of the status line during a refresh.
    const MARK: &str = "refreshing…";

    /// The status line of a 140-column frame: `left` at the start and the
    /// refresh mark at the end.
    fn with_mark(left: &str) -> String {
        format!("{}{MARK}", pad_right(left, 140 - display_width(MARK)))
    }

    #[test]
    fn the_status_line_names_the_first_failed_repository_and_counts_the_rest() {
        let mut board = Board::new(Duration::from_hours(8));
        board.fail(repo("owner/y"), "HTTP 500".to_owned());
        board.fail(repo("owner/x"), "HTTP 404".to_owned());
        assert_eq!(
            status_line_of(&mut board, &Status::default()),
            format!("⚠ owner/x: HTTP 404 (+1 more)  ·  {HINTS_TAIL}")
        );
    }

    #[test]
    fn one_error_has_no_count() {
        let mut board = Board::new(Duration::from_hours(8));
        board.fail(repo("owner/x"), "HTTP 404".to_owned());
        assert_eq!(
            status_line_of(&mut board, &Status::default()),
            format!("⚠ owner/x: HTTP 404  ·  {HINTS_TAIL}")
        );
    }

    #[test]
    fn an_owner_error_counts_with_the_repository_errors() {
        let mut board = Board::new(Duration::from_hours(8));
        board.fail(repo("owner/x"), "HTTP 404".to_owned());
        let status = Status {
            owner_errors: vec![(
                OwnerName::parse("acme").expect("the test owner is valid"),
                "not found".to_owned(),
            )],
            ..Status::default()
        };
        assert_eq!(
            status_line_of(&mut board, &status),
            format!("⚠ owner/x: HTTP 404 (+1 more)  ·  {HINTS_TAIL}")
        );
        let mut empty = Board::new(Duration::from_hours(8));
        assert_eq!(
            status_line_of(&mut empty, &status),
            format!("⚠ acme: not found  ·  {HINTS_TAIL}")
        );
    }

    #[test]
    fn a_message_replaces_the_errors() {
        let mut board = Board::new(Duration::from_hours(8));
        board.fail(repo("owner/x"), "HTTP 404".to_owned());
        let status = Status {
            message: Some(Message {
                text: "could not open a browser: https://example.com/run/1".to_owned(),
                posted_at: now(),
            }),
            refreshing: true,
            ..Status::default()
        };
        assert_eq!(
            status_line_of(&mut board, &status),
            with_mark(&format!(
                "could not open a browser: https://example.com/run/1  ·  {HINTS_TAIL}"
            ))
        );
    }

    /// A status whose message `text` was posted `age_seconds` before `now()`.
    fn status_with_message_aged(text: &str, age_seconds: i64) -> Status {
        Status {
            message: Some(Message {
                text: text.to_owned(),
                posted_at: now() - chrono::Duration::seconds(age_seconds),
            }),
            ..Status::default()
        }
    }

    #[test]
    fn a_message_leaves_the_status_line_after_its_lifetime() {
        let mut board = Board::new(Duration::from_hours(8));
        assert_eq!(
            status_line_of(&mut board, &status_with_message_aged("hello there", 29)),
            format!("hello there  ·  {HINTS_TAIL}")
        );
        assert_eq!(
            status_line_of(&mut board, &status_with_message_aged("hello there", 30)),
            status_line_of(&mut board, &Status::default())
        );
    }

    #[test]
    fn a_message_fades_along_the_gray_ramp() {
        let mut board = Board::new(Duration::from_hours(8));
        let text = "hello there";
        let after = u16::try_from(text.len() + STATUS_SEPARATOR.chars().count()).unwrap();
        for (age, gray) in [(0, 252), (15, 242), (29, 233)] {
            let buffer = render_with(140, 8, &mut board, &status_with_message_aged(text, age));
            assert_eq!(buffer[(0, 7)].fg, Color::Indexed(gray), "age {age}");
            assert_eq!(
                buffer[(u16::try_from(text.len()).unwrap() - 1, 7)].fg,
                Color::Indexed(gray),
                "age {age}"
            );
            assert_eq!(buffer[(after, 7)].symbol(), "l", "age {age}");
            assert_eq!(buffer[(after, 7)].fg, Color::Reset, "age {age}");
        }
    }

    #[test]
    fn a_long_message_on_a_narrow_frame_fades_every_cell_it_covers() {
        let mut board = Board::new(Duration::from_hours(8));
        let status = status_with_message_aged(&"m".repeat(50), 0);
        let buffer = render_with(20, 8, &mut board, &status);
        for x in 0..20 {
            assert_eq!(buffer[(x, 7)].fg, Color::Indexed(252), "cell {x}");
        }
    }

    #[test]
    fn refreshing_stands_at_the_right_end_of_the_status_line() {
        let mut board = Board::new(Duration::from_hours(8));
        let status = Status {
            refreshing: true,
            ..Status::default()
        };
        assert_eq!(status_line_of(&mut board, &status), with_mark(HINTS_TAIL));
        board.fail(repo("owner/x"), "HTTP 404".to_owned());
        assert_eq!(
            status_line_of(&mut board, &status),
            with_mark(&format!("⚠ owner/x: HTTP 404  ·  {HINTS_TAIL}"))
        );
    }

    #[test]
    fn a_refresh_moves_no_part_of_the_status_line() {
        let mut board = Board::new(Duration::from_hours(8));
        let idle = Status {
            rate: Some(rate(4_821)),
            ..loaded()
        };
        let busy = Status {
            refreshing: true,
            ..idle.clone()
        };
        let idle_line = status_line_of(&mut board, &idle);
        let busy_line = status_line_of(&mut board, &busy);
        assert!(busy_line.starts_with(&idle_line), "{busy_line:?}");
        assert!(busy_line.ends_with(MARK), "{busy_line:?}");
        assert_eq!(display_width(&busy_line), 140, "{busy_line:?}");
    }

    #[test]
    fn on_a_narrow_frame_the_refresh_mark_cuts_the_left_part() {
        let status = Status {
            refreshing: true,
            ..Status::default()
        };
        assert_eq!(
            status_line(40, &status),
            "loading…  ·  ↑↓ select  ⏎ …  refreshing…"
        );
    }

    #[test]
    fn a_frame_narrower_than_the_refresh_mark_shows_the_left_part_only() {
        let busy = Status {
            refreshing: true,
            ..Status::default()
        };
        assert_eq!(status_line(10, &busy), status_line(10, &Status::default()));
    }

    #[test]
    fn a_long_message_does_not_fade_the_refresh_mark() {
        let mut board = Board::new(Duration::from_hours(8));
        let status = Status {
            refreshing: true,
            ..status_with_message_aged(&"m".repeat(50), 0)
        };
        let buffer = render_with(40, 8, &mut board, &status);
        assert_eq!(
            lines(&buffer)[7],
            format!("{}…  refreshing…", "m".repeat(26))
        );
        for x in 0..27 {
            assert_eq!(buffer[(x, 7)].fg, Color::Indexed(252), "cell {x}");
        }
        for x in 27..40 {
            assert_eq!(buffer[(x, 7)].fg, Color::Reset, "cell {x}");
        }
    }

    #[test]
    fn a_tiny_frame_does_not_panic_and_nothing_overflows() {
        for (width, height) in [
            (0_u16, 0_u16),
            (1, 1),
            (3, 2),
            (20, 3),
            (33, 4),
            (34, 5),
            (1, 40),
            (200, 1),
        ] {
            let mut board = four_run_board();
            board.fail(repo("owner/x"), "HTTP 404".to_owned());
            let status = Status {
                refreshing: true,
                last_refresh: Some(at("2026-10-14T11:59:30Z")),
                rate: Some(rate(412)),
                backoff_until: Some(at("2026-10-14T12:34:56Z")),
                ..Status::default()
            };
            let buffer = render_with(width, height, &mut board, &status);
            for line in lines(&buffer) {
                assert!(
                    display_width(&line) <= usize::from(width),
                    "{width}x{height}: {line:?}"
                );
            }
        }
    }

    #[test]
    fn a_frame_of_two_lines_holds_the_header_and_the_status_line() {
        let mut board = four_run_board();
        let text = lines(&render_with(120, 2, &mut board, &Status::default()));
        assert_eq!(text.len(), 2);
        assert!(text[0].starts_with("  REPO"));
        assert!(text[1].starts_with("loading…"));
    }
}
