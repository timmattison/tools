//! The paint of one screen of `faulte tui`.
//!
//! [`draw`] reads the state and paints the banner, the header lines, the
//! timeline, the table or the detail view, the line of hidden rows, and the
//! line of keys. It reads no clock and no terminal: the caller gives the time
//! and the time zone in a [`ViewContext`].

use std::fmt::Display;
use std::time::{Duration, SystemTime};

use chrono::{DateTime, TimeZone, Utc};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::history::{Cell, Gap, History, Reading};
use crate::render::{self, Accounts};
use crate::tui::app::{App, Selection};

/// What holds the parts of the banner apart.
const SEPARATOR: &str = " · ";

/// The banner while the history holds no frame.
const WAITING_BANNER: &str =
    "WAITING · faulte waits for the first frame. top can take 30 s on a loaded Mac.";

/// The first word of the banner of the live view.
const LIVE: &str = "LIVE";

/// The first word of the banner of a past view.
const PAST: &str = "PAST";

/// The words that say that the view does not follow the newest frame.
const NOT_LIVE: &str = "NOT LIVE";

/// The word that marks a selected gap in the banner.
const GAP: &str = "GAP";

/// What the banner of a selected gap says before the length of the gap.
const MEASURED_NOTHING: &str = "faulte measured nothing for ";

/// What the banner of a past view says about the way back.
const END_RETURNS: &str = "End returns to the live view";

/// What the banner of the live view says before the reason of a failed read.
const LAST_READ_FAILED: &str = " · the last read failed: ";

/// The format of a clock time.
const CLOCK_FORMAT: &str = "%H:%M:%S";

/// The lines of the timeline: the faults, the swap, and the marker.
const TIMELINE_HEIGHT: u16 = 3;

/// What the view needs besides the state: the time now and the time zone.
pub struct ViewContext<'a, Tz: TimeZone> {
    /// The time now. The age of a frame is the time from the frame to this
    /// time.
    pub now: SystemTime,
    /// The time zone of every clock the view shows.
    pub tz: &'a Tz,
}

/// The parts of the screen, from the top down.
struct Areas {
    /// The banner.
    banner: Rect,
    /// The header lines.
    headers: Rect,
    /// The two rows of the timeline and the marker.
    timeline: Rect,
    /// The table, or the detail view.
    table: Rect,
    /// The line of the rows that the table hides.
    more: Rect,
    /// The line of keys.
    keys: Rect,
}

/// Cuts `area` into the parts of the screen. A part that does not fit gets
/// no lines, and a part never reaches outside `area`.
fn areas(area: Rect, header_lines: usize) -> Areas {
    let headers = u16::try_from(header_lines).unwrap_or(u16::MAX);
    let [banner, headers, timeline, table, more, keys] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(headers),
        Constraint::Length(TIMELINE_HEIGHT),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    Areas {
        banner,
        headers,
        timeline,
        table,
        more,
        keys,
    }
}

/// Paints `text` on the first line of `area`, cut to the width of `area`.
fn paint_line(frame: &mut Frame<'_>, area: Rect, text: &str) {
    let fitted = textfit::truncate_to_budget(text, usize::from(area.width));
    frame.render_widget(Paragraph::new(Line::raw(fitted)), area);
}

/// The line of keys at the bottom of the screen.
const KEYS_LINE: &str =
    "←/→ frame  [ ] minute  Home oldest  End live  w span  ↑/↓ row  Enter detail  q quit";

/// Gives the clock time of `at` in the time zone `tz`, for example
/// `14:23:05`.
fn clock<Tz: TimeZone>(at: SystemTime, tz: &Tz) -> String
where
    Tz::Offset: Display,
{
    DateTime::<Utc>::from(at)
        .with_timezone(tz)
        .format(CLOCK_FORMAT)
        .to_string()
}

/// Gives the time from `at` to `now`, for example `20m 0s`. A time after
/// `now` gives `0s`, because the clock of this Mac can move back.
fn age(at: SystemTime, now: SystemTime) -> String {
    occ::format_uptime(now.duration_since(at).map_or(0, |passed| passed.as_secs()))
}

/// Gives what the history holds, for example `history holds 2m 0s (60 frames,
/// 1 gaps)`.
fn history_text(history: &History) -> String {
    let extent = history.extent();
    let span = match (extent.oldest_at, extent.newest_at) {
        (Some(oldest), Some(newest)) => newest.duration_since(oldest).unwrap_or_default(),
        _ => Duration::ZERO,
    };
    format!(
        "history holds {} ({})",
        occ::format_uptime(span.as_secs()),
        render::frames_and_gaps(extent.frames, extent.gaps),
    )
}

/// Paints `lines` from the top of `area`, one for each line of `area`. Each
/// line is cut to the width of `area`.
fn paint_lines(frame: &mut Frame<'_>, area: Rect, lines: &[String]) {
    for (offset, text) in lines.iter().enumerate().take(usize::from(area.height)) {
        let Ok(offset) = u16::try_from(offset) else {
            return;
        };
        paint_line(
            frame,
            Rect {
                y: area.y.saturating_add(offset),
                height: 1,
                ..area
            },
            text,
        );
    }
}

/// The place of the session column in [`render::COLUMNS`].
const SESSION_COLUMN: usize = 7;

/// The place of the state column in [`render::COLUMNS`].
const STATE_COLUMN: usize = 8;

/// The place of the directory column in [`render::COLUMNS`].
const DIRECTORY_COLUMN: usize = 9;

/// The columns that the view leaves out when each of their cells is empty.
const OPTIONAL_COLUMNS: [usize; 3] = [SESSION_COLUMN, STATE_COLUMN, DIRECTORY_COLUMN];

/// What stands between two columns of the table.
const COLUMN_GAP: &str = " ";

/// Which columns are right-aligned: the columns of numbers.
const RIGHT_ALIGNED: [bool; COLUMN_COUNT] = [
    true, false, true, true, true, true, false, false, false, false,
];

/// The count of columns of the table.
const COLUMN_COUNT: usize = render::COLUMNS.len();

/// The ten cells of one row of the table.
type Cells = [String; COLUMN_COUNT];

/// Gives the width of `text` on the screen, in columns.
fn display_width(text: &str) -> usize {
    Span::raw(text).width()
}

/// The character that takes the place of a control character in a cell.
const CONTROL_REPLACEMENT: char = ' ';

/// Gives `cell` with each control character replaced.
///
/// A process states its own arguments, so a command line can hold a tab or an
/// escape. The screen draws no such character, and the width of a column
/// counts it. A cell that kept one would thus move every column after it.
fn printable(cell: &str) -> String {
    cell.chars()
        .map(|character| {
            if character.is_control() {
                CONTROL_REPLACEMENT
            } else {
                character
            }
        })
        .collect()
}

