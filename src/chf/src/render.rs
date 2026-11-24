//! Draw the state of the screen with ratatui.
//!
//! `draw` reads the state through the read accessors of `App`. It holds no
//! state of its own. The size of the screen comes from the frame on each call,
//! so a resize changes the layout on the next draw.

use std::path::Path;

use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Drawn, Mode, Version};
use crate::excerpt::{preview, thousands, wrap_text, Tone};
use crate::query::Found;
use crate::search::Hit;
use crate::session::Session;

/// The text before the query.
const PROMPT: &str = "> ";
/// The keys, on the last line.
const FOOTER: &str =
    "Enter resume · Esc quit · ↑↓ select · Shift ↑↓ scroll preview · ⇊ compactions";
/// The status line while the index reads.
const INDEXING_LABEL: &str = "  indexing ";
/// The total of an index run that is not known yet.
const UNKNOWN_TOTAL: &str = "?";
/// The end of the status line when files could not be read.
const UNREADABLE_LABEL: &str = " unreadable";
/// The start of the status line when files could not be read.
const UNREADABLE_GAP: &str = "  ";
/// The list text before the index finishes.
const READING: &str = "reading sessions…";
/// The list text while a search of the query has not answered.
const SEARCHING: &str = "searching…";
/// The list text when nothing matches.
const NO_MATCH: &str = "no session matches";
/// The title of a session that has none.
const NO_TITLE: &str = "(no title)";
/// The directory of a session that has none.
const NO_DIRECTORY: &str = "?";
/// The branch of a session that has none.
const NO_BRANCH: &str = "?";
/// The label before the branch name.
const BRANCH_LABEL: &str = "branch ";
/// The mark of a compacted session.
const COMPACTION_MARK: &str = "⇊";
/// The marker of the selected row.
const SELECTED_MARKER: &str = "> ";
/// The marker of each other row.
const OTHER_MARKER: &str = "  ";
/// The format of the date in a row.
const DATE_FORMAT: &str = "%Y-%m-%d %H:%M";
/// The home directory in a shown path.
const HOME_MARK: &str = "~";
/// The line between the list and the preview of a wide screen.
const VERTICAL_LINE: &str = "│";
/// The line between the list and the preview of a narrow screen.
const HORIZONTAL_LINE: &str = "─";
/// The title of the question box.
const ASK_TITLE: &str = " compacted session ";
/// The first line of the question, before the count.
const ASK_COUNT_PREFIX: &str = "This session has ";
/// The text for a session that has a count of 0 but is compacted.
const ASK_COUNT_ZERO: &str = "a compaction";
/// The text for a count of 1.
const ASK_COUNT_ONE: &str = "1 compaction";
/// The end of the count for more than one.
const ASK_COUNT_MANY: &str = " compactions";
/// The text of the token line, before the number.
const ASK_TOKENS_PREFIX: &str = "Before the last compaction, it held ";
/// The text of the token line, after the number.
const ASK_TOKENS_SUFFIX: &str = " tokens.";
/// The token line when the count is not known.
const ASK_TOKENS_UNKNOWN: &str = "The token count before the last compaction is not known.";
/// The warning for a match before the last compaction.
const ASK_WARNING: &str = "The first match comes before the last compaction. \
ch can show it only as part of a summary.";
/// The line for the compacted choice, after the marker.
const ASK_COMPACTED: &str = "ch   compacted: resume after the last summary";
/// The line for the expanded choice, after the marker.
const ASK_EXPANDED: &str = "che  expanded: resume the whole session";
/// The keys of the question.
const ASK_KEYS: &str = "c ch · e che · ↑↓ Enter choose · Esc back";

/// The width of the widest question box.
const ASK_MAX_WIDTH: u16 = 72;
/// The width from which the list and the preview sit side by side.
const WIDE_WIDTH: u16 = 100;
/// The columns of the date and time in a row.
const DATE_COLUMNS: usize = 16;
/// The columns of the compaction mark in a row.
const MARK_COLUMNS: usize = 4;
/// The columns of the gap between fields of a row.
const GAP_COLUMNS: usize = 2;
/// The columns of the marker of a row.
const MARKER_COLUMNS: usize = 2;
/// The share of a row, in percent, that the directory takes at most.
const DIRECTORY_PERCENT: usize = 30;
/// The least columns of the directory, when the row has room.
const DIRECTORY_MIN: usize = 10;
/// The lines of the preview header.
const HEADER_LINES: u16 = 3;
/// The lines of the screen that the prompt, the status, and the footer take.
const CHROME_LINES: u16 = 3;
/// The lines above the body.
const TOP_LINES: u16 = 2;

