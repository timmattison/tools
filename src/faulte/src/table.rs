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
    /// A line holds fewer tokens than the columns before the command.
    #[error(
        "ps printed a row of fewer than {FIELDS} fields, at line {number}: {line:?}. A row holds \
         the PID, the parent, the UID, the RSS, the state, and the five fields of the start \
         time, then the command"
    )]
    TooFewFields {
        /// The number of the line in the output, from 1.
        number: usize,
        /// The line as `ps` printed it.
        line: String,
    },
}

/// Reads the process table from the output of `ps`.
///
/// # Errors
///
/// None yet.
pub fn parse(output: &str) -> Result<Vec<ProcessRow>, TableParseError> {
    output
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim_ascii().is_empty())
        .filter_map(|(index, line)| {
            parse_row(line)
                .map_err(|fault| fault.at(index + 1, line))
                .transpose()
        })
        .collect()
}

/// The reason why one line is not a row.
enum RowFault {
    /// The line holds fewer than [`FIELDS`] tokens.
    TooFewFields,
}

impl RowFault {
    /// Gives the error of this fault at the line `line`, whose number is
    /// `number`.
    fn at(self, number: usize, line: &str) -> TableParseError {
        let line = line.to_owned();
        match self {
            Self::TooFewFields => TableParseError::TooFewFields { number, line },
        }
    }
}

/// The format of the start time, after the parser joins its five tokens with
/// one space.
const START_FORMAT: &str = "%a %b %d %H:%M:%S %Y";

/// The first letter of the state of a zombie, as `ps` prints it.
const ZOMBIE: char = 'Z';

/// The count of tokens before the command: the PID, the parent, the UID, the
/// RSS, the state, and the five tokens of the start time.
const FIELDS: usize = 10;

/// Reads one row.
fn parse_row(line: &str) -> Result<Option<ProcessRow>, RowFault> {
    let (fields, command) = split_fields::<FIELDS>(line).ok_or(RowFault::TooFewFields)?;
    Ok(read_row(fields, command))
}

/// Reads the values of one row from its fields and its command.
fn read_row(
    [pid, ppid, uid, rss, stat, weekday, month, day, time, year]: [&str; FIELDS],
    command: &str,
) -> Option<ProcessRow> {
    let start = [weekday, month, day, time, year].join(" ");
    let started = NaiveDateTime::parse_from_str(&start, START_FORMAT)
        .ok()?
        .and_utc()
        .timestamp();
    Some(ProcessRow {
        pid: Pid::new(unsigned(pid)?),
        ppid: Pid::new(unsigned(ppid)?),
        uid: Uid::new(unsigned(uid)?),
        rss_kib: unsigned(rss)?,
        zombie: stat.starts_with(ZOMBIE),
        started_at_epoch_secs: u64::try_from(started).ok()?,
        command: command.to_owned(),
    })
}

/// Splits the first `N` tokens off `line`, and gives them with the rest of the
/// line.
///
/// A token is a run of characters that are not ASCII white space. The rest
/// keeps the spaces inside it, and loses the white space before and after it.
/// The function splits only where `split_once` finds a separator, so it never
/// cuts a multi-byte character. `None` means that `line` holds fewer than `N`
/// tokens.
fn split_fields<const N: usize>(line: &str) -> Option<([&str; N], &str)> {
    let mut fields = [""; N];
    let mut rest = line;
    for field in &mut fields {
        let trimmed = rest.trim_ascii_start();
        let (token, after) = trimmed
            .split_once(|character: char| character.is_ascii_whitespace())
            .unwrap_or((trimmed, ""));
        if token.is_empty() {
            return None;
        }
        *field = token;
        rest = after;
    }
    Some((fields, rest.trim_ascii()))
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

    /// The command is the rest of the line after the start time. `ps` joins
    /// the arguments with one space, and an argument can hold spaces, so the
    /// spaces in the command are part of it. The parser removes only the
    /// separator before the command and the spaces after it.
    #[test]
    fn the_command_keeps_its_spaces_and_its_text_exactly() {
        for command in [
            "claude --name \"日本語 🎉 café\"",
            "/usr/bin/tool  --flag   value",
            "sh -c sleep\t 5",
            "cafM-CM-) x\\012y",
            "🎉",
            "(node)",
        ] {
            let row = only_row(&row_with("S", command));

            assert_eq!(row.command, command, "the command {command:?}");
        }

        let trailing = only_row(&row_with("S", "/sbin/launchd  \t "));
        assert_eq!(trailing.command, "/sbin/launchd");
    }

    /// A process with no readable arguments gives a line that ends after the
    /// start time. That line is a row with an empty command.
    #[test]
    fn a_line_that_ends_after_the_start_time_has_an_empty_command() {
        for line in [
            "  700     1   501      0 S    Tue Aug 25 16:45:30 2026",
            "  700     1   501      0 S    Tue Aug 25 16:45:30 2026     ",
        ] {
            let row = only_row(line);

            assert_eq!(row.command, "", "the line {line:?}");
            assert_eq!(row.started_at_epoch_secs, LAUNCHD_START);
        }
    }

    /// Gives an output of `ps` that holds `bad` at line 3, between two rows
    /// and after a blank line.
    fn with_bad_line_at_3(bad: &str) -> String {
        format!("{LAUNCHD}\n\n{bad}\n{LAUNCHD}\n")
    }

    /// A line of fewer than ten fields is not a row. The parser refuses the
    /// whole output, because a table that lacks a process looks the same as a
    /// correct one. Multi-byte text gives the same error, and no panic.
    #[test]
    fn a_line_of_fewer_than_ten_fields_is_refused_with_its_line() {
        for bad in [
            "  700     1   501      0 S    Tue Aug 25 16:45:30",
            "  700     1   501      0 S",
            "  700",
            "Tue Aug 25 16:45:30 2026",
            "日本語 🎉 café",
            // A no-break space is not a separator, so `700` and `1` are one
            // token, and the line holds nine.
            "  700\u{a0}1   501      0 S    Tue Aug 25 16:45:30 2026",
        ] {
            let error = parse(&with_bad_line_at_3(bad)).expect_err("a short line is refused");

            assert_eq!(
                error,
                TableParseError::TooFewFields {
                    number: 3,
                    line: bad.to_owned()
                },
                "the line {bad:?}"
            );
            let message = error.to_string();
            assert!(
                message.contains("at line 3") && message.contains(&format!("{bad:?}")),
                "the message shows the line and its number: {message}"
            );
        }
    }

    /// A blank line and a line of white space are not rows, and they are not
    /// errors.
    #[test]
    fn blank_lines_are_not_rows() {
        let text = format!("\n{LAUNCHD}\n   \n\t\n{LAUNCHD}\n\n");

        let rows = parse(&text).expect("blank lines are not errors");

        assert_eq!(rows, [launchd(), launchd()]);
    }
}
