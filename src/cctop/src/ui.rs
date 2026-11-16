//! The screen of cctop: the table of sessions, the change log, and the
//! status line.

use chrono::TimeZone;
use ratatui::Frame;

use crate::app::App;
use crate::state::EpochMillis;

/// Draw the screen: the table of sessions, the change log, and the status
/// line. `now` gives the time that the elapsed times count to.
pub fn draw(frame: &mut Frame, app: &App, now: EpochMillis) {
    let _ = (frame, app, now);
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

    use crate::state::{PaneState, StateFile};

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
}
