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
use ratatui::widgets::Paragraph;
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
        "history holds {} ({} frames, {} gaps)",
        occ::format_uptime(span.as_secs()),
        extent.frames,
        extent.gaps,
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

/// Gives the cells of each row of `reading`. The command of an own row starts
/// with [`OWN_PREFIX`]. The age of a row is the time
/// from its start to the frame that the row comes from.
fn table_cells(reading: &Reading, accounts: &Accounts) -> Vec<Cells> {
    reading
        .rows()
        .map(|view| {
            let mut cells = render::row_cells(reading.ranking(), view.row, accounts, view.as_of);
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
/// reader does not take the reader of the faults for the cause of them.
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

/// Paints the header of the table and the rows that fit in `area`.
fn paint_table(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    let page = usize::from(area.height.saturating_sub(1));
    app.set_page_rows(page);
    if let Some(gap) = past_gap(app) {
        paint_line(frame, area, &gap.reason);
        return;
    }
    let Some(reading) = app.reading() else {
        return;
    };
    let cells = table_cells(reading, app.history().accounts());
    let widths = fit_widths(column_widths(&cells), usize::from(area.width));
    let mut lines = vec![Line::raw(table_line(render::COLUMNS, &widths))];
    lines.extend(
        cells
            .iter()
            .skip(app.top_row())
            .take(page)
            .map(|row| Line::raw(table_line(row.iter().map(String::as_str), &widths))),
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

/// Gives the text of the banner.
fn banner<Tz: TimeZone>(app: &App, context: &ViewContext<'_, Tz>) -> String
where
    Tz::Offset: Display,
{
    let history = app.history();
    if let Some(gap) = past_gap(app) {
        return format!(
            "{PAST}{SEPARATOR}{GAP}{SEPARATOR}{} ({} ago){SEPARATOR}{MEASURED_NOTHING}{}: {}",
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
            "{PAST}{SEPARATOR}{NOT_LIVE}{SEPARATOR}{frame_text}{SEPARATOR}{END_RETURNS}"
        );
    }
    let mut text = format!("{LIVE}{SEPARATOR}{frame_text}");
    if let Some(gap) = history.newest().and_then(|newest| history.gap(newest)) {
        text.push_str(LAST_READ_FAILED);
        text.push_str(&gap.reason);
    }
    text
}

/// Paints one screen.
///
/// The function sets the page size of `app` from the size of the table, and
/// changes nothing else. A screen of any size is valid: a part with no room
/// draws nothing.
pub fn draw<Tz: TimeZone>(frame: &mut Frame<'_>, app: &mut App, context: &ViewContext<'_, Tz>)
where
    Tz::Offset: Display,
{
    let header_lines = app.reading().map_or_else(Vec::new, Reading::header_lines);
    let parts = areas(frame.area(), header_lines.len());
    paint_line(frame, parts.banner, &banner(app, context));
    paint_lines(frame, parts.headers, &header_lines);
    paint_timeline(frame, parts.timeline, app);
    paint_table(frame, parts.table, app);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::Duration;

    use chrono::FixedOffset;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
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
            "LIVE · frame of 22:13:20 (11s ago) · history holds 2s (1 frames, 1 gaps) · the last read failed: top stopped"
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
}