/// Gives the cells of each row of `reading`. The command of an own row starts
/// with [`OWN_PREFIX`]. The age of a row is the time
/// from its start to the frame that the row comes from.
fn table_cells(reading: &Reading, accounts: &Accounts) -> Vec<Cells> {
    reading
        .rows()
        .map(|view| {
            let mut cells = render::row_cells(reading.ranking(), view.row, accounts, view.as_of)
                .map(|cell| printable(&cell));
            if view.own {
                cells[COMMAND_COLUMN].insert_str(0, OWN_PREFIX);
            }
            cells
        })
        .collect()
}

/// Gives the width of each column: the widest of its name and its cells. A
/// column of the optional ones with no text in any cell has width 0, and the
/// view leaves it out.
fn column_widths(cells: &[Cells]) -> [usize; COLUMN_COUNT] {
    let mut widths = render::COLUMNS.map(display_width);
    for row in cells {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(display_width(cell));
        }
    }
    for (place, width) in widths.iter_mut().enumerate() {
        if OPTIONAL_COLUMNS.contains(&place) && cells.iter().all(|row| row[place].is_empty()) {
            *width = 0;
        }
    }
    widths
}

/// The place of the command column in [`render::COLUMNS`].
const COMMAND_COLUMN: usize = 6;

/// What stands before the command of `faulte` and of its children, so that a
/// reader sees that the tool itself made those faults.
const OWN_PREFIX: &str = "[faulte] ";

/// The columns whose width never shrinks: the numbers and the state.
const FIXED_COLUMNS: [usize; 7] = [0, 1, 2, 3, 4, 5, STATE_COLUMN];

/// The part of the free width that the session column can take, as a divisor.
const SESSION_SHARE: usize = 5;

/// The part of the free width that the directory column can take, as a
/// divisor.
const DIRECTORY_SHARE: usize = 4;

/// Gives the widths that fit in `total` columns, from the widths that the
/// content wants.
///
/// The fixed columns keep their width. Let `free` be what remains when the
/// fixed columns and the gaps between the columns are taken away. The session
/// takes at most a fifth of it, the directory at most a quarter, and the
/// command takes what is left, so the command is the column that gives way.
fn fit_widths(wanted: [usize; COLUMN_COUNT], total: usize) -> [usize; COLUMN_COUNT] {
    let visible = wanted.iter().filter(|width| **width > 0).count();
    let gaps = visible.saturating_sub(1) * display_width(COLUMN_GAP);
    let fixed: usize = FIXED_COLUMNS.iter().map(|place| wanted[*place]).sum();
    let free = total.saturating_sub(fixed + gaps);
    let mut widths = wanted;
    widths[SESSION_COLUMN] = wanted[SESSION_COLUMN].min(free / SESSION_SHARE);
    widths[DIRECTORY_COLUMN] = wanted[DIRECTORY_COLUMN].min(free / DIRECTORY_SHARE);
    let left = free - widths[SESSION_COLUMN] - widths[DIRECTORY_COLUMN];
    widths[COMMAND_COLUMN] = wanted[COMMAND_COLUMN].min(left);
    widths
}

/// Pads `text` with spaces to `width` columns of the screen, on the left when
/// `right` is true and on the right when it is false.
fn pad(text: &str, width: usize, right: bool) -> String {
    let fill = " ".repeat(width.saturating_sub(display_width(text)));
    if right {
        format!("{fill}{text}")
    } else {
        format!("{text}{fill}")
    }
}

/// Joins the cells of one row into the text of one line.
fn table_line<'a>(cells: impl IntoIterator<Item = &'a str>, widths: &[usize]) -> String {
    cells
        .into_iter()
        .zip(widths)
        .zip(RIGHT_ALIGNED)
        .filter(|((_, width), _)| **width > 0)
        .map(|((cell, width), right)| {
            pad(&textfit::truncate_to_budget(cell, *width), *width, right)
        })
        .collect::<Vec<_>>()
        .join(COLUMN_GAP)
}

/// The line of the detail view for a process that left the history.
const NO_SERIES: &str = "the history no longer holds this process";

/// Gives the clock time of `at`, or [`render::ABSENT`] when the process made
/// no fault in the history.
fn optional_clock<Tz: TimeZone>(at: Option<SystemTime>, tz: &Tz) -> String
where
    Tz::Offset: Display,
{
    at.map_or_else(|| render::ABSENT.to_string(), |at| clock(at, tz))
}

/// Paints the detail view of the selected process in `area`: its facts, its
/// own timeline, its session and directory, and then its whole command, which
/// wraps and scrolls.
fn paint_detail<Tz: TimeZone>(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &mut App,
    context: &ViewContext<'_, Tz>,
) where
    Tz::Offset: Display,
{
    let history = app.history();
    let cells = usize::from(area.width).saturating_sub(LABEL_WIDTH);
    let Some(series) = app
        .row_key()
        .and_then(|key| history.series(key, cells, app.selected_entry()))
    else {
        paint_line(frame, area, NO_SERIES);
        return;
    };
    let mut lines = vec![
        Line::raw(format!(
            "PID {} · OWNER {} · FIRST {} · LAST {} · PEAK {}/s · {} frames with faults",
            series.key.pid,
            history.accounts().name_of(series.uid),
            optional_clock(series.first_at, context.tz),
            optional_clock(series.last_at, context.tz),
            render::rate(series.peak_rate),
            series.frames_with_faults,
        )),
        cell_line(FAULTS_LABEL, &series.cells),
        marker_line(series.marker),
    ];
    if let Some(session) = &series.session {
        lines.push(Line::raw(format!("{SESSION_LABEL} {session}")));
    }
    if let Some(directory) = &series.directory {
        lines.push(Line::raw(format!("{DIRECTORY_LABEL} {directory}")));
    }
    let facts = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let [facts_area, command_area] =
        Layout::vertical([Constraint::Length(facts), Constraint::Min(0)]).areas(area);
    let command = series.command;
    frame.render_widget(Paragraph::new(lines), facts_area);
    let paragraph = Paragraph::new(command).wrap(Wrap { trim: false });
    let wrapped = paragraph.line_count(command_area.width);
    app.set_detail_room(
        command_area.height,
        u16::try_from(wrapped).unwrap_or(u16::MAX),
    );
    frame.render_widget(paragraph.scroll((app.detail_scroll(), 0)), command_area);
}

/// The label of the session line of the detail view.
const SESSION_LABEL: &str = "SESSION";

/// The label of the directory line of the detail view.
const DIRECTORY_LABEL: &str = "DIRECTORY";

