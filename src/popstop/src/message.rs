//! The texts that popstop shows to the user.
//!
//! The functions here are pure, so the tests reach every text without a
//! terminal, a lock, or an audio device.

use std::fmt;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Local, TimeZone};

use crate::lock::{HolderRecord, StartTime};

/// The start of each line that popstop writes, so a reader of a full
/// terminal sees which program speaks.
const PREFIX: &str = "popstop: ";

/// The command that stops the copy that runs.
const STOP_COMMAND: &str = "popstop --stop";

/// The flag that names the state directory.
const STATE_DIR_FLAG: &str = "--state-dir";

/// The line that tells the user about the effect on the sleep of the Mac.
///
/// An open output stream makes `coreaudiod` hold a power assertion, thus the
/// Mac does not idle sleep while popstop runs. The display still sleeps, and
/// a sleep from the Apple menu still works.
const NO_IDLE_SLEEP_LINE: &str = "popstop: this Mac does not idle sleep while popstop runs";

/// The line that tells the user how to stop a foreground copy.
const PRESS_CTRL_C_LINE: &str = "popstop: press Ctrl-C to stop";

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
    let HolderRecord {
        pid,
        mode,
        started_at,
    } = holder;
    let started = start_time_text_in(*started_at, zone);
    format!(
        "{PREFIX}another copy runs (pid {pid}, {mode}, started {started}), so this copy did not \
         start\n\
         {PREFIX}to stop the copy that runs, use this command: {stop_command}"
    )
}

/// Gives the lines that a foreground copy writes when it plays: the device
/// that it keeps awake, the effect on the sleep of the Mac, and the way to
/// stop it.
///
/// The text has no newline at its end.
#[must_use]
pub fn ready_lines(device_name: &str, pid: u32) -> String {
    format!(
        "{PREFIX}\"{device_name}\" stays awake while popstop runs (pid {pid})\n\
         {NO_IDLE_SLEEP_LINE}\n\
         {PRESS_CTRL_C_LINE}"
    )
}

/// Gives the lines that `popstop --background` writes when the copy that it
/// started plays: the device that the copy keeps awake, the effect on the
/// sleep of the Mac, and the command that stops the copy. `stop_command`
/// comes from [`stop_command`].
///
/// A background copy has no terminal, thus the lines name its process ID and
/// its mode, and they give a command and not a key.
///
/// The text has no newline at its end.
#[must_use]
pub fn background_ready_lines(device_name: &str, pid: u32, stop_command: &str) -> String {
    let _ = (device_name, pid, stop_command);
    String::new()
}

/// Gives the text of a background start whose copy wrote no report.
///
/// The parent knows nothing about the copy then, and the log of that copy
/// holds what it wrote. Thus the text names the log.
///
/// The text has no newline at its end.
#[must_use]
pub fn did_not_report(log_path: &Path) -> String {
    let _ = log_path;
    String::new()
}

/// Gives the text of a background start whose copy wrote no report within
/// `bound`.
///
/// The text has no newline at its end.
#[must_use]
pub fn did_not_report_within(bound: Duration, log_path: &Path) -> String {
    let _ = (bound, log_path);
    String::new()
}

/// Gives the lines that `popstop --status` writes for the copy that runs:
/// the copy itself, and the device that it keeps awake.
///
/// The text has no newline at its end.
#[must_use]
pub fn status_lines(holder: &HolderRecord, device_name: &str) -> String {
    status_lines_in(holder, device_name, &Local)
}

/// Gives the lines of `popstop --status`, with the start time in `zone`.
fn status_lines_in<Tz>(holder: &HolderRecord, device_name: &str, zone: &Tz) -> String
where
    Tz: TimeZone,
    Tz::Offset: fmt::Display,
{
    format!(
        "{}\n{PREFIX}\"{device_name}\" is the default output device",
        copy_runs_line_in(holder, zone)
    )
}

