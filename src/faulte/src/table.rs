//! The process table: the parser of the output of `/bin/ps`.
//!
//! `/bin/ps` is setuid root, and it carries the entitlement
//! `com.apple.system-task-ports.read`. Thus a `ps` that an account without
//! privileges runs reads the owner, the parent, the resident memory, the start
//! time, and the arguments of the processes of every account. `sysinfo` gives
//! none of these for a process of another account.
//!
//! `faulte` uses the owner to find the processes of another account, and the
//! parent to find the descendants and the ancestors of a session. It uses the
//! start time to check a registry record, and to find a PID that a new process
//! uses again.

use std::str::FromStr;

use chrono::NaiveDateTime;

use crate::pid::{Pid, Uid};

/// One process, as one line of the output of `ps`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessRow {
    /// The process.
    pub pid: Pid,
    /// The parent of the process.
    pub ppid: Pid,
    /// The account that owns the process.
    pub uid: Uid,
    /// The resident memory of the process, in KiB.
    pub rss_kib: u64,
    /// True when the process is a zombie: it stopped, and its parent did not
    /// collect its exit status yet.
    pub zombie: bool,
    /// The time when the process started, in seconds since the Unix epoch.
    pub started_at_epoch_secs: u64,
    /// The arguments of the process, as `ps` prints them.
    pub command: String,
}

/// The reason why the output of `ps` is not a process table that `faulte` can
/// use.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TableParseError {
    /// The output holds no row.
    #[error("ps listed no process")]
    NoRows,
}

/// Reads the process table from the output of `ps`.
///
/// # Errors
///
/// None yet.
pub fn parse(output: &str) -> Result<Vec<ProcessRow>, TableParseError> {
    Ok(output.lines().filter_map(parse_row).collect())
}

/// The format of the start time, after the parser joins its five tokens with
/// one space.
const START_FORMAT: &str = "%a %b %d %H:%M:%S %Y";

/// Reads one row.
fn parse_row(line: &str) -> Option<ProcessRow> {
    let mut tokens = line.split_ascii_whitespace();
    let mut next = || tokens.next();
    let (pid, ppid, uid, rss, _stat) = (next()?, next()?, next()?, next()?, next()?);
    let start = [next()?, next()?, next()?, next()?, next()?].join(" ");
    let command = tokens.collect::<Vec<&str>>().join(" ");
    let started = NaiveDateTime::parse_from_str(&start, START_FORMAT)
        .ok()?
        .and_utc()
        .timestamp();
    Some(ProcessRow {
        pid: Pid::new(unsigned(pid)?),
        ppid: Pid::new(unsigned(ppid)?),
        uid: Uid::new(unsigned(uid)?),
        rss_kib: unsigned(rss)?,
        zombie: false,
        started_at_epoch_secs: u64::try_from(started).ok()?,
        command,
    })
}

/// Reads a number of ASCII digits from `token`.
fn unsigned<T: FromStr>(token: &str) -> Option<T> {
    if !token.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    token.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The row of `launchd`, in the shape that `ps` prints it: the numbers
    /// aligned to the right, then the state, the start time, and the
    /// arguments.
    const LAUNCHD: &str =
        "    1     0     0  16896 Ss   Tue Aug 25 16:45:30 2026     /sbin/launchd";

    /// `Tue Aug 25 16:45:30 2026` in UTC, in seconds since the epoch.
    /// `TZ=UTC date -j -f '%a %b %e %H:%M:%S %Y' '<start>' +%s` gives it.
    const LAUNCHD_START: u64 = 1_787_676_330;

    /// Gives the row of `launchd`, as the parser reads it.
    fn launchd() -> ProcessRow {
        ProcessRow {
            pid: Pid::new(1),
            ppid: Pid::new(0),
            uid: Uid::new(0),
            rss_kib: 16_896,
            zombie: false,
            started_at_epoch_secs: LAUNCHD_START,
            command: "/sbin/launchd".to_owned(),
        }
    }

    /// Each column of the row goes to its field. The start time is UTC,
    /// because `faulte` runs `ps` with `TZ=UTC`.
    #[test]
    fn a_row_of_launchd_parses_into_every_field() {
        let rows = parse(&format!("{LAUNCHD}\n")).expect("the row of launchd parses");

        assert_eq!(rows, [launchd()]);
    }

    /// `ps` pads a day of one digit with a space, so the columns after it move
    /// by one position. The parser reads tokens, not positions. The rows keep
    /// the order of `ps`.
    #[test]
    fn a_day_of_one_digit_and_its_padding_space_parse() {
        let text = format!(
            "{LAUNCHD}\n 4242     1   501   2048 S    Mon Sep  7 16:30:07 2026     /usr/bin/tool\n"
        );

        let rows = parse(&text).expect("a day of one digit parses");

        assert_eq!(
            rows,
            [
                launchd(),
                ProcessRow {
                    pid: Pid::new(4_242),
                    ppid: Pid::new(1),
                    uid: Uid::new(501),
                    rss_kib: 2_048,
                    zombie: false,
                    // `Mon Sep  7 16:30:07 2026` in UTC.
                    started_at_epoch_secs: 1_788_798_607,
                    command: "/usr/bin/tool".to_owned(),
                },
            ]
        );
    }

    /// Gives a row of PID 700 with the state `stat` and the command `command`.
    fn row_with(stat: &str, command: &str) -> String {
        format!("  700     1   501      0 {stat:<4} Tue Aug 25 16:45:30 2026     {command}")
    }

    /// Parses `line`, which must hold exactly one row, and gives that row.
    fn only_row(line: &str) -> ProcessRow {
        let rows = parse(line).expect("the row parses");
        let [row] = <[ProcessRow; 1]>::try_from(rows).expect("the text holds one row");
        row
    }

    /// `ps` gives the state of a zombie as `Z`, with the other flags after it.
    /// Every other state is a live process.
    #[test]
    fn a_state_that_starts_with_z_is_a_zombie() {
        for (stat, zombie) in [
            ("Z", true),
            ("Z+", true),
            ("Zs", true),
            ("S", false),
            ("R+", false),
            ("Ss", false),
            ("S+", false),
            ("U", false),
        ] {
            let row = only_row(&row_with(stat, "(node)"));

            assert_eq!(row.zombie, zombie, "the state {stat:?}");
        }
    }
}