/// Paints the header of the table and the rows that fit in `area`.
fn paint_table<Tz: TimeZone>(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &mut App,
    context: &ViewContext<'_, Tz>,
) where
    Tz::Offset: Display,
{
    let page = usize::from(area.height.saturating_sub(1));
    app.set_page_rows(page);
    if let Some(gap) = past_gap(app) {
        paint_line(frame, area, &gap.reason);
        return;
    }
    if app.detail_open() {
        paint_detail(frame, area, app, context);
        return;
    }
    let Some(reading) = app.reading() else {
        return;
    };
    let cells = table_cells(reading, app.history().accounts());
    let widths = fit_widths(column_widths(&cells), usize::from(area.width));
    let mut lines = vec![Line::raw(table_line(render::COLUMNS, &widths))];
    let selected = app.selected_row();
    let top = app.top_row();
    lines.extend(
        cells
            .iter()
            .enumerate()
            .skip(top)
            .take(page)
            .map(|(place, row)| {
                let line = Line::raw(table_line(row.iter().map(String::as_str), &widths));
                if selected == Some(place) {
                    line.style(Style::new().reversed())
                } else {
                    line
                }
            }),
    );
    frame.render_widget(Paragraph::new(lines), area);
}

/// The label of the row of the fault rates in the timeline. It is as wide as
/// [`SWAP_LABEL`].
const FAULTS_LABEL: &str = "FAULTS ";

/// The label of the row of the swap rates in the timeline.
const SWAP_LABEL: &str = "SWAP   ";

/// The width of the labels of the timeline, in columns.
const LABEL_WIDTH: usize = 7;

/// The bars of the timeline, from the lowest to the highest.
const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// The glyph that marks the selected cell.
const MARKER_GLYPH: char = '▲';

/// The glyph of a cell that holds gaps only.
const GAP_GLYPH: char = '╳';

/// The glyph of a cell with nothing to show.
const BLANK_GLYPH: char = ' ';

/// Gives the glyph and the style of `cell`. A bar that holds a gap is yellow,
/// so a reader sees that the slice is not whole.
fn cell_span(cell: Cell) -> Span<'static> {
    match cell {
        Cell::Blank => Span::raw(BLANK_GLYPH.to_string()),
        Cell::Gap => Span::raw(GAP_GLYPH.to_string()),
        Cell::Bar { level, holds_gap } => {
            let top = BARS.len() - 1;
            let glyph = BARS[usize::from(level).min(top)].to_string();
            if holds_gap {
                Span::styled(glyph, Style::new().yellow())
            } else {
                Span::raw(glyph)
            }
        }
    }
}

/// Gives a line of the timeline: `label` and then one glyph for each cell.
fn cell_line(label: &'static str, cells: &[Cell]) -> Line<'static> {
    let mut spans = vec![Span::raw(label)];
    spans.extend(cells.iter().copied().map(cell_span));
    Line::from(spans)
}

/// Gives the line that holds the marker under `cell`, or an empty line when
/// no cell is selected.
fn marker_line(cell: Option<usize>) -> Line<'static> {
    match cell {
        Some(cell) => Line::raw(format!(
            "{}{MARKER_GLYPH}",
            " ".repeat(LABEL_WIDTH.saturating_add(cell))
        )),
        None => Line::default(),
    }
}