/// What the draw needs besides the state of the app.
pub struct Context<'a, Tz: chrono::TimeZone> {
    /// The home directory. A path under it shows as `~`.
    pub home: Option<&'a Path>,
    /// The time zone of the dates in the list.
    pub tz: &'a Tz,
}

/// The areas of the body.
struct Body {
    list: Rect,
    preview: Rect,
    separator: Rect,
    vertical: bool,
}

/// Draw the screen into `frame` and report what the draw measured.
///
/// The draw reads the size of the frame on each call. It never panics, for any
/// size from 0 by 0 up. `Drawn::list_rows` is the height of the list area, at
/// least 1. `Drawn::preview_scroll` is the clamped scroll of the preview, or 0
/// when no preview shows.
pub fn draw<Tz: chrono::TimeZone>(frame: &mut Frame, app: &App, ctx: &Context<'_, Tz>) -> Drawn
where
    Tz::Offset: std::fmt::Display,
{
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return Drawn {
            list_rows: 1,
            preview_scroll: 0,
        };
    }
    draw_prompt(frame, app, area);
    draw_status(frame, app, line_at(area, 1));
    if area.height >= CHROME_LINES {
        let footer = line_at(area, area.height - 1);
        let text = textfit::truncate_to_budget(FOOTER, usize::from(footer.width));
        frame.render_widget(
            Paragraph::new(text).style(Style::default().fg(Color::DarkGray)),
            footer,
        );
    }
    let body = body_areas(area);
    draw_list(frame, app, ctx, body.list);
    let preview_scroll = draw_preview(frame, app, ctx, body.preview);
    draw_separator(frame, &body);
    if let Mode::Ask { session, choice } = app.mode() {
        draw_ask(frame, app, session, choice, content_area(area));
    }
    Drawn {
        list_rows: body.list.height.max(1),
        preview_scroll,
    }
}

/// The one-line area at row `y` of `area`, or an empty area below the screen.
fn line_at(area: Rect, y: u16) -> Rect {
    if y < area.height {
        Rect::new(area.x, area.y + y, area.width, 1)
    } else {
        Rect::new(area.x, area.y, 0, 0)
    }
}

/// The rows between the status line and the footer.
fn content_area(area: Rect) -> Rect {
    if area.height < CHROME_LINES {
        return Rect::new(area.x, area.y, 0, 0);
    }
    Rect::new(
        area.x,
        area.y + TOP_LINES,
        area.width,
        area.height - CHROME_LINES,
    )
}

/// Split the body into the list, the line, and the preview.
fn body_areas(area: Rect) -> Body {
    let body = content_area(area);
    if area.width >= WIDE_WIDTH {
        let list_width = body.width.saturating_sub(1) / 2;
        let preview_x = body.x + list_width + 1;
        Body {
            list: Rect::new(body.x, body.y, list_width, body.height),
            separator: Rect::new(body.x + list_width, body.y, 1, body.height),
            preview: Rect::new(
                preview_x,
                body.y,
                body.width.saturating_sub(list_width + 1),
                body.height,
            ),
            vertical: true,
        }
    } else {
        let list_height = body.height / 2;
        let separator_height = u16::from(body.height > list_height);
        let preview_height = body.height - list_height - separator_height;
        Body {
            list: Rect::new(body.x, body.y, body.width, list_height),
            separator: Rect::new(body.x, body.y + list_height, body.width, separator_height),
            preview: Rect::new(
                body.x,
                body.y + list_height + separator_height,
                body.width,
                preview_height,
            ),
            vertical: false,
        }
    }
}

/// Draw the prompt, the query, and the cursor.
fn draw_prompt(frame: &mut Frame, app: &App, area: Rect) {
    let width = usize::from(area.width);
    let query = clean(app.query_text());
    let room = width.saturating_sub(PROMPT.width());
    let shown = textfit::truncate_left_to_budget(&query, room);
    let text = format!("{PROMPT}{shown}");
    let row = line_at(area, 0);
    frame.render_widget(Paragraph::new(text.clone()), row);
    if app.mode() == Mode::List {
        let x = u16::try_from(text.width().min(width.saturating_sub(1))).unwrap_or(0);
        frame.set_cursor_position(Position::new(area.x + x, area.y));
    }
}

