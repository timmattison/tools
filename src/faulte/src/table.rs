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
    /// A numeric column holds a value that is not a number of its type.
    #[error(
        "ps printed a value of the column {column} that is not a number of its type, at line \
         {number}: {line:?}"
    )]
    MalformedNumber {
        /// The name of the column, as `faulte` gives it to `ps -o`.
        column: &'static str,
        /// The number of the line in the output, from 1.
        number: usize,
        /// The line as `ps` printed it.
        line: String,
    },
    /// The five fields of the start time are not a time at or after the
    /// start of the Unix epoch.
    #[error(
        "ps printed a start time that faulte cannot read as {START_FORMAT} in UTC, at or after \
         1970, at line {number}: {line:?}"
    )]
    MalformedStart {
        /// The number of the line in the output, from 1.
        number: usize,
        /// The line as `ps` printed it.
        line: String,
    },
}

/// The column of the PID, as `ps -o` names it.
const PID_COLUMN: &str = "pid";

/// The column of the PID of the parent.
const PPID_COLUMN: &str = "ppid";

/// The column of the UID of the owner.
const UID_COLUMN: &str = "uid";

/// The column of the resident memory, in KiB.
const RSS_COLUMN: &str = "rss";

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
    /// The numeric column of this name holds a value that is not a number of
    /// its type.
    MalformedNumber(&'static str),
}