/// Paints the timeline of the history in `area`.
fn paint_timeline(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let cells = usize::from(area.width).saturating_sub(LABEL_WIDTH);
    let timeline = app.history().timeline(cells, app.selected_entry());
    let lines = vec![
        cell_line(FAULTS_LABEL, &timeline.faults),
        cell_line(SWAP_LABEL, &timeline.swap),
        marker_line(timeline.marker),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}

/// Gives the gap that the person selected in the past. The live view never
/// selects a gap: it shows the newest frame, even when a gap came after it.
fn past_gap(app: &App) -> Option<&Gap> {
    match app.selection() {
        Selection::Past(_) => app.selected_gap(),
        Selection::Live => None,
    }
}

/// Gives the first part of a past banner: `PAST`, and then the notice of the
/// view, if any. The notice comes first, because the banner is cut at the
/// width of the screen.
fn past_label(app: &App) -> String {
    match app.notice() {
        Some(notice) => format!("{PAST}{SEPARATOR}{notice}"),
        None => PAST.to_string(),
    }
}

/// Gives the text of the banner.
fn banner<Tz: TimeZone>(app: &App, context: &ViewContext<'_, Tz>) -> String
where
    Tz::Offset: Display,
{
    let history = app.history();
    if let Some(gap) = past_gap(app) {
        return format!(
            "{}{SEPARATOR}{GAP}{SEPARATOR}{} ({} ago){SEPARATOR}{MEASURED_NOTHING}{}: {}",
            past_label(app),
            clock(gap.at, context.tz),
            age(gap.at, context.now),
            render::seconds(gap.lost),
            gap.reason,
        );
    }
    let Some(reading) = app.reading() else {
        let mut text = WAITING_BANNER.to_string();
        if let Some(gap) = history.newest().and_then(|newest| history.gap(newest)) {
            text.push_str(SEPARATOR);
            text.push_str(&gap.reason);
        }
        return text;
    };
    let frame_text = format!(
        "frame of {} ({} ago){SEPARATOR}{}",
        clock(reading.at(), context.tz),
        age(reading.at(), context.now),
        history_text(history),
    );
    if let Selection::Past(_) = app.selection() {
        return format!(
            "{}{SEPARATOR}{NOT_LIVE}{SEPARATOR}{frame_text}{SEPARATOR}{END_RETURNS}",
            past_label(app),
        );
    }
    let mut text = format!("{LIVE}{SEPARATOR}{frame_text}");
    if let Some(gap) = history.newest().and_then(|newest| history.gap(newest)) {
        text.push_str(LAST_READ_FAILED);
        text.push_str(&gap.reason);
    }
    text
}

/// Gives the style of the banner.
///
/// A past frame has a background that a live frame does not have, because a
/// person must never read an old table as live. The text of the banner says
/// the same thing, so the color is never the only sign.
fn banner_style(app: &App) -> Style {
    match app.selection() {
        Selection::Live => Style::new(),
        Selection::Past(_) if past_gap(app).is_some() => Style::new().black().on_red(),
        Selection::Past(_) => Style::new().black().on_yellow(),
    }
}

/// Paints the banner across the whole width of `area`.
fn paint_banner<Tz: TimeZone>(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &App,
    context: &ViewContext<'_, Tz>,
) where
    Tz::Offset: Display,
{
    let fitted = textfit::truncate_to_budget(&banner(app, context), usize::from(area.width));
    frame.render_widget(
        Paragraph::new(Line::raw(fitted)).style(banner_style(app)),
        area,
    );
}

/// Paints one screen.
///
/// The function tells `app` the room that the screen gave: the rows of the
/// table, and the lines of the command in the detail view. It changes nothing
/// else. A screen of any size is valid: a part with no room draws nothing.
pub fn draw<Tz: TimeZone>(frame: &mut Frame<'_>, app: &mut App, context: &ViewContext<'_, Tz>)
where
    Tz::Offset: Display,
{
    let header_lines = app.reading().map_or_else(Vec::new, Reading::header_lines);
    let parts = areas(frame.area(), header_lines.len());
    paint_banner(frame, parts.banner, app, context);
    paint_lines(frame, parts.headers, &header_lines);
    paint_timeline(frame, parts.timeline, app);
    paint_table(frame, parts.table, app, context);
    if let Some(reading) = app.reading() {
        let hidden = render::more_line(reading.ranking(), reading.other_rows(), 0);
        paint_line(frame, parts.more, hidden.as_deref().unwrap_or_default());
    }
    paint_line(frame, parts.keys, KEYS_LINE);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::Duration;

    use chrono::FixedOffset;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::{Color, Modifier};
    use ratatui::text::Span;
    use ratatui::Terminal;

    use super::*;
    use crate::history::{EntryId, Gap, History, SampledFrame, Update};
    use crate::machine::Observed;
    use crate::pid::{Pid, Uid};
    use crate::ranking::{ClaudeTotal, ClaudeView, RankedRow, Ranking, Skipped};
    use crate::render::Accounts;
    use crate::state::SessionState;
    use crate::vm::{SwapUsage, VmDelta};

    /// The window of a test frame, in seconds.
    const WINDOW_SECS: u64 = 2;

    /// The account of every test row.
    const ACCOUNT: Uid = Uid::new(501);

    /// The time of the first test frame: 2023-11-14 22:13:20 UTC.
    const BASE_SECS: u64 = 1_700_000_000;

    /// Gives the time `secs` seconds after [`BASE_SECS`].
    fn time(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(BASE_SECS + secs)
    }

    /// Makes a row of a process that is not a session.
    fn row(pid: u32, faults: u64) -> RankedRow {
        RankedRow {
            pid: Pid::new(pid),
            uid: ACCOUNT,
            faults,
            rss_kib: Some(1_000),
            started_at_epoch_secs: Some(BASE_SECS - 3_600),
            command: format!("/bin/command-{pid}"),
            claude: ClaudeView::NotClaude,
        }
    }

    /// Makes the update that holds one sample at `secs`.
    fn frame(secs: u64, rows: Vec<RankedRow>) -> Update {
        frame_with_own(secs, rows, &[])
    }

    /// Makes the update that holds one sample at `secs`, in which the
    /// processes `own` are `faulte` or its children.
    fn frame_with_own(secs: u64, rows: Vec<RankedRow>, own: &[u32]) -> Update {
        let window = Duration::from_secs(WINDOW_SECS);
        let total_faults = rows.iter().map(|row| row.faults).sum();
        Update::Frame(Box::new(SampledFrame {
            observed: Observed {
                ranking: Ranking {
                    rows,
                    window,
                    total_faults,
                    claude: ClaudeTotal {
                        processes: 0,
                        other_account: 0,
                        faults: 0,
                    },
                    skipped: Skipped::default(),
                },
                table: Vec::new(),
                swap: VmDelta::default(),
                usage: SwapUsage::default(),
                compressor_bytes: 0,
                swap_window: window,
                accounts: Accounts::new(),
                now: time(secs),
                interval: "2s".parse().expect("2s is a span"),
            },
            at: time(secs),
            own: own.iter().copied().map(Pid::new).collect::<BTreeSet<_>>(),
        }))
    }

    /// Makes the update that holds one gap at `secs`.
    fn gap(secs: u64, lost_secs: u64, reason: &str) -> Update {
        Update::Gap(Gap {
            at: time(secs),
            lost: Duration::from_secs(lost_secs),
            reason: reason.to_string(),
        })
    }

    /// Makes the state over the updates, and gives the name of each entry.
    fn app_with(updates: Vec<Update>) -> (crate::tui::app::App, Vec<EntryId>) {
        let mut history = History::new();
        let ids = updates.into_iter().map(|u| history.add(u)).collect();
        (crate::tui::app::App::new(history), ids)
    }

    /// Paints `app` on a screen of `width` by `height`, `now_secs` seconds
    /// after [`BASE_SECS`], in UTC.
    fn paint(app: &mut crate::tui::app::App, width: u16, height: u16, now_secs: u64) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test backend");
        let utc = FixedOffset::east_opt(0).expect("zero is an offset");
        let context = ViewContext {
            now: time(now_secs),
            tz: &utc,
        };
        terminal
            .draw(|frame| draw(frame, app, &context))
            .expect("the test backend draws");
        terminal.backend().buffer().clone()
    }

    /// Gives the text of line `y`, with each glyph once. A wide glyph is
    /// followed by cells that hold no glyph of their own, and they are
    /// skipped.
    fn line_text(buffer: &Buffer, y: u16) -> String {
        let mut text = String::new();
        let mut x = 0;
        while x < buffer.area.width {
            let symbol = buffer[(x, y)].symbol();
            text.push_str(symbol);
            x += u16::try_from(Span::raw(symbol).width().max(1)).expect("a glyph is narrow");
        }
        text.trim_end().to_string()
    }

    /// Gives every line of the screen.
    fn screen(buffer: &Buffer) -> Vec<String> {
        (0..buffer.area.height)
            .map(|y| line_text(buffer, y))
            .collect()
    }

    /// With no frame, the banner says that it waits, and gives the reason of
    /// the newest gap.
    #[test]
    fn the_banner_waits_for_the_first_frame_and_gives_the_reason() {
        let (mut app, _) = app_with(vec![gap(0, 2, "top stopped")]);

        let lines = screen(&paint(&mut app, 120, 24, 10));

        assert_eq!(
            lines[0],
            "WAITING · faulte waits for the first frame. top can take 30 s on a loaded Mac. · top stopped"
        );
    }

    /// A live view states the clock and the age of the newest frame, and what
    /// the history holds.
    #[test]
    fn the_live_banner_gives_the_clock_and_the_age() {
        let (mut app, _) = app_with(vec![frame(0, vec![row(1, 5)]), frame(2, vec![row(1, 5)])]);

        let lines = screen(&paint(&mut app, 120, 24, 11));

        assert_eq!(
            lines[0],
            "LIVE · frame of 22:13:22 (9s ago) · history holds 2s (2 frames, 0 gaps)"
        );
    }

    /// A live view with a gap at its end says why the last read failed.
    #[test]
    fn the_live_banner_gives_the_reason_of_a_gap_at_the_end() {
        let (mut app, _) = app_with(vec![frame(0, vec![row(1, 5)]), gap(2, 2, "top stopped")]);

        let lines = screen(&paint(&mut app, 120, 24, 11));

        assert_eq!(
            lines[0],
            "LIVE · frame of 22:13:20 (11s ago) · history holds 2s (1 frame, 1 gap) · the last read failed: top stopped"
        );
    }

    /// Makes a key press without modifiers.
    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// A past view says that it is not live, and gives the clock and the age
    /// of its frame.
    #[test]
    fn a_past_frame_shows_not_live_the_clock_and_the_age() {
        let (mut app, _) = app_with(vec![frame(0, vec![row(1, 5)]), frame(2, vec![row(1, 5)])]);
        app.on_key(press(KeyCode::Left));

        let lines = screen(&paint(&mut app, 140, 24, 1_200));

        assert_eq!(
            lines[0],
            "PAST · NOT LIVE · frame of 22:13:20 (20m 0s ago) · history holds 2s (2 frames, 0 gaps) · End returns to the live view"
        );
    }

    /// When the selected past frame leaves the history, the banner says that
    /// the view moved to the oldest frame, directly after `PAST`. The next key
    /// clears the note.
    #[test]
    fn the_banner_says_when_the_selected_frame_left_the_history() {
        let fill = |history: &mut History| {
            for step in 0..3 {
                history.add(frame(step * 2, vec![row(1, 1), row(2, 2)]));
            }
        };
        let mut probe = History::with_budget(usize::MAX);
        fill(&mut probe);
        let mut history = History::with_budget(probe.bytes_used());
        fill(&mut history);
        let mut app = crate::tui::app::App::new(history);
        app.on_key(press(KeyCode::Home));

        app.on_update(frame(6, vec![row(1, 1), row(2, 2)]));
        let moved = screen(&paint(&mut app, 300, 24, 10));
        app.on_key(press(KeyCode::Char('x')));
        let after_a_key = screen(&paint(&mut app, 300, 24, 10));

        assert!(
            moved[0].starts_with(
                "PAST · the selected frame left the history, so the view moved to the oldest \
                 frame · NOT LIVE · frame of "
            ),
            "{}",
            moved[0]
        );
        assert!(
            after_a_key[0].starts_with("PAST · NOT LIVE · frame of "),
            "{}",
            after_a_key[0]
        );
    }

    /// The header lines of a frame are the lines of the reading, directly
    /// under the banner.
    #[test]
    fn the_header_lines_follow_the_banner_for_a_frame() {
        let (mut app, _) = app_with(vec![frame(0, vec![row(1, 5)])]);
        let expected = app.reading().expect("a frame").header_lines();

        let lines = screen(&paint(&mut app, 160, 24, 3));

        assert!(expected[0].contains("over a 2.0 s window"), "{expected:?}");
        assert_eq!(lines[1..=expected.len()], expected[..]);
    }

    /// The header lines of a span state the span and its frames.
    #[test]
    fn the_header_lines_follow_the_banner_for_a_span() {
        let (mut app, _) = app_with(vec![frame(0, vec![row(1, 5)]), frame(2, vec![row(1, 5)])]);
        app.on_key(press(KeyCode::Char('w')));
        let expected = app.reading().expect("a span").header_lines();

        let lines = screen(&paint(&mut app, 160, 24, 3));

        assert!(expected[0].contains("the last 1 m"), "{expected:?}");
        assert_eq!(lines[1..=expected.len()], expected[..]);
    }

    /// The ID of the session of every Claude Code test row.
    const SESSION_ID: &str = "d3b0d921-f0a1-41fc-b309-c11aa30c1173";

    /// Makes a row of a busy Claude Code session with the command `command`.
    fn claude_row(pid: u32, faults: u64, command: &str) -> RankedRow {
        RankedRow {
            command: command.to_string(),
            claude: ClaudeView::Session {
                id: occ::SessionId::parse(SESSION_ID).expect("the test ID is a UUID"),
                state: SessionState::Busy,
                directory: Some("/work/project".into()),
                status_changed_at: None,
            },
            ..row(pid, faults)
        }
    }

    /// Gives the index of the first line that starts with `prefix`.
    fn line_starting(lines: &[String], prefix: &str) -> usize {
        lines
            .iter()
            .position(|line| line.starts_with(prefix))
            .unwrap_or_else(|| panic!("no line starts with {prefix:?} in {lines:#?}"))
    }

    /// Gives the index of the first line that contains `text`.
    fn line_containing(lines: &[String], text: &str) -> usize {
        lines
            .iter()
            .position(|line| line.contains(text))
            .unwrap_or_else(|| panic!("no line contains {text:?} in {lines:#?}"))
    }

    /// The table has the columns of `render::COLUMNS`, in that order.
    #[test]
    fn the_table_has_the_same_columns_in_the_same_order() {
        let (mut app, _) = app_with(vec![frame(
            0,
            vec![claude_row(10, 50, "claude"), claude_row(11, 30, "claude")],
        )]);

        let lines = screen(&paint(&mut app, 160, 24, 3));

        let header = line_starting(&lines, "PID");
        let names: Vec<&str> = lines[header].split_whitespace().collect();
        assert_eq!(names, crate::render::COLUMNS);
        assert_eq!(lines[header + 1].split_whitespace().next(), Some("10"));
        assert_eq!(lines[header + 2].split_whitespace().next(), Some("11"));
    }

    /// Makes five entries, two seconds apart, with a gap at the third.
    fn five_with_a_gap() -> crate::tui::app::App {
        app_with(vec![
            frame(0, vec![row(1, 5)]),
            frame(2, vec![row(1, 5)]),
            gap(4, 2, "top stopped"),
            frame(6, vec![row(1, 5)]),
            frame(8, vec![row(1, 5)]),
        ])
        .0
    }

    /// A gap shows as its glyph in the timeline.
    #[test]
    fn a_gap_shows_its_glyph_in_the_timeline() {
        let mut app = five_with_a_gap();

        let lines = screen(&paint(&mut app, 120, 24, 100));

        let faults: Vec<char> = lines[line_starting(&lines, "FAULTS ")]
            .chars()
            .skip(7)
            .collect();
        assert_eq!(faults.len(), 5, "{faults:?}");
        assert_eq!(faults[2], '╳');
        assert!(faults.iter().filter(|glyph| **glyph == '╳').count() == 1);
    }

    /// A selected gap says how long nothing was measured and why, and the
    /// table area gives the same reason and no rows.
    #[test]
    fn a_selected_gap_shows_its_reason() {
        let mut app = five_with_a_gap();
        app.on_key(press(KeyCode::Left));
        app.on_key(press(KeyCode::Left));

        let lines = screen(&paint(&mut app, 120, 24, 100));

        assert_eq!(
            lines[0],
            "PAST · GAP · 22:13:24 (1m 36s ago) · faulte measured nothing for 2.0 s: top stopped"
        );
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.contains("top stopped"))
                .count(),
            2
        );
        assert!(
            lines.iter().all(|line| !line.starts_with("PID")),
            "{lines:#?}"
        );
    }

    /// Gives the index of the cell of the timeline that holds the marker, or
    /// `None` when the line under the swap row holds no marker.
    fn marker_cell(lines: &[String]) -> Option<usize> {
        let swap = line_starting(lines, "SWAP ");
        lines[swap + 1]
            .chars()
            .position(|glyph| glyph == '▲')
            .map(|column| column - 7)
    }

    /// The marker stands under the cell of the selected entry.
    #[test]
    fn the_marker_stands_under_the_selected_cell() {
        let mut app = five_with_a_gap();

        assert_eq!(
            marker_cell(&screen(&paint(&mut app, 120, 24, 100))),
            Some(4)
        );

        app.on_key(press(KeyCode::Left));
        assert_eq!(
            marker_cell(&screen(&paint(&mut app, 120, 24, 100))),
            Some(3)
        );
    }

    /// Makes ten frames, with a spike of faults at the sixth.
    fn ten_with_a_spike() -> crate::tui::app::App {
        let updates = (0..10)
            .map(|index| frame(index * 2, vec![row(1, if index == 5 { 1_000 } else { 10 })]))
            .collect();
        app_with(updates).0
    }

    /// The bars are block elements, and the spike reaches the top bar.
    #[test]
    fn the_bars_are_blocks_and_a_spike_reaches_level_seven() {
        let mut app = ten_with_a_spike();

        let lines = screen(&paint(&mut app, 120, 24, 100));

        let faults: Vec<char> = lines[line_starting(&lines, "FAULTS ")]
            .chars()
            .skip(7)
            .collect();
        assert_eq!(faults.len(), 10, "{faults:?}");
        assert!(
            faults.iter().all(|glyph| ('▁'..='█').contains(glyph)),
            "{faults:?}"
        );
        assert_eq!(faults[5], '█');
        assert_eq!(faults.iter().filter(|glyph| **glyph == '█').count(), 1);
    }

    /// A spike survives a timeline that has fewer cells than entries.
    #[test]
    fn a_spike_survives_a_squeezed_timeline() {
        let mut app = ten_with_a_spike();

        let lines = screen(&paint(&mut app, 12, 24, 100));

        let faults: Vec<char> = lines[line_starting(&lines, "FAULTS ")]
            .chars()
            .skip(7)
            .collect();
        assert_eq!(faults.len(), 5, "{faults:?}");
        assert!(faults.contains(&'█'), "{faults:?}");
    }

    /// Gives the screen column where `needle` starts in `line`.
    fn screen_column(line: &str, needle: &str) -> Option<usize> {
        let (before, _) = line.split_once(needle)?;
        Some(Span::raw(before).width())
    }

    /// The state column starts at one screen column for commands of any
    /// glyph width, also when the commands do not fit.
    #[test]
    fn commands_of_any_glyph_width_keep_the_next_column_in_place() {
        let long = "x".repeat(60);
        let commands = [
            format!("claude 日本語 {long}"),
            format!("claude 🎉🎉🎉 {long}"),
            format!("claude café {long}"),
            format!("claude plain {long}"),
        ];
        let rows = commands
            .iter()
            .zip(10..)
            .map(|(command, pid)| claude_row(pid, 50, command))
            .collect();
        let (mut app, _) = app_with(vec![frame(0, rows)]);

        let lines = screen(&paint(&mut app, 100, 24, 3));

        let header = line_starting(&lines, "PID");
        let columns: Vec<Option<usize>> = lines[header + 1..header + 5]
            .iter()
            .map(|line| screen_column(line, "busy"))
            .collect();
        assert!(columns[0].is_some(), "{lines:#?}");
        assert!(
            columns.iter().all(|column| *column == columns[0]),
            "{columns:?}"
        );
        assert_eq!(columns[0], screen_column(&lines[header], "STATE"));
        assert!(lines[header + 1..header + 5]
            .iter()
            .all(|line| line.contains('…')));
    }

    /// A row of `faulte` or of its child carries a mark in its command.
    #[test]
    fn an_own_row_is_marked() {
        let (mut app, _) = app_with(vec![frame_with_own(0, vec![row(7, 50), row(8, 30)], &[7])]);

        let lines = screen(&paint(&mut app, 120, 24, 3));

        let header = line_starting(&lines, "PID");
        assert!(
            lines[header + 1].contains("[faulte] /bin/command-7"),
            "{lines:#?}"
        );
        assert!(!lines[header + 2].contains("[faulte]"), "{lines:#?}");
    }

    /// The age and the state of a row in the past are the age and the state
    /// of that frame, and not of now.
    #[test]
    fn a_past_row_states_what_was_true_in_its_frame() {
        let idle = RankedRow {
            claude: ClaudeView::Session {
                id: occ::SessionId::parse(SESSION_ID).expect("the test ID is a UUID"),
                state: SessionState::Idle {
                    for_: Some(Duration::from_secs(300)),
                },
                directory: Some("/work/project".into()),
                status_changed_at: None,
            },
            ..row(10, 50)
        };
        let (mut app, _) = app_with(vec![frame(0, vec![idle]), frame(100, vec![row(10, 50)])]);
        app.on_key(press(KeyCode::Left));

        let lines = screen(&paint(&mut app, 160, 24, 10_000));

        let header = line_starting(&lines, "PID");
        assert!(lines[header + 1].contains(" 1h 0m "), "{lines:#?}");
        assert!(lines[header + 1].contains("idle 5m 0s"), "{lines:#?}");
        assert!(!lines[header + 1].contains("2h 46m"), "{lines:#?}");
    }

    /// The detail view gives the facts of one process, its own timeline, its
    /// session, its directory, and its whole command.
    #[test]
    fn the_detail_view_gives_the_facts_of_the_selected_process() {
        let (mut app, _) = app_with(vec![
            frame(
                0,
                vec![claude_row(10, 50, "claude --resume the-whole-command")],
            ),
            frame(
                2,
                vec![claude_row(10, 20, "claude --resume the-whole-command")],
            ),
        ]);
        app.on_key(press(KeyCode::Down));
        app.on_key(press(KeyCode::Enter));

        let lines = screen(&paint(&mut app, 120, 24, 3));

        let facts = line_starting(&lines, "PID 10");
        assert_eq!(
            lines[facts],
            "PID 10 · OWNER 501 · FIRST 22:13:20 · LAST 22:13:22 · PEAK 25/s · 2 frames with faults"
        );
        assert!(lines[facts + 1].starts_with("FAULTS "), "{lines:#?}");
        assert!(lines[facts + 2].contains('▲'), "{lines:#?}");
        assert_eq!(lines[facts + 3], format!("SESSION {SESSION_ID}"));
        assert_eq!(lines[facts + 4], "DIRECTORY /work/project");
        assert_eq!(lines[facts + 5], "claude --resume the-whole-command");
        assert!(
            lines.iter().all(|line| !line.starts_with("PID  ")),
            "{lines:#?}"
        );
    }

    /// The detail view of a process that left the history says so.
    #[test]
    fn the_detail_view_says_when_the_history_lost_the_process() {
        let mut history = History::with_budget(0);
        history.add(frame(0, vec![row(10, 50)]));
        let mut app = crate::tui::app::App::new(history);
        app.on_key(press(KeyCode::Down));
        app.on_key(press(KeyCode::Enter));
        app.on_update(frame(2, vec![row(11, 50)]));

        let lines = screen(&paint(&mut app, 120, 24, 3));

        assert!(app.detail_open());
        assert!(
            lines
                .iter()
                .any(|line| line == "the history no longer holds this process"),
            "{lines:#?}"
        );
    }

    /// A count of one reads in the singular, the same as the header lines.
    #[test]
    fn the_banner_counts_one_frame_and_one_gap_in_the_singular() {
        let (mut app, _) = app_with(vec![frame(0, vec![row(1, 5)]), gap(2, 2, "top stopped")]);

        let lines = screen(&paint(&mut app, 120, 24, 11));

        assert!(lines[0].contains("(1 frame, 1 gap)"), "{lines:#?}");
    }

    /// A person must never read an old table as live, so a past banner has a
    /// background that a live banner does not have. The text says the same
    /// thing, for a person who cannot see the color.
    #[test]
    fn a_past_banner_has_a_background_and_a_live_banner_has_none() {
        let (mut app, _) = app_with(vec![
            frame(0, vec![row(1, 5)]),
            gap(2, 2, "top stopped"),
            frame(4, vec![row(1, 5)]),
        ]);

        let live = paint(&mut app, 120, 24, 11);
        assert_eq!(live[(0, 0)].bg, Color::Reset);

        app.on_key(press(KeyCode::Left));
        let gap = paint(&mut app, 120, 24, 11);
        assert!(line_text(&gap, 0).starts_with("PAST · GAP"));
        assert_eq!(gap[(0, 0)].bg, Color::Red);

        app.on_key(press(KeyCode::Left));
        let past = paint(&mut app, 120, 24, 11);
        assert!(line_text(&past, 0).starts_with("PAST · NOT LIVE"));
        assert_eq!(past[(0, 0)].bg, Color::Yellow);
    }

    /// A command line can be longer than the screen. The detail view wraps it
    /// and keeps every character, and the page keys scroll it.
    #[test]
    fn the_detail_view_wraps_and_scrolls_a_long_command() {
        let words: Vec<String> = (0..60).map(|word| format!("日本語-{word}")).collect();
        let command = words.join(" ");
        let (mut app, _) = app_with(vec![frame(
            0,
            vec![
                row(10, 50),
                RankedRow {
                    command,
                    ..row(11, 40)
                },
            ],
        )]);
        app.on_key(press(KeyCode::Down));
        app.on_key(press(KeyCode::Down));
        app.on_key(press(KeyCode::Enter));

        let lines = screen(&paint(&mut app, 40, 14, 3));
        let start = line_starting(&lines, "日本語-0 ");
        let shown: Vec<&str> = lines[start..lines.len() - 2]
            .iter()
            .flat_map(|line| line.split(' '))
            .filter(|word| !word.is_empty())
            .collect();
        assert!(shown.len() >= 2 && shown.len() < words.len(), "{lines:#?}");
        for (seen, word) in shown.iter().zip(&words) {
            assert_eq!(seen, word, "a word is missing or out of order: {lines:#?}");
        }

        app.on_key(press(KeyCode::PageDown));
        let scrolled = screen(&paint(&mut app, 40, 14, 3));
        assert!(
            !scrolled[start].starts_with("日本語-0 "),
            "the page key scrolls the command: {scrolled:#?}"
        );
    }

    /// Opens the detail view of a process whose command is `command`.
    fn detail_of(command: String) -> crate::tui::app::App {
        let (mut app, _) = app_with(vec![frame(
            0,
            vec![
                row(10, 50),
                RankedRow {
                    command,
                    ..row(11, 40)
                },
            ],
        )]);
        app.on_key(press(KeyCode::Down));
        app.on_key(press(KeyCode::Down));
        app.on_key(press(KeyCode::Enter));
        app
    }

    /// A command that fits on the screen has nothing to scroll to. The page
    /// key leaves it where it is, so no number of presses blanks the text.
    #[test]
    fn page_down_does_not_scroll_a_short_command_off_the_screen() {
        let mut app = detail_of("fits-on-one-screen --flag".to_owned());
        assert!(screen(&paint(&mut app, 80, 24, 3))
            .iter()
            .any(|line| line.contains("fits-on-one-screen")));

        for _ in 0..5 {
            app.on_key(press(KeyCode::PageDown));
            let lines = screen(&paint(&mut app, 80, 24, 3));
            assert!(
                lines.iter().any(|line| line.contains("fits-on-one-screen")),
                "the command is gone: {lines:#?}"
            );
        }
    }

    /// The page key moves by the height of the command area. The line after
    /// the last line on the screen becomes the first one, with no line skipped
    /// and none repeated.
    #[test]
    fn page_down_scrolls_the_command_by_the_lines_that_it_shows() {
        let words: Vec<String> = (0..60)
            .map(|word| format!("zq{word:02}{}", "-".repeat(26)))
            .collect();
        let mut app = detail_of(words.join(" "));
        let command_lines = |lines: &[String]| -> Vec<usize> {
            lines
                .iter()
                .filter(|line| line.starts_with("zq"))
                .map(|line| {
                    line.get(2..4)
                        .and_then(|digits| digits.parse().ok())
                        .expect("two digits follow zq")
                })
                .collect()
        };

        let first = command_lines(&screen(&paint(&mut app, 40, 20, 3)));
        let last = *first.last().expect("the command shows");
        assert_eq!(first[0], 0, "{first:?}");

        app.on_key(press(KeyCode::PageDown));
        let second = command_lines(&screen(&paint(&mut app, 40, 20, 3)));
        assert_eq!(
            second.first(),
            Some(&(last + 1)),
            "{first:?} then {second:?}"
        );
    }

    /// A move of the row with the detail view open shows another process. Its
    /// command starts at its first line, and not at the scroll of the command
    /// before it. A move that stays on the same process keeps the scroll.
    #[test]
    fn a_move_to_another_process_shows_its_command_from_the_first_line() {
        let command = |prefix: &str| -> String {
            (0..60)
                .map(|word| format!("{prefix}{word:02}{}", "-".repeat(26)))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let (mut app, _) = app_with(vec![frame(
            0,
            vec![
                RankedRow {
                    command: command("ya"),
                    ..row(10, 50)
                },
                RankedRow {
                    command: command("zq"),
                    ..row(11, 40)
                },
            ],
        )]);
        app.on_key(press(KeyCode::Down));
        app.on_key(press(KeyCode::Enter));
        paint(&mut app, 40, 20, 3);
        app.on_key(press(KeyCode::PageDown));
        paint(&mut app, 40, 20, 3);
        assert!(app.detail_scroll() > 0, "the first command scrolls");

        for (key, pid, prefix) in [
            (KeyCode::Down, 11, "zq00"),
            (KeyCode::Char('k'), 10, "ya00"),
        ] {
            app.on_key(press(KeyCode::PageDown));
            paint(&mut app, 40, 20, 3);
            app.on_key(press(key));
            let lines = screen(&paint(&mut app, 40, 20, 3));
            assert_eq!(app.row_key().map(|key| key.pid.get()), Some(pid));
            assert_eq!(app.detail_scroll(), 0, "{key:?}: {lines:#?}");
            assert!(
                lines.iter().any(|line| line.starts_with(prefix)),
                "{key:?}: the first line of the command is hidden: {lines:#?}"
            );
        }

        app.on_key(press(KeyCode::PageDown));
        paint(&mut app, 40, 20, 3);
        let scroll = app.detail_scroll();
        app.on_key(press(KeyCode::Up));
        assert_eq!(
            (app.row_key().map(|key| key.pid.get()), app.detail_scroll()),
            (Some(10), scroll),
            "a move at the first row stays on the process and keeps the scroll"
        );
    }

    /// A process states its own arguments, so a command line can hold a tab or
    /// an escape. The row stays aligned, and no such character reaches the
    /// screen.
    #[test]
    fn a_control_character_in_a_command_does_not_move_the_next_column() {
        let commands = ["claude \t tab", "claude \u{1b}[31m esc", "claude plain"];
        let rows = commands
            .iter()
            .zip(10..)
            .map(|(command, pid)| claude_row(pid, 50, command))
            .collect();
        let (mut app, _) = app_with(vec![frame(0, rows)]);

        let buffer = paint(&mut app, 100, 24, 3);
        let lines = screen(&buffer);

        let header = line_starting(&lines, "PID");
        let columns: Vec<Option<usize>> = lines[header + 1..header + 4]
            .iter()
            .map(|line| screen_column(line, "busy"))
            .collect();
        assert!(columns[0].is_some(), "{lines:#?}");
        assert!(
            columns.iter().all(|column| *column == columns[0]),
            "{columns:?} {lines:#?}"
        );
        assert!(
            !lines.iter().any(|line| line.contains(char::is_control)),
            "{lines:#?}"
        );
    }

    /// The sizes of the screens that the no-panic test draws on.
    const SIZES: [(u16, u16); 8] = [
        (1, 1),
        (1, 40),
        (200, 1),
        (80, 24),
        (7, 4),
        (8, 5),
        (30, 9),
        (120, 3),
    ];

    /// Makes a state with frames, a gap, and Claude Code rows.
    fn busy_app() -> crate::tui::app::App {
        app_with(vec![
            frame(0, vec![claude_row(10, 50, "claude 日本語"), row(11, 5)]),
            gap(2, 2, "top stopped"),
            frame_with_own(4, vec![claude_row(10, 70, "claude 🎉"), row(12, 9)], &[12]),
        ])
        .0
    }

    /// No size of screen makes the view panic, with data and with none, live
    /// and in the past, on a gap, and in the detail view.
    #[test]
    fn no_size_of_screen_makes_the_view_panic() {
        for (width, height) in SIZES {
            let mut empty = app_with(Vec::new()).0;
            paint(&mut empty, width, height, 10);

            let mut live = busy_app();
            paint(&mut live, width, height, 10);

            live.on_key(press(KeyCode::Down));
            live.on_key(press(KeyCode::Enter));
            paint(&mut live, width, height, 10);
            live.on_key(press(KeyCode::PageDown));
            paint(&mut live, width, height, 10);
            live.on_key(press(KeyCode::Esc));

            live.on_key(press(KeyCode::Left));
            paint(&mut live, width, height, 10);
            live.on_key(press(KeyCode::Left));
            paint(&mut live, width, height, 10);
            live.on_key(press(KeyCode::Char('w')));
            paint(&mut live, width, height, 10);

            let mut gaps_only = app_with(vec![gap(0, 2, "top stopped")]).0;
            paint(&mut gaps_only, width, height, 10);
        }
    }

    /// Tells whether the first cell of line `y` is reversed.
    fn is_reversed(buffer: &Buffer, y: u16) -> bool {
        buffer[(0, y)].modifier.contains(Modifier::REVERSED)
    }

    /// After moves down, the selected row is on the screen, and it is the one
    /// row that is reversed.
    #[test]
    fn the_selected_row_is_on_screen_and_reversed_after_a_move() {
        let rows = (1..=30)
            .map(|pid| row(pid, u64::from(1_000 - pid)))
            .collect();
        let (mut app, _) = app_with(vec![frame(0, rows)]);
        paint(&mut app, 120, 16, 3);
        for _ in 0..9 {
            app.on_key(press(KeyCode::Down));
        }

        let buffer = paint(&mut app, 120, 16, 3);

        let lines = screen(&buffer);
        let selected = line_containing(&lines, "/bin/command-9");
        let reversed: Vec<usize> = (0..buffer.area.height)
            .filter(|y| is_reversed(&buffer, *y))
            .map(usize::from)
            .collect();
        assert_eq!(reversed, vec![selected], "{lines:#?}");
    }

    /// The line under the table counts the processes that made no fault.
    #[test]
    fn the_more_line_counts_the_hidden_processes() {
        let (mut app, _) = app_with(vec![frame(0, vec![row(1, 50), row(2, 0), row(3, 0)])]);

        let lines = screen(&paint(&mut app, 120, 24, 3));

        assert_eq!(lines[22], "… 2 more processes made 0% of all faults");
    }

    /// The last line lists the keys.
    #[test]
    fn the_last_line_lists_the_keys() {
        let (mut app, _) = app_with(vec![frame(0, vec![row(1, 50)])]);

        let lines = screen(&paint(&mut app, 120, 24, 3));

        assert_eq!(
            lines[23],
            "←/→ frame  [ ] minute  Home oldest  End live  w span  ↑/↓ row  Enter detail  q quit"
        );
    }
}
