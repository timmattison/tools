//! Draws the board of `ha` and the text formats of its columns.

use std::fmt::Display;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Clear, Widget};
use ratatui::Frame;
use textfit::{pad_right, truncate_to_budget};

use crate::board::{Board, Row, Section};
use crate::cancel::Confirm;
use crate::job::Job;
use crate::run::{Run, RunState};
use crate::settings::format_duration_setting;
use crate::status::{Status, MESSAGE_LIFETIME};

/// The width of the marker column.
const MARKER_WIDTH: usize = 1;
/// The width of the ID part. It fits an ID of 12 digits. On 2026-10-18, run
/// IDs have 11 digits and job IDs have 12 digits.
const ID_WIDTH: usize = 12;
/// The width of the started column.
const STARTED_WIDTH: usize = 11;
/// The width of the time column.
const TIME_WIDTH: usize = 7;
/// The columns that the text columns do not share: the marker, the ID part,
/// started, and time columns, and the seven spaces between the columns.
const FIXED_COLUMNS: usize = MARKER_WIDTH + ID_WIDTH + STARTED_WIDTH + TIME_WIDTH + 7;
/// The header word of the workflow column.
const WORKFLOW_HEADER: &str = "WORKFLOW";
/// The most columns that the workflow name gets: the width of its header word.
/// Most workflow names fit, for example `CI`, `Release`, and `CodeQL`.
const MAX_WORKFLOW_WIDTH: usize = WORKFLOW_HEADER.len();
/// The most columns that the branch gets. A person reads the start of a branch
/// name, for example `issue-577` or `main`. Past 12 columns, the rest of a name
/// seldom helps to find a run, and a longer name ends in `…`.
const MAX_BRANCH_WIDTH: usize = 12;
/// The lines that are not board rows: the column header and the status line.
const FIXED_LINES: usize = 2;
/// The text that stands between two parts of the status line.
const STATUS_SEPARATOR: &str = "  ·  ";
/// The key hints at the end of the left part of the status line. The status line
/// shows them only while no cancel box shows.
const KEY_HINTS: &str = "↑↓ select  →← jobs  ⏎ open  c cancel  r refresh  q quit";
/// The title in the top border of the box of the cancel prompt.
const PROMPT_TITLE: &str = " Cancel run? ";
/// The widest outer width of the box of the cancel prompt.
const PROMPT_WIDTH: usize = 44;
/// The columns of the box that hold no text: a border column and a space of
/// padding on each side.
const PROMPT_FRAME_COLUMNS: usize = 4;
/// The rows of the box that hold no text: the top border and the bottom border.
const PROMPT_BORDER_ROWS: usize = 2;
/// The note that the box shows when a job row is selected.
const PROMPT_JOB_NOTE: &str = "GitHub cannot cancel one job. y cancels every job of this run.";
/// The key that cancels the run.
const PROMPT_KEY_CANCEL: &str = "y cancel the run";
/// The text for each other key.
const PROMPT_KEY_KEEP: &str = "any other key: keep";
/// The text between the two keys when they share one line.
const PROMPT_KEY_GAP: &str = "   ";
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
    /// Shares the columns that the fixed columns leave over: 24 percent to the
    /// repository, 19 percent to the workflow name, 21 percent to the branch,
    /// and the rest to the title.
    ///
    /// The workflow name never gets more than [`MAX_WORKFLOW_WIDTH`] columns,
    /// the width of its header word, and the branch never gets more than
    /// [`MAX_BRANCH_WIDTH`] columns. The title gets each column that the two
    /// give up. A workflow name is usually short, a person reads only the start
    /// of a branch name, and the title is the longest text of a row. A frame
    /// that is too narrow gives each 0.
    fn for_width(width: usize) -> Widths {
        let room = width.saturating_sub(FIXED_COLUMNS);
        let repo = room * 24 / 100;
        let workflow = (room * 19 / 100).min(MAX_WORKFLOW_WIDTH);
        let branch = (room * 21 / 100).min(MAX_BRANCH_WIDTH);
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

/// Cuts a text to a column width, then pads it on the left to that width.
///
/// The text must be ASCII, for example the ID `39149840084`, because the padding counts
/// characters. The cut uses `textfit` and so is safe for any text.
fn fit_right(text: &str, width: usize) -> String {
    let cut = truncate_to_budget(text, width);
    let padding = width.saturating_sub(cut.chars().count());
    format!("{}{cut}", " ".repeat(padding))
}

/// Joins the cells of one row into one line. One space stands between cells.
///
/// The ID part stands after the workflow cell, and `id` is right-aligned in it.
/// The header line does not come from this function. `header_cells` builds it.
fn cells(
    widths: Widths,
    marker: &str,
    texts: [&str; 4],
    id: &str,
    started: &str,
    time: &str,
) -> String {
    let [repo, workflow, branch, title] = texts;
    [
        pad_right(marker, MARKER_WIDTH),
        fit(repo, widths.repo),
        fit(workflow, widths.workflow),
        fit_right(id, ID_WIDTH),
        fit(branch, widths.branch),
        fit(title, widths.title),
        pad_right(started, STARTED_WIDTH),
        pad_right(time, TIME_WIDTH),
    ]
    .join(" ")
}

/// Joins the cells of the header line. One space stands between cells.
///
/// The word `WORKFLOW` fills the workflow cell, one space, and the ID part. Thus the
/// whole word shows at every frame width, and it heads the workflow name and the ID.
/// The other cells stand in the same columns as those of a run row.
fn header_cells(widths: Widths) -> String {
    [
        pad_right(" ", MARKER_WIDTH),
        fit("REPO", widths.repo),
        fit(WORKFLOW_HEADER, widths.workflow + 1 + ID_WIDTH),
        fit("BRANCH", widths.branch),
        fit("TITLE", widths.title),
        pad_right("STARTED", STARTED_WIDTH),
        pad_right("TIME", TIME_WIDTH),
    ]
    .join(" ")
}

/// Joins the cells of one job row into one line. One space stands between cells.
///
/// The text fills the repository cell, one space, and the workflow-name cell.
/// The job ID stands in the ID part of a run row, right-aligned. The branch and
/// title columns stay blank. Thus the ID, started, and time cells stand in the
/// same columns as those of a run row, at every frame width.
fn job_cells(
    widths: Widths,
    marker: &str,
    text: &str,
    id: &str,
    started: &str,
    time: &str,
) -> String {
    [
        pad_right(marker, MARKER_WIDTH),
        fit(text, widths.repo + 1 + widths.workflow),
        fit_right(id, ID_WIDTH),
        " ".repeat(widths.branch + 1 + widths.title),
        pad_right(started, STARTED_WIDTH),
        pad_right(time, TIME_WIDTH),
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

/// The line of one run. `state` is the state that the row shows, not the state
/// that GitHub reports. It sets the marker and the time cells.
fn run_line<Tz: TimeZone>(
    run: &Run,
    state: RunState,
    now: DateTime<Utc>,
    tz: &Tz,
    widths: Widths,
) -> String
where
    Tz::Offset: Display,
{
    let time = if state.is_active() {
        format_elapsed(span(run.started_at, now))
    } else {
        format_elapsed(span(run.started_at, run.updated_at))
    };
    cells(
        widths,
        marker(state),
        [
            &run.repo.to_string(),
            &run.workflow,
            &run.branch,
            &run.title,
        ],
        &run.id.0.to_string(),
        &format_started(run.started_at, now, tz),
        &time,
    )
}

/// The line of one job of an expanded run.
///
/// The text cell holds the tree glyph (`└` when `last` is true, else `├`), a
/// space, and the name of the job. The job ID follows, in the columns of the
/// run ID.
///
/// A job that waits did not start, thus its started cell is empty. Its time
/// cell shows how long it waited, from its creation until `now`, and is empty
/// when the job has no creation time. The time cells of each other job follow
/// the rules of [`run_line`], with the finish time of the job in place of the
/// update time. Such a job with no start time has an empty started cell and an
/// empty time cell.
fn job_line<Tz: TimeZone>(
    job: &Job,
    last: bool,
    now: DateTime<Utc>,
    tz: &Tz,
    widths: Widths,
) -> String
where
    Tz::Offset: Display,
{
    let (started, time) = if job.state == RunState::Queued {
        // A job that waits did not start. Its time is the wait, from its creation.
        (
            String::new(),
            job.created_at
                .map(|created| format_elapsed(span(created, now)))
                .unwrap_or_default(),
        )
    } else {
        (
            job.started_at
                .map(|started| format_started(started, now, tz))
                .unwrap_or_default(),
            job.started_at.map_or_else(String::new, |started| {
                if job.state.is_active() {
                    format_elapsed(span(started, now))
                } else {
                    job.completed_at
                        .map_or_else(String::new, |done| format_elapsed(span(started, done)))
                }
            }),
        )
    };
    let glyph = if last { TREE_LAST } else { TREE_MIDDLE };
    job_cells(
        widths,
        marker(job.state),
        &format!("{glyph} {}", job.name),
        &job.id.0.to_string(),
        &started,
        &time,
    )
}

/// The text of the status row of an expanded run before the first answer.
const JOBS_LOADING: &str = "└ Loading jobs…";
/// The text of the status row of an expanded run whose answer holds no job.
const NO_JOBS: &str = "└ No jobs yet";
/// The text before the error on the status row of a failed fetch of jobs.
const JOBS_ERROR_PREFIX: &str = "└ ⚠ ";

/// The line of a status row of an expanded run.
///
/// The marker column is empty. The text fills the rest of the line and ends
/// in `…` when it is too wide. A status row has no time cells.
fn status_row_line(text: &str, width: usize) -> String {
    format!(
        "{} {}",
        pad_right("", MARKER_WIDTH),
        fit(text, width.saturating_sub(MARKER_WIDTH + 1))
    )
}

/// Writes a status row at row `y` of the buffer, as wide as the buffer.
fn put_status_row(buffer: &mut Buffer, y: usize, text: &str) {
    let line = status_row_line(text, usize::from(buffer.area.width));
    put(buffer, y, &line, Style::default());
}

/// The tree glyph of a job row that is not the last row of its block.
const TREE_MIDDLE: &str = "├";
/// The tree glyph of the last row of the block of a run.
const TREE_LAST: &str = "└";

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
/// With `hints` false, the key hints and the separator before them are absent.
/// The box of the cancel prompt names its own keys, thus the line drops the hints.
///
/// The parts stand in this order, with [`STATUS_SEPARATOR`] between them.
/// The refresh mark is not part of it. [`layout_status`] puts the mark at the
/// right end of the line.
fn status_line<Tz: TimeZone>(
    board: &Board,
    status: &Status,
    now: DateTime<Utc>,
    tz: &Tz,
    hints: bool,
) -> String
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
    if hints {
        parts.push(KEY_HINTS.to_owned());
    }
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

/// Reverses the colors across the full width of buffer row `y` when `selected`
/// is true. The highlight of a run row and of a job row use this one rule.
fn reverse_if(selected: bool, buffer: &mut Buffer, area: Rect, y: u16) {
    if selected {
        buffer.set_style(
            Rect::new(area.x, y, area.width, 1),
            Style::default().add_modifier(Modifier::REVERSED),
        );
    }
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
/// two fixed lines. Times show in the time zone `tz`. With a `prompt`, the box
/// of the cancel prompt draws last, over the rows (see [`draw_prompt`]). While
/// the box shows, the status line has no key hints. This function never
/// panics, whatever the size of the frame.
pub fn draw<Tz: TimeZone>(
    frame: &mut Frame,
    board: &mut Board,
    status: &Status,
    now: DateTime<Utc>,
    tz: &Tz,
    prompt: Option<&Confirm>,
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
    put(buffer, 0, &header_cells(widths), bold);
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
            Row::Run {
                run,
                state,
                selected,
            } => {
                let line = run_line(run, *state, now, tz, widths);
                let y = put(buffer, index + 1, &line, Style::default());
                buffer.set_style(
                    Rect::new(area.x, y, 1, 1),
                    Style::default().fg(marker_color(*state)),
                );
                reverse_if(*selected, buffer, area, y);
            }
            Row::Job {
                job,
                last,
                selected,
            } => {
                let line = job_line(job, *last, now, tz, widths);
                let y = put(buffer, index + 1, &line, Style::default());
                buffer.set_style(
                    Rect::new(area.x, y, 1, 1),
                    Style::default().fg(marker_color(job.state)),
                );
                reverse_if(*selected, buffer, area, y);
            }
            Row::JobsLoading => put_status_row(buffer, index + 1, JOBS_LOADING),
            Row::NoJobs => put_status_row(buffer, index + 1, NO_JOBS),
            Row::JobsError(error) => {
                put_status_row(buffer, index + 1, &format!("{JOBS_ERROR_PREFIX}{error}"));
            }
        }
    }
    if usize::from(area.height) >= FIXED_LINES {
        let layout = layout_status(
            &status_line(board, status, now, tz, prompt.is_none()),
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
    if let Some(prompt) = prompt {
        draw_prompt(buffer, area, prompt);
    }
}

/// The part of the box of the cancel prompt, in the order of the box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptPart {
    /// The repository.
    Repo,
    /// The workflow, the run ID, and the branch.
    Run,
    /// The title of the run.
    Title,
    /// The name of the job.
    Job,
    /// The note that GitHub cannot cancel one job.
    Note,
    /// The empty line above the keys.
    Blank,
}

/// The order in which a low frame removes the parts of the box. The empty line goes first,
/// because it holds no information. The key lines never go.
const PROMPT_DROP_ORDER: [PromptPart; 6] = [
    PromptPart::Blank,
    PromptPart::Title,
    PromptPart::Note,
    PromptPart::Job,
    PromptPart::Run,
    PromptPart::Repo,
];

/// Breaks `text` into lines of at most `width` columns.
///
/// A text that fits stays as it is. Else the break comes at spaces, and each
/// line takes as many words as fit. A word wider than `width` is cut with
/// [`truncate_to_budget`] and ends in `…`. The width is the display width, thus
/// a wide glyph never crosses the end of a line. The result holds at least one
/// line.
fn wrap_line(text: &str, width: usize) -> Vec<String> {
    if Span::raw(text).width() <= width {
        return vec![text.to_owned()];
    }
    let mut wrapped = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;
    for word in text.split_whitespace() {
        let word = if Span::raw(word).width() > width {
            truncate_to_budget(word, width)
        } else {
            word.to_owned()
        };
        let word_width = Span::raw(word.as_str()).width();
        if current.is_empty() {
            current = word;
            current_width = word_width;
        } else if current_width + 1 + word_width <= width {
            current.push(' ');
            current.push_str(&word);
            current_width += 1 + word_width;
        } else {
            wrapped.push(std::mem::replace(&mut current, word));
            current_width = word_width;
        }
    }
    wrapped.push(current);
    wrapped
}

/// The key lines of the box for a text `width`: one line when both keys fit,
/// else two lines.
fn prompt_key_lines(width: usize) -> Vec<String> {
    let joined = format!("{PROMPT_KEY_CANCEL}{PROMPT_KEY_GAP}{PROMPT_KEY_KEEP}");
    if Span::raw(joined.as_str()).width() <= width {
        return vec![joined];
    }
    [PROMPT_KEY_CANCEL, PROMPT_KEY_KEEP]
        .into_iter()
        .flat_map(|key| wrap_line(key, width))
        .collect()
}

/// The parts of the box above the key lines, each one wrapped to `width`.
fn prompt_parts(prompt: &Confirm, width: usize) -> Vec<(PromptPart, Vec<String>)> {
    let mut run_line = format!("{} {}", prompt.workflow, prompt.run.0);
    if !prompt.branch.is_empty() {
        run_line.push_str(" on ");
        run_line.push_str(&prompt.branch);
    }
    let mut parts = vec![
        (PromptPart::Repo, prompt.repo.to_string()),
        (PromptPart::Run, run_line),
        (PromptPart::Title, prompt.title.clone()),
    ];
    if let Some(job) = &prompt.job {
        parts.push((PromptPart::Job, format!("job: {job}")));
        parts.push((PromptPart::Note, PROMPT_JOB_NOTE.to_owned()));
    }
    parts.push((PromptPart::Blank, String::new()));
    parts
        .into_iter()
        .map(|(part, text)| (part, wrap_line(&text, width)))
        .collect()
}

/// Converts a row or column count to `u16`. A count past the range of `u16`
/// gives the largest value, and the caller clamps it to the frame.
fn to_u16(count: usize) -> u16 {
    u16::try_from(count).unwrap_or(u16::MAX)
}

/// Draws the box of the cancel prompt over the frame.
///
/// The box is at most 44 columns wide and sits in the middle of the columns.
/// It sits in the middle of the board rows, between the column header and the
/// status line. Its lines are the repository, the run, the title, the job and
/// a note (for a job row only), an empty line, and the keys. Each line wraps
/// at spaces to the text width, which is the outer width minus 4.
///
/// A frame that is too low drops whole parts, in this order: the empty
/// line, the title, the note, the job, the run, the repository. The key lines
/// stay. When the board rows cannot hold the border and the key lines, the box
/// uses all rows of the frame. When even the frame cannot hold them, the box
/// has no border and shows the key lines from the first row, as many as fit.
fn draw_prompt(buffer: &mut Buffer, area: Rect, prompt: &Confirm) {
    let outer = area.width.min(to_u16(PROMPT_WIDTH));
    let x = area.x + (area.width - outer) / 2;
    let height = usize::from(area.height);
    let text_width = usize::from(outer).saturating_sub(PROMPT_FRAME_COLUMNS);
    let keys = prompt_key_lines(text_width);
    let board_rows = height.saturating_sub(FIXED_LINES);
    let (first_row, rows) = if board_rows >= keys.len() + PROMPT_BORDER_ROWS {
        (1, board_rows)
    } else {
        (0, height)
    };
    if rows < keys.len() + PROMPT_BORDER_ROWS {
        let keys = prompt_key_lines(usize::from(outer));
        let used = keys.len().min(height);
        Clear.render(Rect::new(x, area.y, outer, to_u16(used)), buffer);
        for (index, line) in keys.iter().take(used).enumerate() {
            buffer.set_stringn(
                x,
                area.y + to_u16(index),
                line,
                usize::from(outer),
                Style::default(),
            );
        }
        return;
    }
    let mut parts = prompt_parts(prompt, text_width);
    let room = rows - PROMPT_BORDER_ROWS;
    let total = |parts: &[(PromptPart, Vec<String>)]| {
        keys.len() + parts.iter().map(|(_, lines)| lines.len()).sum::<usize>()
    };
    for dropped in PROMPT_DROP_ORDER {
        if total(&parts) <= room {
            break;
        }
        parts.retain(|(part, _)| *part != dropped);
    }
    let text: Vec<&String> = parts
        .iter()
        .flat_map(|(_, lines)| lines)
        .chain(&keys)
        .collect();
    let box_height = text.len() + PROMPT_BORDER_ROWS;
    let y = area.y + to_u16(first_row + (rows - box_height) / 2);
    let rect = Rect::new(x, y, outer, to_u16(box_height));
    Clear.render(rect, buffer);
    Block::bordered().title(PROMPT_TITLE).render(rect, buffer);
    for (index, line) in text.into_iter().enumerate() {
        buffer.set_stringn(
            x + 2,
            y + 1 + to_u16(index),
            line,
            text_width,
            Style::default(),
        );
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

#[cfg(test)]
mod tests {
    use super::*;

    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::{Color, Modifier};
    use std::fmt::Display;

    use ratatui::Terminal;

    use crate::job::{Job, JobId};
    use crate::run::{Run, RunId, RunState};
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
            .draw(|frame| draw(frame, board, status, now(), tz, None))
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

    /// These lines are the target example of issue #571, with the two section
    /// headers added. The workflow name is 8 columns wide and the branch is 12
    /// columns wide (issue #577), thus the title has 12 columns more than in
    /// that example.
    #[test]
    fn a_frame_of_120_columns_has_the_target_layout_of_issue_571() {
        let mut board = Board::new(Duration::from_hours(8));
        board.replace(
            repo("timmattison/tools"),
            vec![run(
                39_149_840_084,
                "timmattison/tools",
                "CI",
                "main",
                "Fix the thing",
                RunState::Running,
                "2026-10-14T11:45:30Z",
                "2026-10-14T11:59:00Z",
            )],
        );
        board.replace(
            repo("acme/widgets"),
            vec![run(
                39_149_711_502,
                "acme/widgets",
                "Release",
                "issue-11",
                "Bump deps",
                RunState::Passed,
                "2026-10-14T09:00:00Z",
                "2026-10-14T09:10:05Z",
            )],
        );
        board.expand(now());
        board.set_jobs(
            RunId(39_149_840_084),
            vec![
                job(
                    107_851_237_794,
                    "build",
                    RunState::Passed,
                    Some("2026-10-14T11:45:40Z"),
                    Some("2026-10-14T11:50:00Z"),
                ),
                job(
                    107_851_237_795,
                    "lint",
                    RunState::Failed,
                    Some("2026-10-14T11:46:00Z"),
                    Some("2026-10-14T11:46:45Z"),
                ),
                job(
                    107_851_237_796,
                    "test",
                    RunState::Running,
                    Some("2026-10-14T11:50:10Z"),
                    None,
                ),
            ],
        );
        let text = lines(&render_with(120, 10, &mut board, &loaded()));
        assert_eq!(
            text[0..8],
            [
                "  REPO                WORKFLOW              BRANCH       TITLE                                       STARTED     TIME",
                "Running · 1",
                "● timmattison/tools   CI        39149840084 main         Fix the thing                               11:45:30    14m 30s",
                "✓ ├ build                      107851237794                                                          11:45:40    4m 20s",
                "✗ ├ lint                       107851237795                                                          11:46:00    45s",
                "● └ test                       107851237796                                                          11:50:10    9m 50s",
                "Last 8h · 1 passed · 0 failed",
                "✓ acme/widgets        Release   39149711502 issue-11     Bump deps                                   09:00:00    10m 05s",
            ]
        );
    }

    #[test]
    fn the_frame_holds_the_header_the_rows_and_the_status_line() {
        let mut board = four_run_board();
        let buffer = render_with(120, 10, &mut board, &loaded());
        assert_eq!(
            lines(&buffer),
            [
                "  REPO                WORKFLOW              BRANCH       TITLE                                       STARTED     TIME",
                "Running · 2",
                "● timmattison/tools   CI                  1 main         Fix the thing                               11:45:30    14m 30s",
                "○ acme/widgets        Release             2 issue-12     Add release job                             11:58:00    2m 00s",
                "Last 8h · 1 passed · 1 failed",
                "✓ timmattison/tools   CI                  3 main         Bump deps                                   09:00:00    10m 05s",
                "✗ acme/widgets        Release             4 issue-11     Broken build                                10-13 23:50 4h 40m",
                "",
                "",
                "updated 30s ago      ·  ↑↓ select  →← jobs  ⏎ open  c cancel  r refresh  q quit",
            ]
        );
    }

    /// The characters of a line from column `from`, `count` columns wide.
    fn columns(line: &str, from: usize, count: usize) -> String {
        line.chars().skip(from).take(count).collect()
    }

    #[test]
    fn a_run_id_wider_than_the_id_part_is_cut_and_keeps_the_other_columns() {
        let mut board = Board::new(Duration::from_hours(8));
        board.replace(
            repo("acme/widgets"),
            vec![run(
                1_234_567_890_123,
                "acme/widgets",
                "CI",
                "main",
                "Wide ID",
                RunState::Running,
                "2026-10-14T11:45:30Z",
                "2026-10-14T11:59:00Z",
            )],
        );
        let text = lines(&render_with(120, 10, &mut board, &loaded()));
        let branch = header_column(&text, "BRANCH");
        assert_eq!(
            columns(&text[2], branch - 1 - ID_WIDTH, ID_WIDTH),
            "12345678901…"
        );
        assert_eq!(started_cells(&text, 2), "11:45:30");
    }

    #[test]
    fn the_header_names_the_workflow_column_with_the_whole_word() {
        for width in [80_u16, 120, 200] {
            let mut board = Board::new(Duration::from_hours(8));
            board.replace(
                repo("timmattison/tools"),
                vec![run(
                    391_498_400_841,
                    "timmattison/tools",
                    "CI",
                    "main",
                    "Fix the thing",
                    RunState::Running,
                    "2026-10-14T11:45:30Z",
                    "2026-10-14T11:59:00Z",
                )],
            );
            let text = lines(&render_with(width, 8, &mut board, &loaded()));
            assert!(
                text[0].split_whitespace().any(|word| word == "WORKFLOW"),
                "width {width}: the header does not hold the whole word: {:?}",
                text[0]
            );
            let name_column = text[2]
                .split_once("CI")
                .expect("the run row holds the workflow name")
                .0
                .chars()
                .count();
            assert_eq!(
                header_column(&text, "WORKFLOW"),
                name_column,
                "width {width}: the header word and the workflow name start in different columns"
            );
        }
    }

    #[test]
    fn the_workflow_cell_ends_in_the_whole_run_id() {
        for (width, id) in [
            (120_u16, 39_149_840_084_u64),
            (80, 391_498_400_841),
            (200, 391_498_400_841),
        ] {
            let mut board = Board::new(Duration::from_hours(8));
            board.replace(
                repo("timmattison/tools"),
                vec![run(
                    id,
                    "timmattison/tools",
                    "CI",
                    "main",
                    "Fix the thing",
                    RunState::Running,
                    "2026-10-14T11:45:30Z",
                    "2026-10-14T11:59:00Z",
                )],
            );
            let text = lines(&render_with(width, 8, &mut board, &loaded()));
            assert!(
                !text[0].split_whitespace().any(|word| word == "RUN"),
                "width {width}, ID {id}: the header holds a RUN word: {:?}",
                text[0]
            );
            let from = header_column(&text, "WORKFLOW");
            let branch = header_column(&text, "BRANCH");
            let cell = columns(&text[2], from, branch - 1 - from);
            assert!(
                cell.starts_with("CI"),
                "width {width}, ID {id}: the cell does not start with the name: {cell:?}"
            );
            assert!(
                cell.ends_with(&id.to_string()),
                "width {width}, ID {id}: the cell does not end in the whole ID: {cell:?}"
            );
            assert_eq!(
                columns(&text[2], branch - 1, 1),
                " ",
                "width {width}, ID {id}: no space stands before the branch cell"
            );
        }
    }

    #[test]
    fn a_long_workflow_name_is_cut_and_the_run_id_stays_whole() {
        let long_name = "日本語のワークフロー🎉".repeat(4);
        let mut board = Board::new(Duration::from_hours(8));
        board.replace(
            repo("timmattison/tools"),
            vec![
                run(
                    39_149_840_084,
                    "timmattison/tools",
                    "CI",
                    "feature-x",
                    "Short",
                    RunState::Running,
                    "2026-10-14T11:45:30Z",
                    "2026-10-14T11:59:00Z",
                ),
                run(
                    39_149_840_085,
                    "timmattison/tools",
                    &long_name,
                    "feature-x",
                    "Long",
                    RunState::Running,
                    "2026-10-14T11:46:30Z",
                    "2026-10-14T11:59:00Z",
                ),
            ],
        );
        let text = lines(&render_with(120, 8, &mut board, &loaded()));
        let (short, long) = (&text[2], &text[3]);
        assert!(long.contains("日本語"), "the name lost its start: {long:?}");
        let (before_id, _) = long
            .split_once("39149840085")
            .unwrap_or_else(|| panic!("the whole ID is missing: {long:?}"));
        assert!(
            before_id.trim_end().ends_with('…'),
            "the name does not end in an ellipsis before the ID: {long:?}"
        );
        let end_of = |line: &str, id: &str| {
            let (before, _) = line
                .split_once(id)
                .unwrap_or_else(|| panic!("{id} is missing: {line:?}"));
            display_width(before) + display_width(id)
        };
        assert_eq!(
            end_of(long, "39149840085"),
            end_of(short, "39149840084"),
            "the IDs end in different columns"
        );
        let branch_at = |line: &str| {
            let (before, _) = line
                .split_once("feature-x")
                .unwrap_or_else(|| panic!("the branch is missing: {line:?}"));
            display_width(before)
        };
        assert_eq!(branch_at(long), branch_at(short), "the branches move");
    }

    /// The name is 15 columns wide. It fits whole in the share that the
    /// workflow had before issue #577 at 120 columns, thus this test fails
    /// without the cap.
    #[test]
    fn a_workflow_name_wider_than_its_header_is_cut_and_the_branch_stays() {
        for width in 80_u16..=200 {
            let mut board = Board::new(Duration::from_hours(8));
            board.replace(
                repo("timmattison/tools"),
                vec![
                    run(
                        39_149_840_084,
                        "timmattison/tools",
                        "CI",
                        "main",
                        "Short",
                        RunState::Running,
                        "2026-10-14T11:45:30Z",
                        "2026-10-14T11:59:00Z",
                    ),
                    run(
                        39_149_840_085,
                        "timmattison/tools",
                        "🎉日本語 deploy",
                        "main",
                        "Long",
                        RunState::Running,
                        "2026-10-14T11:46:30Z",
                        "2026-10-14T11:59:00Z",
                    ),
                ],
            );
            let text = lines(&render_with(width, 8, &mut board, &loaded()));
            let (short, long) = (&text[2], &text[3]);
            let (before_id, _) = long
                .split_once("39149840085")
                .unwrap_or_else(|| panic!("width {width}: the whole ID is missing: {long:?}"));
            // The cell is 8 columns wide, and 7 at a frame of 80 columns. `語`
            // needs columns 7 and 8 of the name, thus `…` stands in column 7.
            assert!(
                before_id.trim_end().ends_with(" 🎉日本…"),
                "width {width}: the name does not keep its start and end in `…`: {long:?}"
            );
            let branch_at = |line: &str| {
                let (before, _) = line
                    .split_once("main")
                    .unwrap_or_else(|| panic!("width {width}: the branch is missing: {line:?}"));
                display_width(before)
            };
            assert_eq!(
                branch_at(long),
                branch_at(short),
                "width {width}: the branches move"
            );
        }
    }

    /// The branch is 19 columns wide. Without the cap, it fits whole in the
    /// share of the branch from 129 columns, and its cell is wider than 12
    /// columns from 100 columns. Thus this test fails without the cap.
    #[test]
    fn a_branch_wider_than_twelve_columns_is_cut_and_the_title_stays() {
        for width in 80_u16..=200 {
            let mut board = Board::new(Duration::from_hours(8));
            board.replace(
                repo("timmattison/tools"),
                vec![
                    run(
                        39_149_840_084,
                        "timmattison/tools",
                        "CI",
                        "main",
                        "Short",
                        RunState::Running,
                        "2026-10-14T11:45:30Z",
                        "2026-10-14T11:59:00Z",
                    ),
                    run(
                        39_149_840_085,
                        "timmattison/tools",
                        "CI",
                        "issue-577-日本語-🎉",
                        "Short",
                        RunState::Running,
                        "2026-10-14T11:46:30Z",
                        "2026-10-14T11:59:00Z",
                    ),
                ],
            );
            let text = lines(&render_with(width, 8, &mut board, &loaded()));
            let (short, long) = (&text[2], &text[3]);
            let (_, after_id) = long
                .split_once("39149840085 ")
                .unwrap_or_else(|| panic!("width {width}: the whole ID is missing: {long:?}"));
            let (branch, _) = after_id
                .split_once(" Short")
                .unwrap_or_else(|| panic!("width {width}: the title is missing: {long:?}"));
            assert!(
                branch.starts_with("issue-"),
                "width {width}: the branch lost its start: {long:?}"
            );
            assert!(
                branch.trim_end().ends_with('…'),
                "width {width}: the branch does not end in `…`: {long:?}"
            );
            assert!(
                display_width(branch) <= 12,
                "width {width}: the branch cell is wider than 12 columns: {long:?}"
            );
            let title_at = |line: &str| {
                let (before, _) = line
                    .split_once("Short")
                    .unwrap_or_else(|| panic!("width {width}: the title is missing: {line:?}"));
                display_width(before)
            };
            assert_eq!(
                title_at(long),
                title_at(short),
                "width {width}: the titles move"
            );
        }
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

    /// The first column of the first word `word` in the header line of a frame.
    /// The header is ASCII, but the count is in characters.
    fn header_column(text: &[String], word: &str) -> usize {
        let (before, _) = text[0]
            .split_once(word)
            .expect("the header names the column");
        before.chars().count()
    }

    /// The text of the started column of line `y`. The column stands where
    /// the header of the same frame puts `STARTED`.
    fn started_cells(text: &[String], y: usize) -> String {
        text[y]
            .chars()
            .skip(header_column(text, "STARTED"))
            .take(STARTED_WIDTH)
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

    /// The text of the time column of line `y`. The column stands where the
    /// header of the same frame puts `TIME`.
    fn time_cells(text: &[String], y: usize) -> String {
        text[y]
            .chars()
            .skip(header_column(text, "TIME"))
            .take(TIME_WIDTH)
            .collect::<String>()
            .trim_end()
            .to_owned()
    }

    #[test]
    fn an_active_run_shows_the_time_since_the_start() {
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
        assert_eq!(time_cells(&text, 2), "1h 14m");
    }

    #[test]
    fn a_finished_run_shows_its_own_duration() {
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
        assert_eq!(time_cells(&text, 4), "3m 07s");
    }

    /// A job with the state `state`. `None` times stand for a job that GitHub
    /// gives no time for.
    fn job(
        id: u64,
        name: &str,
        state: RunState,
        started: Option<&str>,
        completed: Option<&str>,
    ) -> Job {
        Job {
            id: JobId(id),
            name: name.to_owned(),
            state,
            created_at: None,
            started_at: started.map(at),
            completed_at: completed.map(at),
            url: format!("https://github.com/timmattison/tools/actions/runs/1/job/{id}"),
        }
    }

    /// The board of four runs with the first run expanded and `jobs` loaded.
    fn board_with_jobs(jobs: Vec<Job>) -> Board {
        let mut board = four_run_board();
        board.expand(now());
        board.set_jobs(RunId(1), jobs);
        board
    }

    /// The three jobs of the first run: one running, one passed, one failed.
    fn three_jobs() -> Vec<Job> {
        vec![
            job(
                11,
                "test",
                RunState::Running,
                Some("2026-10-14T11:50:10Z"),
                None,
            ),
            job(
                12,
                "build",
                RunState::Passed,
                Some("2026-10-14T11:45:40Z"),
                Some("2026-10-14T11:50:00Z"),
            ),
            job(
                13,
                "lint",
                RunState::Failed,
                Some("2026-10-14T11:46:00Z"),
                Some("2026-10-14T11:46:45Z"),
            ),
        ]
    }

    /// The expected line of a job row at 120 columns. The widths follow the
    /// shares of the 82 columns of room: 19 repository, 8 workflow name, 12
    /// branch, and 43 title. The text fills 28 columns, the job ID fills 12, and
    /// the branch and title columns are 56 blank columns.
    fn job_golden(marker: &str, text: &str, id: u64, times: &str) -> String {
        format!(
            "{marker} {text:<28} {id:>ID_WIDTH$} {blank:<56} {times}",
            blank = ""
        )
        .trim_end()
        .to_owned()
    }

    #[test]
    fn the_jobs_of_an_expanded_run_stand_below_it_in_the_run_columns() {
        let mut board = board_with_jobs(three_jobs());
        let text = lines(&render_with(120, 14, &mut board, &loaded()));
        assert_eq!(
            text[2..6],
            [
                "● timmattison/tools   CI                  1 main         Fix the thing                               11:45:30    14m 30s".to_owned(),
                job_golden("✓", "├ build", 12, "11:45:40    4m 20s"),
                job_golden("✗", "├ lint", 13, "11:46:00    45s"),
                job_golden("●", "└ test", 11, "11:50:10    9m 50s"),
            ]
            .map(|line| line.trim_end().to_owned())
        );
        assert!(text[6].starts_with("○ acme/widgets"), "{:?}", text[6]);
    }

    /// The display column of the last digit of the first `digits` in `line`.
    fn last_digit_column(line: &str, digits: &str) -> Option<usize> {
        let (before, _) = line.split_once(digits)?;
        Some(display_width(before) + display_width(digits) - 1)
    }

    #[test]
    fn each_job_id_ends_in_the_column_of_the_run_id() {
        let mut board = Board::new(Duration::from_hours(8));
        board.replace(
            repo("timmattison/tools"),
            vec![run(
                39_149_840_084,
                "timmattison/tools",
                "CI",
                "main",
                "Fix the thing",
                RunState::Running,
                "2026-10-14T11:45:30Z",
                "2026-10-14T11:59:00Z",
            )],
        );
        board.expand(now());
        board.set_jobs(
            RunId(39_149_840_084),
            vec![
                job(
                    107_851_237_796,
                    "test",
                    RunState::Running,
                    Some("2026-10-14T11:50:10Z"),
                    None,
                ),
                job(
                    107_851_237_794,
                    "build",
                    RunState::Passed,
                    Some("2026-10-14T11:45:40Z"),
                    Some("2026-10-14T11:50:00Z"),
                ),
                job(
                    107_851_237_795,
                    "lint",
                    RunState::Failed,
                    Some("2026-10-14T11:46:00Z"),
                    Some("2026-10-14T11:46:45Z"),
                ),
            ],
        );
        let text = lines(&render_with(120, 14, &mut board, &loaded()));
        let run_column = last_digit_column(&text[2], "39149840084")
            .unwrap_or_else(|| panic!("the run row holds its ID: {:?}", text[2]));
        for (y, start, id) in [
            (3, "✓ ├ build", "107851237794"),
            (4, "✗ ├ lint", "107851237795"),
            (5, "● └ test", "107851237796"),
        ] {
            assert!(text[y].starts_with(start), "job {start}: {:?}", text[y]);
            assert_eq!(
                last_digit_column(&text[y], id),
                Some(run_column),
                "job {start}: {:?}",
                text[y]
            );
        }
    }

    #[test]
    fn a_long_job_name_is_cut_and_keeps_the_job_id_and_the_time_columns() {
        let mut board = Board::new(Duration::from_hours(8));
        board.replace(
            repo("timmattison/tools"),
            vec![run(
                39_149_840_084,
                "timmattison/tools",
                "CI",
                "main",
                "Fix the thing",
                RunState::Running,
                "2026-10-14T11:45:30Z",
                "2026-10-14T11:59:00Z",
            )],
        );
        board.expand(now());
        board.set_jobs(
            RunId(39_149_840_084),
            vec![
                job(
                    107_851_237_794,
                    &"日本語のとても長いジョブの名前".repeat(4),
                    RunState::Running,
                    Some("2026-10-14T11:51:00Z"),
                    None,
                ),
                job(
                    107_851_237_795,
                    &"🎉🎊🎁".repeat(20),
                    RunState::Running,
                    Some("2026-10-14T11:52:00Z"),
                    None,
                ),
                job(
                    107_851_237_796,
                    &"café-au-lait-".repeat(10),
                    RunState::Running,
                    Some("2026-10-14T11:53:00Z"),
                    None,
                ),
            ],
        );
        let text = lines(&render_with(120, 14, &mut board, &loaded()));
        let run_column = last_digit_column(&text[2], "39149840084")
            .unwrap_or_else(|| panic!("the run row holds its ID: {:?}", text[2]));
        // The order is by name: `c` comes before `日`, and `日` before `🎉`.
        for (y, start, id, started, time) in [
            (3, "├ café-", "107851237796", "11:53:00", "7m 00s"),
            (4, "├ 日本語", "107851237794", "11:51:00", "9m 00s"),
            (5, "└ 🎉🎊🎁", "107851237795", "11:52:00", "8m 00s"),
        ] {
            let line = &text[y];
            assert!(line.contains(start), "line {y}: {line:?}");
            let (name, _) = line
                .split_once(id)
                .unwrap_or_else(|| panic!("line {y} holds its job ID: {line:?}"));
            assert!(name.trim_end().ends_with('…'), "line {y}: {line:?}");
            assert_eq!(
                last_digit_column(line, id),
                Some(run_column),
                "line {y}: {line:?}"
            );
            // The header is ASCII, thus its character count is its display column.
            for (word, cell) in [("STARTED", started), ("TIME", time)] {
                let (before, _) = line
                    .rsplit_once(cell)
                    .unwrap_or_else(|| panic!("line {y} holds {cell:?}: {line:?}"));
                assert_eq!(
                    display_width(before),
                    header_column(&text, word),
                    "line {y}: {line:?}"
                );
            }
            assert!(display_width(line) <= 120, "line {y}: {line:?}");
        }
    }

    #[test]
    fn no_row_shows_how_long_ago_a_run_or_a_job_finished() {
        let mut board = board_with_jobs(three_jobs());
        let text = lines(&render_with(120, 14, &mut board, &loaded()));
        assert!(
            !text[0].split_whitespace().any(|word| word == "FINISHED"),
            "the header names a finished column: {:?}",
            text[0]
        );
        // Line 13 is the status line. It says `updated 30s ago` and stays.
        for (y, line) in text.iter().enumerate().take(13).skip(1) {
            assert!(!line.contains("ago"), "line {y} shows an age: {line:?}");
        }
    }

    #[test]
    fn each_job_marker_has_the_color_of_its_state() {
        let mut board = board_with_jobs(three_jobs());
        let buffer = render_with(120, 14, &mut board, &loaded());
        let marks: Vec<(&str, Color)> = [3_u16, 4, 5]
            .iter()
            .map(|y| (buffer[(0, *y)].symbol(), buffer[(0, *y)].fg))
            .collect();
        assert_eq!(
            marks,
            [("✓", Color::Green), ("✗", Color::Red), ("●", Color::Yellow)]
        );
    }

    #[test]
    fn a_queued_run_with_a_job_in_progress_shows_the_running_marker() {
        let mut board = Board::new(Duration::from_hours(8));
        board.replace(
            repo("timmattison/tools"),
            vec![run(
                1,
                "timmattison/tools",
                "CI",
                "main",
                "Fix the thing",
                RunState::Queued,
                "2026-10-14T11:45:30Z",
                "2026-10-14T11:59:00Z",
            )],
        );
        board.expand(now());
        board.set_jobs(
            RunId(1),
            vec![
                job(
                    11,
                    "test",
                    RunState::Running,
                    Some("2026-10-14T11:50:10Z"),
                    None,
                ),
                job(12, "build", RunState::Queued, None, None),
            ],
        );
        let buffer = render_with(120, 14, &mut board, &loaded());
        let text = lines(&buffer);
        let row_of = |needle: &str| {
            let found = text.iter().position(|line| line.contains(needle));
            u16::try_from(found.unwrap_or_else(|| panic!("no row holds {needle:?}: {text:?}")))
                .expect("a row index fits in u16")
        };
        for (name, y) in [
            ("the run row", row_of("Fix the thing")),
            ("the running job row", row_of("test")),
        ] {
            assert_eq!(
                (buffer[(0, y)].symbol(), buffer[(0, y)].fg),
                ("●", Color::Yellow),
                "{name}"
            );
        }
    }

    #[test]
    fn no_job_row_is_reversed_when_its_run_is_selected() {
        let mut board = board_with_jobs(three_jobs());
        let buffer = render_with(120, 14, &mut board, &loaded());
        let reversed = |x: u16, y: u16| buffer[(x, y)].modifier.contains(Modifier::REVERSED);
        assert!(reversed(60, 2), "the selected run row");
        for y in [3_u16, 4, 5] {
            for x in [0_u16, 5, 60, 95, 119] {
                assert!(!reversed(x, y), "line {y} column {x}");
            }
        }
    }

    #[test]
    fn the_selected_job_row_is_reversed_across_the_full_width() {
        let mut board = board_with_jobs(three_jobs());
        board.select_next(now());
        let buffer = render_with(120, 14, &mut board, &loaded());
        let reversed = |x: u16, y: u16| buffer[(x, y)].modifier.contains(Modifier::REVERSED);
        for x in [0_u16, 5, 60, 95, 119] {
            assert!(reversed(x, 3), "the selected job row, column {x}");
            assert!(!reversed(x, 2), "the run row, column {x}");
            for y in [4_u16, 5] {
                assert!(!reversed(x, y), "line {y} column {x}");
            }
        }
    }

    #[test]
    fn an_active_job_shows_the_time_since_its_start() {
        let mut board = board_with_jobs(vec![job(
            11,
            "test",
            RunState::Running,
            Some("2026-10-14T10:45:30Z"),
            None,
        )]);
        let text = lines(&render_with(120, 14, &mut board, &loaded()));
        assert_eq!(time_cells(&text, 3), "1h 14m");
    }

    #[test]
    fn a_finished_job_shows_its_own_duration() {
        let mut board = board_with_jobs(vec![job(
            11,
            "lint",
            RunState::Failed,
            Some("2026-10-14T10:00:00Z"),
            Some("2026-10-14T10:03:07Z"),
        )]);
        let text = lines(&render_with(120, 14, &mut board, &loaded()));
        assert_eq!(time_cells(&text, 3), "3m 07s");
    }

    #[test]
    fn a_job_with_no_start_and_no_creation_shows_no_started_and_no_time() {
        let mut board = board_with_jobs(vec![
            job(11, "wait", RunState::Queued, None, None),
            job(
                12,
                "skip",
                RunState::Passed,
                None,
                Some("2026-10-14T11:55:00Z"),
            ),
        ]);
        let text = lines(&render_with(120, 14, &mut board, &loaded()));
        // The order is by name: "skip" stands above "wait".
        assert_eq!(started_cells(&text, 3), "");
        assert_eq!(time_cells(&text, 3), "");
        assert_eq!(started_cells(&text, 4), "");
        assert_eq!(time_cells(&text, 4), "");
    }

    #[test]
    fn a_waiting_job_shows_no_start_and_the_time_since_its_creation() {
        // The start time is set on purpose: the row must read the state of the
        // job, not the presence of a start time.
        let mut board = board_with_jobs(vec![Job {
            created_at: Some(at("2026-10-14T11:56:00Z")),
            ..job(
                11,
                "wait",
                RunState::Queued,
                Some("2026-10-14T11:58:00Z"),
                None,
            )
        }]);
        let text = lines(&render_with(120, 14, &mut board, &loaded()));
        assert_eq!(started_cells(&text, 3), "");
        assert_eq!(time_cells(&text, 3), "4m 00s");
    }

    #[test]
    fn a_running_job_counts_its_time_from_its_start_not_its_creation() {
        let mut board = board_with_jobs(vec![Job {
            created_at: Some(at("2026-10-14T11:40:00Z")),
            ..job(
                11,
                "test",
                RunState::Running,
                Some("2026-10-14T11:50:00Z"),
                None,
            )
        }]);
        let text = lines(&render_with(120, 14, &mut board, &loaded()));
        assert_eq!(started_cells(&text, 3), "11:50:00");
        assert_eq!(time_cells(&text, 3), "10m 00s");
    }

    #[test]
    fn an_expanded_run_says_loading_jobs_until_the_answer_comes() {
        let mut board = four_run_board();
        board.expand(now());
        let text = lines(&render_with(120, 14, &mut board, &loaded()));
        assert_eq!(text[3], "  └ Loading jobs…");
        assert!(text[4].starts_with("○ acme/widgets"), "{:?}", text[4]);
    }

    #[test]
    fn an_expanded_run_with_no_jobs_says_so() {
        let mut board = board_with_jobs(Vec::new());
        let text = lines(&render_with(120, 14, &mut board, &loaded()));
        assert_eq!(text[3], "  └ No jobs yet");
    }

    #[test]
    fn a_failed_fetch_shows_the_error_below_the_last_good_jobs() {
        let mut board = board_with_jobs(three_jobs());
        board.fail_jobs(RunId(1), "HTTP 502".to_owned());
        let text = lines(&render_with(120, 14, &mut board, &loaded()));
        assert!(text[3].starts_with("✓ ├ build"), "{:?}", text[3]);
        assert!(text[4].starts_with("✗ ├ lint"), "{:?}", text[4]);
        assert!(text[5].starts_with("● ├ test"), "{:?}", text[5]);
        assert_eq!(text[6], "  └ ⚠ HTTP 502");
    }

    #[test]
    fn a_failed_fetch_before_any_answer_shows_the_error_only() {
        let mut board = four_run_board();
        board.expand(now());
        board.fail_jobs(RunId(1), "HTTP 404".to_owned());
        let text = lines(&render_with(120, 14, &mut board, &loaded()));
        assert_eq!(text[3], "  └ ⚠ HTTP 404");
    }

    #[test]
    fn a_long_error_is_cut_by_display_width_to_the_line() {
        let mut board = four_run_board();
        board.expand(now());
        board.fail_jobs(RunId(1), "日本語のエラー".repeat(30));
        let text = lines(&render_with(40, 14, &mut board, &loaded()));
        assert!(text[3].starts_with("  └ ⚠ 日本語"), "{:?}", text[3]);
        assert!(text[3].ends_with('…'), "{:?}", text[3]);
        assert!(display_width(&text[3]) <= 40, "{:?}", text[3]);
        assert!(display_width(&text[3]) >= 39, "{:?}", text[3]);
    }

    #[test]
    fn a_status_row_is_never_reversed_and_has_no_marker_color() {
        let mut board = four_run_board();
        board.expand(now());
        let buffer = render_with(120, 14, &mut board, &loaded());
        for x in [0_u16, 5, 60, 119] {
            assert!(!buffer[(x, 3)].modifier.contains(Modifier::REVERSED), "{x}");
        }
        assert_eq!(buffer[(0, 3)].fg, Color::Reset);
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
        assert_eq!(time_cells(&text, 2), "0s");
        assert_eq!(time_cells(&text, 4), "0s");
    }

    fn display_width(text: &str) -> usize {
        unicode_width::UnicodeWidthStr::width(text)
    }

    /// The (branch, title) widths that the board gave before issue #571, at
    /// each width from 80 to 200 columns. The entry for width `w` is at index
    /// `w - 80`. A change to the layout must never go below these values,
    /// except that the branch gets at most 12 columns (issue #577).
    const TODAY_BRANCH_AND_TITLE: [(usize, usize); 121] = [
        (7, 15),
        (7, 16),
        (8, 14),
        (8, 15),
        (8, 16),
        (8, 17),
        (8, 17),
        (9, 16),
        (9, 17),
        (9, 18),
        (9, 18),
        (9, 19),
        (10, 18),
        (10, 19),
        (10, 19),
        (10, 20),
        (10, 21),
        (11, 20),
        (11, 20),
        (11, 21),
        (11, 22),
        (11, 23),
        (12, 21),
        (12, 22),
        (12, 23),
        (12, 24),
        (12, 24),
        (13, 23),
        (13, 24),
        (13, 25),
        (13, 25),
        (13, 26),
        (14, 25),
        (14, 26),
        (14, 26),
        (14, 27),
        (14, 28),
        (15, 27),
        (15, 27),
        (15, 28),
        (15, 29),
        (15, 30),
        (16, 28),
        (16, 29),
        (16, 30),
        (16, 31),
        (16, 31),
        (17, 30),
        (17, 31),
        (17, 32),
        (17, 32),
        (17, 33),
        (18, 32),
        (18, 33),
        (18, 33),
        (18, 34),
        (18, 35),
        (19, 34),
        (19, 34),
        (19, 35),
        (19, 36),
        (19, 37),
        (20, 35),
        (20, 36),
        (20, 37),
        (20, 38),
        (20, 38),
        (21, 37),
        (21, 38),
        (21, 39),
        (21, 39),
        (21, 40),
        (22, 39),
        (22, 40),
        (22, 40),
        (22, 41),
        (22, 42),
        (23, 41),
        (23, 41),
        (23, 42),
        (23, 43),
        (23, 44),
        (24, 42),
        (24, 43),
        (24, 44),
        (24, 45),
        (24, 45),
        (25, 44),
        (25, 45),
        (25, 46),
        (25, 46),
        (25, 47),
        (26, 46),
        (26, 47),
        (26, 47),
        (26, 48),
        (26, 49),
        (27, 48),
        (27, 48),
        (27, 49),
        (27, 50),
        (27, 51),
        (28, 49),
        (28, 50),
        (28, 51),
        (28, 52),
        (28, 52),
        (29, 51),
        (29, 52),
        (29, 53),
        (29, 53),
        (29, 54),
        (30, 53),
        (30, 54),
        (30, 54),
        (30, 55),
        (30, 56),
        (31, 55),
        (31, 55),
        (31, 56),
        (31, 57),
    ];

    #[test]
    fn the_title_keeps_its_old_width_and_the_branch_keeps_it_up_to_twelve_columns() {
        for w in 80_u16..=200 {
            let mut board = Board::new(Duration::from_hours(8));
            board.replace(
                repo("o/r"),
                vec![run(
                    1,
                    "o/r",
                    "w",
                    &"B".repeat(400),
                    &"T".repeat(400),
                    RunState::Running,
                    "2026-10-14T11:45:30Z",
                    "2026-10-14T11:45:30Z",
                )],
            );
            board.expand(now());
            board.set_jobs(RunId(1), three_jobs());
            let text = lines(&render_with(w, 14, &mut board, &loaded()));

            // The header names BRANCH and TITLE, so the search skips it.
            let row = text
                .iter()
                .skip(1)
                .find(|line| line.contains('B'))
                .expect("the frame holds the run row");
            let branch = row.chars().filter(|c| *c == 'B').count() + 1;
            let title = row.chars().filter(|c| *c == 'T').count() + 1;
            let (old_branch, old_title) = TODAY_BRANCH_AND_TITLE[usize::from(w) - 80];
            assert!(
                branch >= old_branch.min(12),
                "width {w}: the branch is {branch} wide, the old width is {old_branch}"
            );
            assert!(
                branch <= 12,
                "width {w}: the branch is {branch} wide, more than 12 columns"
            );
            assert!(
                title >= old_title,
                "width {w}: the title is {title} wide, the old width is {old_title}"
            );
            for line in &text {
                assert!(
                    display_width(line) <= usize::from(w),
                    "width {w}: the line is too wide: {line:?}"
                );
            }
        }
    }

    /// The (repository, workflow, branch, title) widths at each frame width
    /// from 80 to 200 columns. The entry for width `w` is at index `w - 80`.
    /// The workflow name and the branch have the shares that they had before
    /// issue #577, but the workflow name is never wider than the header
    /// `WORKFLOW`, which is 8 columns, and the branch is never wider than 12
    /// columns. The title gets each column that the two give up.
    const TEXT_WIDTHS: [(usize, usize, usize, usize); 121] = [
        (10, 7, 8, 17),
        (10, 8, 9, 16),
        (10, 8, 9, 17),
        (10, 8, 9, 18),
        (11, 8, 9, 18),
        (11, 8, 9, 19),
        (11, 8, 10, 19),
        (11, 8, 10, 20),
        (12, 8, 10, 20),
        (12, 8, 10, 21),
        (12, 8, 10, 22),
        (12, 8, 11, 22),
        (12, 8, 11, 23),
        (13, 8, 11, 23),
        (13, 8, 11, 24),
        (13, 8, 11, 25),
        (13, 8, 12, 25),
        (14, 8, 12, 25),
        (14, 8, 12, 26),
        (14, 8, 12, 27),
        (14, 8, 12, 28),
        (15, 8, 12, 28),
        (15, 8, 12, 29),
        (15, 8, 12, 30),
        (15, 8, 12, 31),
        (16, 8, 12, 31),
        (16, 8, 12, 32),
        (16, 8, 12, 33),
        (16, 8, 12, 34),
        (17, 8, 12, 34),
        (17, 8, 12, 35),
        (17, 8, 12, 36),
        (17, 8, 12, 37),
        (18, 8, 12, 37),
        (18, 8, 12, 38),
        (18, 8, 12, 39),
        (18, 8, 12, 40),
        (18, 8, 12, 41),
        (19, 8, 12, 41),
        (19, 8, 12, 42),
        (19, 8, 12, 43),
        (19, 8, 12, 44),
        (20, 8, 12, 44),
        (20, 8, 12, 45),
        (20, 8, 12, 46),
        (20, 8, 12, 47),
        (21, 8, 12, 47),
        (21, 8, 12, 48),
        (21, 8, 12, 49),
        (21, 8, 12, 50),
        (22, 8, 12, 50),
        (22, 8, 12, 51),
        (22, 8, 12, 52),
        (22, 8, 12, 53),
        (23, 8, 12, 53),
        (23, 8, 12, 54),
        (23, 8, 12, 55),
        (23, 8, 12, 56),
        (24, 8, 12, 56),
        (24, 8, 12, 57),
        (24, 8, 12, 58),
        (24, 8, 12, 59),
        (24, 8, 12, 60),
        (25, 8, 12, 60),
        (25, 8, 12, 61),
        (25, 8, 12, 62),
        (25, 8, 12, 63),
        (26, 8, 12, 63),
        (26, 8, 12, 64),
        (26, 8, 12, 65),
        (26, 8, 12, 66),
        (27, 8, 12, 66),
        (27, 8, 12, 67),
        (27, 8, 12, 68),
        (27, 8, 12, 69),
        (28, 8, 12, 69),
        (28, 8, 12, 70),
        (28, 8, 12, 71),
        (28, 8, 12, 72),
        (29, 8, 12, 72),
        (29, 8, 12, 73),
        (29, 8, 12, 74),
        (29, 8, 12, 75),
        (30, 8, 12, 75),
        (30, 8, 12, 76),
        (30, 8, 12, 77),
        (30, 8, 12, 78),
        (30, 8, 12, 79),
        (31, 8, 12, 79),
        (31, 8, 12, 80),
        (31, 8, 12, 81),
        (31, 8, 12, 82),
        (32, 8, 12, 82),
        (32, 8, 12, 83),
        (32, 8, 12, 84),
        (32, 8, 12, 85),
        (33, 8, 12, 85),
        (33, 8, 12, 86),
        (33, 8, 12, 87),
        (33, 8, 12, 88),
        (34, 8, 12, 88),
        (34, 8, 12, 89),
        (34, 8, 12, 90),
        (34, 8, 12, 91),
        (35, 8, 12, 91),
        (35, 8, 12, 92),
        (35, 8, 12, 93),
        (35, 8, 12, 94),
        (36, 8, 12, 94),
        (36, 8, 12, 95),
        (36, 8, 12, 96),
        (36, 8, 12, 97),
        (36, 8, 12, 98),
        (37, 8, 12, 98),
        (37, 8, 12, 99),
        (37, 8, 12, 100),
        (37, 8, 12, 101),
        (38, 8, 12, 101),
        (38, 8, 12, 102),
        (38, 8, 12, 103),
        (38, 8, 12, 104),
    ];

    #[test]
    fn the_workflow_and_the_branch_are_capped_and_the_title_gets_the_rest() {
        for w in 80_u16..=200 {
            let repo_text = format!("Q/{}", "Q".repeat(400));
            let mut board = Board::new(Duration::from_hours(8));
            board.replace(
                repo(&repo_text),
                vec![run(
                    1,
                    &repo_text,
                    &"W".repeat(400),
                    &"B".repeat(400),
                    &"T".repeat(400),
                    RunState::Running,
                    "2026-10-14T11:45:30Z",
                    "2026-10-14T11:45:30Z",
                )],
            );
            let text = lines(&render_with(w, 6, &mut board, &loaded()));
            let row = &text[2];
            assert!(
                row.starts_with('●'),
                "width {w}: line 2 is not the run row: {row:?}"
            );
            // Each text is too long for its cell, thus the cell holds the letters and `…`.
            let letters = |letter: char| row.chars().filter(|c| *c == letter).count();
            // The repository cell also holds the slash.
            let widths = (
                letters('Q') + 2,
                letters('W') + 1,
                letters('B') + 1,
                letters('T') + 1,
            );
            assert_eq!(widths, TEXT_WIDTHS[usize::from(w) - 80], "width {w}");
        }
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
        let text = lines(&render_with(68, 6, &mut board, &Status::default()));
        // 68 - 38 = 30 columns for the text: 7, 5, 6, and 12.
        assert_eq!(
            text[2],
            "● timmat… Cont…            1 featu… A very long… 11:55:00    5m 00s"
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
        let text = lines(&render_with(68, 7, &mut board, &Status::default()));
        assert_eq!(
            text[2],
            "● a/x     CI               1 日本…  t            11:51:00    9m 00s"
        );
        assert_eq!(
            text[3],
            "● a/x     CI               2 café-… t            11:52:00    8m 00s"
        );
        assert_eq!(
            text[4],
            "● a/x     CI               3 🎉🎉…  t            11:53:00    7m 00s"
        );
        for line in &text {
            assert!(display_width(line) <= 68, "{line:?} is too wide");
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
        // The header ends with `TIME`, which is 3 columns short of its cell.
        let short = TIME_WIDTH - "TIME".len();
        assert_eq!(display_width(&narrow[0]), 80 - short, "the header fills 80");
        assert_eq!(display_width(&wide[0]), 160 - short, "the header fills 160");
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
            "loading…  ·  ↑↓ select  →← jobs  ⏎ open  c cancel  r refresh  q quit"
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
            "updated 30s ago      ·  ↑↓ select  →← jobs  ⏎ open  c cancel  r refresh  q quit"
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
            "updated 30s ago      ·  API 4821/5000  ·  ↑↓ select  →← jobs  ⏎ open  c cancel  r refresh  q quit"
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
            "updated 30s ago      ·  API low 412/5000, next refresh 12:34  ·  ↑↓ select  →← jobs  ⏎ open  c cancel  r refresh  q quit"
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

    #[test]
    fn the_status_line_names_the_cancel_key_between_open_and_refresh() {
        let line = status_line(140, &Status::default());
        assert!(
            line.ends_with("⏎ open  c cancel  r refresh  q quit"),
            "{line}"
        );
    }

    /// A cancel that waits, as `c` makes it over the run `run_id`.
    fn confirm(title: &str, job: Option<&str>) -> Confirm {
        Confirm {
            repo: repo("timmattison/tools"),
            run: RunId(39_149_840_084),
            workflow: "CI".to_owned(),
            branch: "main".to_owned(),
            title: title.to_owned(),
            job: job.map(str::to_owned),
        }
    }

    /// The confirm of the mock of issue 578: a job row is selected.
    fn job_confirm() -> Confirm {
        confirm("fix(ha): show the run ID", Some("build (ubuntu-latest)"))
    }

    /// Draws one frame in UTC with the prompt `prompt` over a loaded board.
    fn render_prompt(width: u16, height: u16, prompt: &Confirm) -> Buffer {
        let mut board = four_run_board();
        let mut terminal =
            Terminal::new(TestBackend::new(width, height)).expect("a test terminal opens");
        terminal
            .draw(|frame| draw(frame, &mut board, &loaded(), now(), &utc(), Some(prompt)))
            .expect("a test frame draws");
        terminal.backend().buffer().clone()
    }

    /// The `count` columns from column `x` of the rows `first..first + rows`.
    /// The text holds no wide glyph.
    fn box_rows(buffer: &Buffer, x: usize, count: usize, first: usize, rows: usize) -> Vec<String> {
        lines(buffer)
            .into_iter()
            .skip(first)
            .take(rows)
            .map(|line| line.chars().skip(x).take(count).collect())
            .collect()
    }

    #[test]
    fn the_box_over_a_job_row_matches_the_mock_of_issue_578() {
        let buffer = render_prompt(80, 16, &job_confirm());
        // 10 rows, centered in the rows 1 to 14: the top row is row 3.
        assert_eq!(
            box_rows(&buffer, 18, 44, 3, 10),
            vec![
                "┌ Cancel run? ─────────────────────────────┐",
                "│ timmattison/tools                        │",
                "│ CI 39149840084 on main                   │",
                "│ fix(ha): show the run ID                 │",
                "│ job: build (ubuntu-latest)               │",
                "│ GitHub cannot cancel one job. y cancels  │",
                "│ every job of this run.                   │",
                "│                                          │",
                "│ y cancel the run   any other key: keep   │",
                "└──────────────────────────────────────────┘",
            ]
        );
        let all = lines(&buffer);
        assert!(!all[2].contains('┌'), "{:?}", all[2]);
        assert!(!all[13].contains('└'), "{:?}", all[13]);
    }

    #[test]
    fn the_box_over_a_run_row_has_no_job_line_and_no_note() {
        let buffer = render_prompt(80, 16, &confirm("fix(ha): show the run ID", None));
        // 7 rows, centered in the rows 1 to 14: the top row is row 4.
        assert_eq!(
            box_rows(&buffer, 18, 44, 4, 7),
            vec![
                "┌ Cancel run? ─────────────────────────────┐",
                "│ timmattison/tools                        │",
                "│ CI 39149840084 on main                   │",
                "│ fix(ha): show the run ID                 │",
                "│                                          │",
                "│ y cancel the run   any other key: keep   │",
                "└──────────────────────────────────────────┘",
            ]
        );
        let text = lines(&buffer).join("\n");
        assert!(!text.contains("job:"));
        assert!(!text.contains("GitHub cannot cancel one job"));
    }

    #[test]
    fn a_branch_that_is_empty_leaves_the_branch_out_of_the_run_line() {
        let mut prompt = confirm("title", None);
        prompt.branch = String::new();
        let buffer = render_prompt(80, 16, &prompt);
        assert_eq!(
            box_rows(&buffer, 18, 44, 6, 1),
            vec!["│ CI 39149840084                           │"]
        );
    }

    /// The glyph of the cell at (`x`, `y`).
    fn cell(buffer: &Buffer, x: u16, y: u16) -> String {
        buffer[(x, y)].symbol().to_owned()
    }

    #[test]
    fn wide_glyphs_at_the_wrap_edge_do_not_break_the_border() {
        let title = "日本語日本語日本語日本語日本語日本語日本語日本語日本語日本語日本語 \
                     🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉 café café café café café café café \
                     日本語 日本語 日本語 日本語 日本語 日本語 日本語 日本語 🎉";
        let buffer = render_prompt(80, 24, &confirm(title, Some("日本語 🎉 café")));
        let tops: Vec<u16> = (0..24).filter(|&y| cell(&buffer, 18, y) == "┌").collect();
        let bottoms: Vec<u16> = (0..24).filter(|&y| cell(&buffer, 18, y) == "└").collect();
        assert_eq!((tops.len(), bottoms.len()), (1, 1));
        let (top, bottom) = (tops[0], bottoms[0]);
        assert!(bottom - top >= 8, "the title wraps onto several rows");
        assert_eq!(
            (cell(&buffer, 61, top), cell(&buffer, 61, bottom)),
            ("┐".to_owned(), "┘".to_owned())
        );
        for y in top + 1..bottom {
            assert_eq!(cell(&buffer, 18, y), "│", "left border, row {y}");
            assert_eq!(cell(&buffer, 61, y), "│", "right border, row {y}");
        }
    }

    #[test]
    fn a_low_terminal_drops_the_parts_and_keeps_the_key_line() {
        let prompt = confirm("fix(ha): show the run ID", None);
        // 4 board rows: the blank line, the title, and the run line go.
        assert_eq!(
            box_rows(&render_prompt(80, 6, &prompt), 18, 44, 1, 4),
            vec![
                "┌ Cancel run? ─────────────────────────────┐",
                "│ timmattison/tools                        │",
                "│ y cancel the run   any other key: keep   │",
                "└──────────────────────────────────────────┘",
            ]
        );
        // 1 board row: the box takes all 3 rows of the frame.
        assert_eq!(
            box_rows(&render_prompt(80, 3, &prompt), 18, 44, 0, 3),
            vec![
                "┌ Cancel run? ─────────────────────────────┐",
                "│ y cancel the run   any other key: keep   │",
                "└──────────────────────────────────────────┘",
            ]
        );
    }

    #[test]
    fn a_box_one_row_too_tall_drops_the_empty_line_and_keeps_the_title() {
        let prompt = confirm("fix(ha): show the run ID", None);
        // 6 board rows: room for 4 text lines, and the box needs 5. The empty line goes.
        assert_eq!(
            box_rows(&render_prompt(80, 8, &prompt), 18, 44, 1, 6),
            vec![
                "┌ Cancel run? ─────────────────────────────┐",
                "│ timmattison/tools                        │",
                "│ CI 39149840084 on main                   │",
                "│ fix(ha): show the run ID                 │",
                "│ y cancel the run   any other key: keep   │",
                "└──────────────────────────────────────────┘",
            ]
        );
    }

    #[test]
    fn a_frame_too_low_for_a_border_shows_the_key_line_alone() {
        let prompt = confirm("fix(ha): show the run ID", Some("build"));
        let key_line = "y cancel the run   any other key: keep";
        let two = render_prompt(80, 2, &prompt);
        // The box clears 44 columns of row 0. The status line stays on row 1.
        assert_eq!(box_rows(&two, 18, 44, 0, 1)[0].trim_end(), key_line);
        assert!(lines(&two)[1].starts_with("updated 30s ago"));
        let one = render_prompt(80, 1, &prompt);
        assert_eq!(box_rows(&one, 18, 44, 0, 1)[0].trim_end(), key_line);
    }

    #[test]
    fn the_status_line_shows_no_key_hints_while_the_box_shows() {
        let buffer = render_prompt(120, 12, &confirm("fix(ha): show the run ID", None));
        let status_line = lines(&buffer).pop().expect("the frame has a status line");
        assert!(
            status_line.starts_with("updated 30s ago"),
            "{status_line:?}"
        );
        assert!(!status_line.contains("q quit"), "{status_line:?}");
        assert!(!status_line.contains(KEY_HINTS), "{status_line:?}");
    }

    #[test]
    fn a_narrow_frame_puts_the_two_keys_on_two_lines() {
        let buffer = render_prompt(30, 10, &confirm("fix(ha): show the run ID", None));
        let text = lines(&buffer);
        let first = text
            .iter()
            .position(|line| line.contains("y cancel the run"))
            .expect("the first key line shows");
        assert!(
            text[first].starts_with("│ y cancel the run "),
            "{:?}",
            text[first]
        );
        assert!(
            text[first + 1].starts_with("│ any other key: keep "),
            "{:?}",
            text[first + 1]
        );
    }

    #[test]
    fn wrap_line_keeps_a_line_that_fits_exactly_as_it_is() {
        assert_eq!(wrap_line("a  b", 10), vec!["a  b"]);
        assert_eq!(wrap_line("", 10), vec![""]);
    }

    #[test]
    fn wrap_line_breaks_at_spaces_and_fills_each_line() {
        assert_eq!(
            wrap_line("one two three four", 9),
            vec!["one two", "three", "four"]
        );
        assert_eq!(wrap_line("one two", 7), vec!["one two"]);
    }

    #[test]
    fn wrap_line_measures_display_width_not_bytes_or_chars() {
        // Each word is 6 columns wide. Two words and a space need 13.
        assert_eq!(wrap_line("日本語 日本語", 12), vec!["日本語", "日本語"]);
        assert_eq!(wrap_line("🎉🎉 🎉🎉", 8), vec!["🎉🎉", "🎉🎉"]);
        assert_eq!(wrap_line("café café café", 9), vec!["café café", "café"]);
    }

    #[test]
    fn wrap_line_cuts_a_word_that_is_wider_than_the_width() {
        let lines = wrap_line("ab 日本語日本語日本語 cd", 7);
        assert_eq!(lines.first().map(String::as_str), Some("ab"));
        assert!(
            lines.iter().all(|line| display_width(line) <= 7),
            "{lines:?}"
        );
        assert!(lines.iter().any(|line| line.ends_with('…')), "{lines:?}");
        assert_eq!(lines.last().map(String::as_str), Some("cd"));
    }

    #[test]
    fn wrap_line_with_a_zero_width_gives_one_empty_line() {
        assert_eq!(wrap_line("a b c", 0), vec![""]);
    }

    #[test]
    fn the_box_never_panics_whatever_the_size_of_the_frame() {
        let long = "x".repeat(300);
        let prompts = [
            job_confirm(),
            confirm(&long, Some(&long)),
            confirm("", None),
        ];
        for prompt in &prompts {
            for (width, height) in [
                (1, 1),
                (3, 3),
                (10, 2),
                (44, 4),
                (200, 60),
                (5, 40),
                (80, 0),
            ] {
                let _ = render_prompt(width, height, prompt);
            }
        }
    }

    /// The status line of a frame that holds the board `board`.
    fn status_line_of(board: &mut Board, status: &Status) -> String {
        let text = lines(&render_with(140, 8, board, status));
        text[7].clone()
    }

    const HINTS_TAIL: &str = "loading…  ·  ↑↓ select  →← jobs  ⏎ open  c cancel  r refresh  q quit";

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
            "loading…  ·  ↑↓ select  →←…  refreshing…"
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
            let mut board = board_with_jobs(three_jobs());
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