/// Draw the status line.
fn draw_status(frame: &mut Frame, app: &App, row: Rect) {
    let progress = app.progress();
    let mut spans = vec![Span::raw(format!(
        "{}/{}",
        thousands(app.hits().len() as u64),
        thousands(app.sessions_len() as u64)
    ))];
    if !progress.done {
        let total = progress
            .total
            .map_or_else(|| UNKNOWN_TOTAL.to_string(), |t| thousands(t as u64));
        spans.push(Span::raw(format!(
            "{INDEXING_LABEL}{}/{total}",
            thousands(progress.read as u64)
        )));
    }
    if progress.unreadable > 0 {
        spans.push(Span::styled(
            format!(
                "{UNREADABLE_GAP}{}{UNREADABLE_LABEL}",
                thousands(progress.unreadable as u64)
            ),
            Style::default().fg(Color::Yellow),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), row);
}

/// Draw the rows of the list, or the sentence for an empty list.
fn draw_list<Tz: chrono::TimeZone>(frame: &mut Frame, app: &App, ctx: &Context<'_, Tz>, area: Rect)
where
    Tz::Offset: std::fmt::Display,
{
    if area.width == 0 || area.height == 0 {
        return;
    }
    if app.hits().is_empty() {
        let done = app.progress().done || !app.query_text().is_empty();
        let text = if app.searching() {
            SEARCHING
        } else if done {
            NO_MATCH
        } else {
            READING
        };
        let text = textfit::truncate_to_budget(text, usize::from(area.width));
        frame.render_widget(Paragraph::new(text), line_at(area, 0));
        return;
    }
    let width = usize::from(area.width);
    let rows = app
        .hits()
        .iter()
        .enumerate()
        .skip(app.list_offset())
        .take(usize::from(area.height));
    for (y, (index, hit)) in (0_u16..).zip(rows) {
        let selected = index == app.selected();
        let text = row_text(app.session(hit.session), selected, ctx, width);
        let style = if selected {
            Style::default()
                .add_modifier(Modifier::BOLD)
                .bg(Color::Blue)
        } else {
            Style::default()
        };
        frame.render_widget(Paragraph::new(text).style(style), line_at(area, y));
    }
}

/// The text of one row, exactly `width` columns wide.
fn row_text<Tz: chrono::TimeZone>(
    session: &Session,
    selected: bool,
    ctx: &Context<'_, Tz>,
    width: usize,
) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let marker = if selected {
        SELECTED_MARKER
    } else {
        OTHER_MARKER
    };
    let date = session
        .last_activity
        .with_timezone(ctx.tz)
        .format(DATE_FORMAT)
        .to_string();
    let prefix = format!(
        "{marker}{date}{gap}{mark}",
        gap = " ".repeat(GAP_COLUMNS),
        mark = compaction_mark(session)
    );
    let fixed = MARKER_COLUMNS + DATE_COLUMNS + GAP_COLUMNS + MARK_COLUMNS;
    let available = width.saturating_sub(fixed + GAP_COLUMNS);
    let directory_width = (width * DIRECTORY_PERCENT / 100)
        .max(DIRECTORY_MIN)
        .min(available);
    let title_width = available.saturating_sub(directory_width);
    let directory = display_directory(session.cwd.as_deref(), ctx.home);
    let directory = textfit::truncate_left_to_budget(&clean(&directory), directory_width);
    let title = session.title().unwrap_or(NO_TITLE);
    let title = textfit::truncate_to_budget(&clean(title), title_width);
    let row = format!(
        "{prefix}{directory}{gap}{title}",
        directory = textfit::pad_right(&directory, directory_width),
        gap = " ".repeat(GAP_COLUMNS)
    );
    let row = textfit::truncate_to_budget(&row, width);
    textfit::pad_right(&row, width)
}

/// The mark of a compacted session, exactly 4 columns wide.
fn compaction_mark(session: &Session) -> String {
    let facts = &session.compaction;
    let mark = if !facts.compacted {
        String::new()
    } else if facts.compactions == 0 {
        COMPACTION_MARK.to_string()
    } else {
        format!("{COMPACTION_MARK}{}", facts.compactions)
    };
    textfit::pad_right(
        &textfit::truncate_to_budget(&mark, MARK_COLUMNS),
        MARK_COLUMNS,
    )
}

/// Replace each control character with a space.
fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// A directory with the home shown as `~`.
fn display_directory(cwd: Option<&str>, home: Option<&Path>) -> String {
    let Some(cwd) = cwd else {
        return NO_DIRECTORY.to_string();
    };
    let Some(home) = home.map(|h| h.to_string_lossy()) else {
        return cwd.to_string();
    };
    let home = home.trim_end_matches('/');
    if home.is_empty() {
        return cwd.to_string();
    }
    match cwd.strip_prefix(home) {
        Some("") => HOME_MARK.to_string(),
        Some(rest) if rest.starts_with('/') => format!("{HOME_MARK}{rest}"),
        _ => cwd.to_string(),
    }
}

/// Draw the preview of the selected hit. Return the clamped scroll.
fn draw_preview<Tz: chrono::TimeZone>(
    frame: &mut Frame,
    app: &App,
    ctx: &Context<'_, Tz>,
    area: Rect,
) -> i64
where
    Tz::Offset: std::fmt::Display,
{
    let Some(hit) = app.selected_hit() else {
        return 0;
    };
    if area.width == 0 || area.height == 0 {
        return 0;
    }
    let session = app.session(hit.session);
    let width = usize::from(area.width);
    let branch = session.git_branch.as_deref().unwrap_or(NO_BRANCH);
    let directory = display_directory(session.cwd.as_deref(), ctx.home);
    let mut lines: Vec<Line> = [
        session.id.as_str().to_string(),
        directory,
        format!("{BRANCH_LABEL}{branch}"),
    ]
    .into_iter()
    .map(|text| Line::from(textfit::truncate_to_budget(&clean(&text), width)))
    .collect();
    let left = area.height.saturating_sub(HEADER_LINES);
    let body = preview(
        session,
        app.query(),
        &hit.found,
        area.width,
        left,
        app.preview_scroll(),
    );
    lines.extend(body.lines.iter().map(|line| {
        Line::from(
            line.0
                .iter()
                .map(|piece| Span::styled(piece.text.clone(), tone_style(piece.tone)))
                .collect::<Vec<_>>(),
        )
    }));
    frame.render_widget(Paragraph::new(lines), area);
    body.scroll
}

/// The style of a tone.
fn tone_style(tone: Tone) -> Style {
    match tone {
        Tone::Text => Style::default(),
        Tone::Highlight => Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(Color::Black)
            .bg(Color::Yellow),
        Tone::Author => Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(Color::Cyan),
        Tone::Compaction => Style::default().fg(Color::Magenta),
        Tone::Muted => Style::default().fg(Color::DarkGray),
    }
}

/// Draw the line between the list and the preview.
fn draw_separator(frame: &mut Frame, body: &Body) {
    let area = body.separator;
    if area.width == 0 || area.height == 0 {
        return;
    }
    let (glyph, count) = if body.vertical {
        (VERTICAL_LINE, usize::from(area.height))
    } else {
        (HORIZONTAL_LINE, 1)
    };
    let line = if body.vertical {
        glyph.to_string()
    } else {
        glyph.repeat(usize::from(area.width))
    };
    let text = vec![line; count].join("\n");
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

/// The lines of the question about a compacted session.
fn ask_lines(app: &App, session_index: usize, choice: Version) -> Vec<String> {
    let session = app.session(session_index);
    let facts = &session.compaction;
    let count = match facts.compactions {
        0 => ASK_COUNT_ZERO.to_string(),
        1 => ASK_COUNT_ONE.to_string(),
        n => format!("{n}{ASK_COUNT_MANY}"),
    };
    let mut lines = vec![format!("{ASK_COUNT_PREFIX}{count}.")];
    lines.push(match facts.last_pre_tokens {
        Some(tokens) => format!(
            "{ASK_TOKENS_PREFIX}{}{ASK_TOKENS_SUFFIX}",
            thousands(tokens)
        ),
        None => ASK_TOKENS_UNKNOWN.to_string(),
    });
    if match_before_last_compaction(app, session_index, session) {
        lines.push(ASK_WARNING.to_string());
    }
    let marker = |version: Version| {
        if version == choice {
            SELECTED_MARKER
        } else {
            OTHER_MARKER
        }
    };
    lines.push(String::new());
    lines.push(format!("{}{ASK_COMPACTED}", marker(Version::Compacted)));
    lines.push(format!("{}{ASK_EXPANDED}", marker(Version::Expanded)));
    lines.push(String::new());
    lines.push(ASK_KEYS.to_string());
    lines
}

/// True when the first match of the session sits before its last compaction.
fn match_before_last_compaction(app: &App, session_index: usize, session: &Session) -> bool {
    let hit: Option<&Hit> = app.hits().iter().find(|h| h.session == session_index);
    match (hit.map(|h| &h.found), session.last_compaction_before()) {
        (Some(Found::At(m)), Some(before)) => m.message < before,
        _ => false,
    }
}

/// Draw the question box, centered over `area`.
fn draw_ask(frame: &mut Frame, app: &App, session: usize, choice: Version, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let width = area.width.min(ASK_MAX_WIDTH);
    let inner = width.saturating_sub(2).max(1);
    // The box wraps its lines here and draws each row as it comes, so the
    // count of rows is the count of lines it draws. A paragraph that wraps
    // again at word breaks can need more rows than such a count.
    let rows: Vec<Line> = ask_lines(app, session, choice)
        .iter()
        .flat_map(|line| wrap_text(line, inner))
        .map(Line::from)
        .collect();
    let height = u16::try_from(rows.len() + 2)
        .unwrap_or(u16::MAX)
        .min(area.height);
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, rect);
    let block = Block::default().borders(Borders::ALL).title(ASK_TITLE);
    frame.render_widget(Paragraph::new(rows).block(block), rect);
}