/// Gives the lines that `popstop --status` writes when the name of the
/// default output device cannot be read.
///
/// A copy runs, and that is the answer that the user asked for. So the text
/// reports the copy, and it says why the name of the device is not there.
///
/// The text has no newline at its end.
#[must_use]
pub fn status_lines_without_device_name(
    holder: &HolderRecord,
    problem: &dyn fmt::Display,
) -> String {
    status_lines_without_device_name_in(holder, problem, &Local)
}

/// Gives the lines of `popstop --status` without a device name, with the
/// start time in `zone`.
fn status_lines_without_device_name_in<Tz>(
    holder: &HolderRecord,
    problem: &dyn fmt::Display,
    zone: &Tz,
) -> String
where
    Tz: TimeZone,
    Tz::Offset: fmt::Display,
{
    format!(
        "{}\n{PREFIX}the name of the default output device cannot be read: {problem}",
        copy_runs_line_in(holder, zone)
    )
}

/// Gives the line that names the copy that runs: its PID, its mode, and its
/// start time in `zone`.
fn copy_runs_line_in<Tz>(holder: &HolderRecord, zone: &Tz) -> String
where
    Tz: TimeZone,
    Tz::Offset: fmt::Display,
{
    let HolderRecord {
        pid,
        mode,
        started_at,
    } = holder;
    let started = start_time_text_in(*started_at, zone);
    format!("{PREFIX}a copy runs (pid {pid}, {mode}, started {started})")
}

/// Gives the line that says that no copy of popstop runs.
///
/// `popstop --status` and `popstop --stop` both write it. A stop that finds
/// nothing is a success, thus the line reports and does not complain.
#[must_use]
pub fn no_copy_runs() -> String {
    format!("{PREFIX}no copy runs")
}

/// Gives the line that reports the copy that a stop ended.
#[must_use]
pub fn stopped(pid: u32) -> String {
    format!("{PREFIX}the copy stopped (pid {pid})")
}

/// Gives the text of a stop that did not end the copy within `bound`.
///
/// popstop never sends `SIGKILL` by itself, thus the text names the PID and
/// the user decides what to do next.
#[must_use]
pub fn did_not_stop(pid: u32, bound: Duration) -> String {
    format!(
        "{PREFIX}the copy did not stop within {} seconds (pid {pid})",
        bound.as_secs()
    )
}

/// Gives the text of a stop that found a record which names a process that
/// the system gave the PID to after the record was written.
///
/// A copy writes its record a moment after it takes the lock, thus a reader
/// can see the record of a copy that crashed for a very short time. popstop
/// sends no signal then, because the signal goes to a process of somebody
/// else.
#[must_use]
pub fn stale_record(pid: u32) -> String {
    format!(
        "{PREFIX}the record in the lock file names pid {pid}, and another process has that PID \
         now\n\
         {PREFIX}no signal went to that process. Do the command again"
    )
}

/// Gives one line that reports a problem, for example
/// `popstop: the lock file cannot be used: permission denied`.
#[must_use]
pub fn problem_line(problem: &dyn fmt::Display) -> String {
    format!("{PREFIX}{problem}")
}

/// Gives one line that reports a problem which popstop continues after, for
/// example `popstop: warning: ...`.
#[must_use]
pub fn warning_line(problem: &dyn fmt::Display) -> String {
    format!("{PREFIX}warning: {problem}. popstop continues")
}

#[cfg(test)]
mod tests {
    use super::{background_ready_lines, did_not_report, did_not_report_within};
    use super::{did_not_stop, no_copy_runs, stale_record, stopped};
    use super::{problem_line, ready_lines, refusal_in, start_time_text_in, stop_command};
    use super::{status_lines_in, status_lines_without_device_name_in, warning_line, PREFIX};
    use crate::lock::{HolderRecord, Mode, StartTime};
    use chrono::{FixedOffset, Utc};
    use std::path::Path;
    use std::time::Duration;

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
    fn the_ready_lines_name_the_device_the_sleep_and_the_way_to_stop() {
        assert_eq!(
            ready_lines("Klipsch R-51PM", 4242),
            "popstop: \"Klipsch R-51PM\" stays awake while popstop runs (pid 4242)\n\
             popstop: this Mac does not idle sleep while popstop runs\n\
             popstop: press Ctrl-C to stop"
        );
    }

