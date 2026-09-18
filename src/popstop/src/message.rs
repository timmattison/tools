//! The texts that popstop shows to the user.
//!
//! The functions here are pure, so the tests reach every text without a
//! terminal, a lock, or an audio device.

use std::fmt;
use std::path::Path;

use chrono::{DateTime, Local, TimeZone};

use crate::lock::{HolderRecord, StartTime};

/// The command that stops the copy that runs.
const STOP_COMMAND: &str = "popstop --stop";

/// The flag that names the state directory.
const STATE_DIR_FLAG: &str = "--state-dir";

/// Gives the command that stops the copy that runs.
///
/// A copy that runs with `--state-dir` holds the lock in that directory, so
/// the command names the same directory. The path is quoted for a shell, so
/// the user can copy the command as it is.
#[must_use]
pub fn stop_command(state_dir: Option<&Path>) -> String {
    match state_dir {
        None => STOP_COMMAND.to_owned(),
        Some(dir) => format!(
            "{STOP_COMMAND} {STATE_DIR_FLAG} {}",
            shellquote::shell_quote(&dir.to_string_lossy())
        ),
    }
}

/// Gives the start time of a copy as a local date and time, for example
/// `2026-09-18 10:00:00`.
#[must_use]
pub fn start_time_text(start: StartTime) -> String {
    start_time_text_in(start, &Local)
}

/// The format of a start time: the date and the time to the second.
const START_TIME_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

/// Gives the start time of a copy as a date and time in `zone`.
///
/// A record holds any `u64`, and the calendar of chrono ends long before
/// `u64::MAX` microseconds. A time outside the calendar shows as its number,
/// so the text never hides the value that the record holds.
fn start_time_text_in<Tz>(start: StartTime, zone: &Tz) -> String
where
    Tz: TimeZone,
    Tz::Offset: fmt::Display,
{
    let micros = start.unix_micros();
    i64::try_from(micros)
        .ok()
        .and_then(DateTime::from_timestamp_micros)
        .map_or_else(
            || format!("{micros} microseconds after the Unix epoch"),
            |utc| {
                utc.with_timezone(zone)
                    .format(START_TIME_FORMAT)
                    .to_string()
            },
        )
}

/// Gives the text of the refusal of a start, for the copy that holds the
/// lock. `stop_command` comes from [`stop_command`].
///
/// The text names the copy that runs, and it tells the user how to stop that
/// copy. It has no newline at its end.
#[must_use]
pub fn refusal(holder: &HolderRecord, stop_command: &str) -> String {
    refusal_in(holder, stop_command, &Local)
}

/// Gives the text of the refusal of a start, with the start time in `zone`.
fn refusal_in<Tz>(holder: &HolderRecord, stop_command: &str, zone: &Tz) -> String
where
    Tz: TimeZone,
    Tz::Offset: fmt::Display,
{
    let _ = (holder, stop_command, zone);
    String::new()
}

#[cfg(test)]
mod tests {
    use super::{refusal_in, start_time_text_in, stop_command};
    use crate::lock::{HolderRecord, Mode, StartTime};
    use chrono::{FixedOffset, Utc};
    use std::path::Path;

    /// 2026-09-18 10:00:00.123456 UTC.
    const TEN_O_CLOCK_UTC: StartTime = StartTime::from_unix_micros(1_789_725_600_123_456);

    /// The number of seconds in one hour.
    const HOUR: i32 = 60 * 60;

    #[test]
    fn the_stop_command_names_a_state_directory_quoted_for_a_shell() {
        assert_eq!(stop_command(None), "popstop --stop");
        assert_eq!(
            stop_command(Some(Path::new("/tmp/state dir"))),
            "popstop --stop --state-dir '/tmp/state dir'"
        );
        assert_eq!(
            stop_command(Some(Path::new("/tmp/it's here"))),
            r"popstop --stop --state-dir '/tmp/it'\''s here'"
        );
    }

    #[test]
    fn a_start_time_shows_as_a_date_and_a_time_to_the_second_in_the_zone() {
        assert_eq!(
            start_time_text_in(TEN_O_CLOCK_UTC, &Utc),
            "2026-09-18 10:00:00"
        );
        let east = FixedOffset::east_opt(2 * HOUR).expect("a valid offset");
        assert_eq!(
            start_time_text_in(TEN_O_CLOCK_UTC, &east),
            "2026-09-18 12:00:00"
        );
        let west = FixedOffset::west_opt(11 * HOUR).expect("a valid offset");
        assert_eq!(
            start_time_text_in(TEN_O_CLOCK_UTC, &west),
            "2026-09-17 23:00:00",
            "a zone west of UTC can show the day before"
        );
    }

    #[test]
    fn a_refusal_names_the_copy_that_runs_and_the_command_that_stops_it() {
        let holder = HolderRecord {
            pid: 4242,
            mode: Mode::Foreground,
            started_at: TEN_O_CLOCK_UTC,
        };

        assert_eq!(
            refusal_in(&holder, "popstop --stop", &Utc),
            "popstop: another copy runs (pid 4242, foreground, started 2026-09-18 10:00:00), so \
             this copy did not start\n\
             popstop: to stop the copy that runs, use this command: popstop --stop"
        );

        let background = HolderRecord {
            pid: 5353,
            mode: Mode::Background,
            started_at: TEN_O_CLOCK_UTC,
        };
        assert_eq!(
            refusal_in(&background, "popstop --stop --state-dir '/tmp/state'", &Utc),
            "popstop: another copy runs (pid 5353, background, started 2026-09-18 10:00:00), so \
             this copy did not start\n\
             popstop: to stop the copy that runs, use this command: popstop --stop --state-dir \
             '/tmp/state'"
        );
    }

    #[test]
    fn a_start_time_outside_the_calendar_shows_as_a_number() {
        assert_eq!(
            start_time_text_in(StartTime::from_unix_micros(u64::MAX), &Utc),
            "18446744073709551615 microseconds after the Unix epoch"
        );
    }
}
