//! The paint of one screen of `faulte tui`.
//!
//! [`draw`] reads the state and paints the banner, the header lines, the
//! timeline, the table or the detail view, the line of hidden rows, and the
//! line of keys. It reads no clock and no terminal: the caller gives the time
//! and the time zone in a [`ViewContext`].

use std::fmt::Display;
use std::time::SystemTime;

use chrono::TimeZone;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::tui::app::App;

/// What holds the parts of the banner apart.
const SEPARATOR: &str = " · ";

/// The banner while the history holds no frame.
const WAITING_BANNER: &str =
    "WAITING · faulte waits for the first frame. top can take 30 s on a loaded Mac.";

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

/// Gives the text of the banner.
fn banner(app: &App) -> String {
    let mut text = WAITING_BANNER.to_string();
    if let Some(gap) = app
        .history()
        .newest()
        .and_then(|newest| app.history().gap(newest))
    {
        text.push_str(SEPARATOR);
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
    let _ = context;
    let header_lines = app
        .reading()
        .map_or(0, |reading| reading.header_lines().len());
    let parts = areas(frame.area(), header_lines);
    paint_line(frame, parts.banner, &banner(app));
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::Duration;

    use chrono::FixedOffset;
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
            own: BTreeSet::new(),
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
        let _ = SessionState::Busy;
    }
}