    #[test]
    fn the_background_lines_name_the_device_the_mode_and_the_command_that_stops_the_copy() {
        assert_eq!(
            background_ready_lines("Klipsch R-51PM", 4242, "popstop --stop"),
            "popstop: \"Klipsch R-51PM\" stays awake while popstop runs (pid 4242, background)\n\
             popstop: this Mac does not idle sleep while popstop runs\n\
             popstop: to stop the copy that runs, use this command: popstop --stop"
        );

        // A copy that runs with a state directory of its own needs the same
        // directory in the command that stops it.
        assert_eq!(
            background_ready_lines("A Device", 7, "popstop --stop --state-dir '/tmp/state'"),
            "popstop: \"A Device\" stays awake while popstop runs (pid 7, background)\n\
             popstop: this Mac does not idle sleep while popstop runs\n\
             popstop: to stop the copy that runs, use this command: popstop --stop --state-dir \
             '/tmp/state'"
        );
    }

    #[test]
    fn a_background_copy_that_wrote_no_report_leaves_its_log_for_the_user() {
        let log = Path::new("/tmp/state/popstop.log");

        assert_eq!(
            did_not_report(log),
            "popstop: the background copy did not report that it plays\n\
             popstop: the log of that copy says why: /tmp/state/popstop.log"
        );
        assert_eq!(
            did_not_report_within(Duration::from_secs(10), log),
            "popstop: the background copy did not report that it plays within 10 seconds\n\
             popstop: the log of that copy says why: /tmp/state/popstop.log"
        );
    }

    #[test]
    fn the_status_text_names_the_copy_that_runs_and_the_device_that_it_holds() {
        let holder = HolderRecord {
            pid: 4242,
            mode: Mode::Foreground,
            started_at: TEN_O_CLOCK_UTC,
        };

        assert_eq!(
            status_lines_in(&holder, "Klipsch R-51PM", &Utc),
            "popstop: a copy runs (pid 4242, foreground, started 2026-09-18 10:00:00)\n\
             popstop: \"Klipsch R-51PM\" is the default output device"
        );

        // The mode comes from the record, thus a background copy reports the
        // mode that it runs in.
        let background = HolderRecord {
            pid: 5353,
            mode: Mode::Background,
            started_at: TEN_O_CLOCK_UTC,
        };
        assert_eq!(
            status_lines_without_device_name_in(&background, &"the device gave no name", &Utc),
            "popstop: a copy runs (pid 5353, background, started 2026-09-18 10:00:00)\n\
             popstop: the name of the default output device cannot be read: the device gave no name"
        );
    }

    #[test]
    fn the_stop_texts_say_what_happened_to_the_copy_and_name_its_pid() {
        assert_eq!(no_copy_runs(), "popstop: no copy runs");
        assert_eq!(stopped(4242), "popstop: the copy stopped (pid 4242)");
        assert_eq!(
            did_not_stop(4242, Duration::from_secs(5)),
            "popstop: the copy did not stop within 5 seconds (pid 4242)"
        );
        assert_eq!(
            stale_record(4242),
            "popstop: the record in the lock file names pid 4242, and another process has that PID \
             now\n\
             popstop: no signal went to that process. Do the command again"
        );
    }

    #[test]
    fn every_line_that_popstop_writes_carries_its_name() {
        let problem = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "no entrance");

        assert_eq!(problem_line(&problem), "popstop: no entrance");
        assert_eq!(
            warning_line(&problem),
            "popstop: warning: no entrance. popstop continues"
        );
        assert!(
            ready_lines("A Device", 1)
                .lines()
                .all(|line| line.starts_with(PREFIX)),
            "each ready line carries the name"
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
