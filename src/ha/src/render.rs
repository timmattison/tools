//! Draws the board of `ha` and the text formats of its columns.

use std::fmt::Display;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use ratatui::Frame;

use crate::board::Board;
use crate::status::Status;

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
    let _ = (frame, board, status, now, tz);
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
        let mut board = Board::new(Duration::from_secs(8 * 3_600));
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
                let mut skip = 0usize;
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
}