impl RowFault {
    /// Gives the error of this fault at the line `line`, whose number is
    /// `number`.
    fn at(self, number: usize, line: &str) -> TableParseError {
        let line = line.to_owned();
        match self {
            Self::TooFewFields => TableParseError::TooFewFields { number, line },
            Self::MalformedNumber(column) => TableParseError::MalformedNumber {
                column,
                number,
                line,
            },
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
    let ([pid, ppid, uid, rss, stat, weekday, month, day, time, year], command) =
        split_fields::<FIELDS>(line).ok_or(RowFault::TooFewFields)?;
    let pid = Pid::new(number(PID_COLUMN, pid)?);
    let ppid = Pid::new(number(PPID_COLUMN, ppid)?);
    let uid = parse_uid(uid)?;
    let rss_kib = number(RSS_COLUMN, rss)?;
    let start = [weekday, month, day, time, year].join(" ");
    let Some(started) = NaiveDateTime::parse_from_str(&start, START_FORMAT)
        .ok()
        .and_then(|start| u64::try_from(start.and_utc().timestamp()).ok())
    else {
        return Ok(None);
    };
    Ok(Some(ProcessRow {
        pid,
        ppid,
        uid,
        rss_kib,
        zombie: stat.starts_with(ZOMBIE),
        started_at_epoch_secs: started,
        command: command.to_owned(),
    }))
}

/// Reads the value `token` of the numeric column `column`.
fn number<T: FromStr>(column: &'static str, token: &str) -> Result<T, RowFault> {
    unsigned(token).ok_or(RowFault::MalformedNumber(column))
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

/// The sign that `ps` prints before a negative UID.
const MINUS: char = '-';

/// Reads the UID `token`.
///
/// `ps` prints the UID as a signed 32-bit number, so the UID of `nobody`,
/// 4294967294, prints as `-2`. A minus and ASCII digits give the UID with the
/// same 32 bits as that negative value. ASCII digits alone give the UID
/// itself. `-0` is not a UID, because `ps` never prints it.
fn parse_uid(token: &str) -> Result<Uid, RowFault> {
    let value = match token.strip_prefix(MINUS) {
        // The check of the digits comes first, because the parse of an `i32`
        // also accepts a sign after the minus.
        Some(digits) => ascii_digits(digits)
            .then(|| token.parse::<i32>().ok())
            .flatten()
            .filter(|value| value.is_negative())
            .map(i32::cast_unsigned),
        None => unsigned(token),
    };
    value
        .map(Uid::new)
        .ok_or(RowFault::MalformedNumber(UID_COLUMN))
}

/// Tells whether `text` holds ASCII digits only.
///
/// The parse of a Rust integer also accepts a leading `+`, and `ps` never
/// prints one.
fn ascii_digits(text: &str) -> bool {
    text.bytes().all(|byte| byte.is_ascii_digit())
}

/// Reads a number of ASCII digits from `token`. An empty text fails the
/// parse, and so does a value too large for `T`.
fn unsigned<T: FromStr>(token: &str) -> Option<T> {
    ascii_digits(token).then(|| token.parse().ok()).flatten()
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

    /// Gives a row with the PID `pid`, the parent `ppid`, the UID `uid`, and
    /// the RSS `rss`, each aligned to the right as `ps` prints it.
    fn row_of([pid, ppid, uid, rss]: [&str; 4]) -> String {
        format!(
            "{pid:>5} {ppid:>5} {uid:>5} {rss:>6} S    Tue Aug 25 16:45:30 2026     /usr/bin/tool"
        )
    }

    /// The four numeric columns, in the order of the row, with a valid value
    /// of each one.
    const NUMERIC_COLUMNS: [(&str, &str); 4] = [
        (PID_COLUMN, "700"),
        (PPID_COLUMN, "1"),
        (UID_COLUMN, "501"),
        (RSS_COLUMN, "2048"),
    ];

    /// Each numeric column that holds a value other than ASCII digits of its
    /// type is refused, with the name of the column and the line. A sign, a
    /// fraction, another base, a value too large for the type, and multi-byte
    /// text are each refused, and none of them panics.
    #[test]
    fn a_value_that_is_not_a_number_of_its_column_is_refused_with_its_line() {
        let common = [
            "abc", "+12", "3.5", "1e3", "0x1F", "日本語", "🎉", "12🎉", "１２", "12\u{a0}", "café",
        ];
        let cases = [
            (PID_COLUMN, vec!["-5", "4294967296"]),
            (PPID_COLUMN, vec!["-5", "4294967296"]),
            (UID_COLUMN, vec!["4294967296", "+2"]),
            (RSS_COLUMN, vec!["-5", "18446744073709551616"]),
        ];
        for (position, (column, own)) in cases.into_iter().enumerate() {
            for bad in common.iter().copied().chain(own) {
                let mut values = NUMERIC_COLUMNS.map(|(_, good)| good);
                if let Some(value) = values.get_mut(position) {
                    *value = bad;
                }
                let line = row_of(values);

                let error = parse(&with_bad_line_at_3(&line))
                    .expect_err("a value that is not a number is refused");

                assert_eq!(
                    error,
                    TableParseError::MalformedNumber {
                        column,
                        number: 3,
                        line: line.clone()
                    },
                    "the value {bad:?} of the column {column}"
                );
                let message = error.to_string();
                assert!(
                    message.contains(&format!("column {column} "))
                        && message.contains("at line 3")
                        && message.contains(&format!("{line:?}")),
                    "the message names the column and shows the line: {message}"
                );
            }
        }
    }

    /// The largest value of each type, and zero, are numbers, not errors.
    #[test]
    fn the_largest_values_and_zero_parse() {
        let largest = only_row(&row_of([
            "4294967295",
            "4294967295",
            "4294967295",
            "18446744073709551615",
        ]));
        let zero = only_row(&row_of(["0", "0", "0", "0"]));

        assert_eq!(
            (largest.pid, largest.ppid, largest.uid, largest.rss_kib),
            (
                Pid::new(u32::MAX),
                Pid::new(u32::MAX),
                Uid::new(u32::MAX),
                u64::MAX
            )
        );
        assert_eq!(
            (zero.pid, zero.ppid, zero.uid, zero.rss_kib),
            (Pid::new(0), Pid::new(0), Uid::new(0), 0)
        );
    }

    /// The UID of `nobody`. `id -u nobody` prints it, and `ps` prints `-2`.
    const NOBODY: u32 = 4_294_967_294;

    /// `ps` prints the UID as a signed 32-bit number. On 2026-09-18, three
    /// processes of `nobody` on this Mac printed the UID `-2`. A negative
    /// value is the UID with the same 32 bits, so both spellings give the
    /// same account.
    #[test]
    fn a_negative_uid_is_the_uid_with_the_same_bits() {
        for (printed, uid) in [
            ("-2", NOBODY),
            ("4294967294", NOBODY),
            ("-1", u32::MAX),
            ("-2147483648", 2_147_483_648),
            ("2147483647", 2_147_483_647),
        ] {
            let row = only_row(&row_of(["700", "1", printed, "2048"]));

            assert_eq!(row.uid, Uid::new(uid), "the UID {printed:?}");
        }
    }

    /// A negative UID is a minus and ASCII digits of a value below zero that
    /// fits in 32 bits. Every other text with a minus is refused. `-0` is
    /// refused too, because `ps` never prints it.
    #[test]
    fn a_uid_with_a_minus_that_is_not_a_negative_number_is_refused() {
        for bad in ["-", "--2", "-0", "-2147483649", "-+2", "-２", "-2🎉", "-0x2", "2-"] {
            let line = row_of(["700", "1", bad, "2048"]);

            assert_eq!(
                parse(&with_bad_line_at_3(&line)),
                Err(TableParseError::MalformedNumber {
                    column: UID_COLUMN,
                    number: 3,
                    line: line.clone()
                }),
                "the UID {bad:?}"
            );
        }
    }

    /// Gives a row of PID 700 that started at `start`.
    fn row_started(start: &str) -> String {
        format!("  700     1   501   2048 S    {start}     /usr/bin/tool")
    }

    /// Each start time that is not a weekday, a month, a day, a time, and a
    /// year in the names of the C locale is refused, with the line. A weekday
    /// that does not match the date is refused too, because `ps` computes
    /// both from one time. A time before 1970 has no value in seconds since
    /// the epoch. None of them panics.
    #[test]
    fn a_start_time_that_does_not_parse_is_refused_with_its_line() {
        for bad in [
            "Tue Foo 25 16:45:30 2026",
            "Xyz Aug 25 16:45:30 2026",
            "Mon Aug 25 16:45:30 2026",
            "Tue Aug 32 16:45:30 2026",
            "Tue Aug 25 25:00:00 2026",
            "Tue Aug 25 16:45 2026 x",
            "Tue Aug 25 16:45:30 20x6",
            "Tue 25 Aug 16:45:30 2026",
            "Di Aug 25 16:45:30 2026",
            "火 8月 25 16:45:30 2026",
            "Tue Aug ２５ 16:45:30 2026",
            "Tue Aug 25 16:45:30 🎉",
            "Wed Dec 31 23:59:59 1969",
        ] {
            let line = row_started(bad);

            let error = parse(&with_bad_line_at_3(&line))
                .expect_err("a start time that does not parse is refused");

            assert_eq!(
                error,
                TableParseError::MalformedStart {
                    number: 3,
                    line: line.clone()
                },
                "the start time {bad:?}"
            );
            let message = error.to_string();
            assert!(
                message.contains("start time")
                    && message.contains("at line 3")
                    && message.contains(&format!("{line:?}")),
                "the message shows the line and its number: {message}"
            );
        }
    }

    /// The first second of the epoch is zero, not an error.
    #[test]
    fn the_start_of_the_epoch_is_zero() {
        let row = only_row(&row_started("Thu Jan  1 00:00:00 1970"));

        assert_eq!(row.started_at_epoch_secs, 0);
    }
}
