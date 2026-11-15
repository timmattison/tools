//! One sample of the process table.
//!
//! `whyslow` reads the table of `/bin/ps` twice. The CPU time that a process
//! used between the two reads is the second CPU time minus the first. This
//! module holds the command line of `ps`, the parser of its output, the type
//! of a CPU time, and the subtraction of two tables.

use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;

use faulte::pid::{Pid, Uid};
use faulte::table::{parse_ps_number, parse_ps_uid, split_fields};

/// The path of the process table. It is the `/bin/ps` of macOS, which is
/// setuid root and reads the processes of every account.
pub const PROGRAM: &str = faulte::table::PROGRAM;

/// The columns of a row, in the order that [`parse`] reads them. Each column
/// has an empty header, so `ps` prints no header line.
pub const COLUMNS: &str = "pid=,uid=,lstart=,time=,rss=,comm=";

/// The variables that the caller sets in the environment of [`PROGRAM`].
///
/// `ps` prints the start time in the local zone and names no zone, so
/// `TZ=UTC` gives one text for each start time, also when daylight saving
/// time ends.
///
/// The locale is `en_US.UTF-8` and not `C`. The C locale makes `ps` write
/// each non-ASCII byte of a name as an escape: `é` is `M-CM-)`. A person reads
/// the names, so the escape would hide them. `en_US.UTF-8` prints UTF-8 as it
/// is, and gives the same English names in `lstart` as the C locale. A Mac
/// without that locale falls back to the C locale, which gives the same
/// `lstart`.
pub const ENVIRONMENT: [(&str, &str); 2] = [("TZ", "UTC"), ("LC_ALL", "en_US.UTF-8")];

/// Gives the arguments of [`PROGRAM`].
///
/// `-A` asks for every process of every account. `-ww` removes the limit on
/// the width, so `ps` does not cut a long path. `-o` gives the [`COLUMNS`].
#[must_use]
pub fn arguments() -> Vec<String> {
    vec![
        "-A".to_owned(),
        "-ww".to_owned(),
        "-o".to_owned(),
        COLUMNS.to_owned(),
    ]
}

/// Gives the CPU time that each process of `after` used between the two
/// snapshots.
///
/// The result holds one value for each row of `after`, in the same order. The
/// identity of a process is its PID and its start time, because the system can
/// give a PID to a new process after the old one exits.
///
/// - A process in both snapshots with the same identity gives the CPU time of
///   `after` minus the CPU time of `before`. The subtraction stops at zero.
/// - A process in `after` only, or a PID with another start time, is a new
///   process. It gives all of its CPU time.
/// - A row of `before` that is not in `after` gives no value, because the
///   process exited and the result has no row for it.
#[must_use]
pub fn cpu_used(before: &[ProcessRow], after: &[ProcessRow]) -> Vec<CpuTime> {
    let earlier: HashMap<(Pid, &str), CpuTime> = before
        .iter()
        .map(|row| ((row.pid, row.started.as_str()), row.cpu))
        .collect();
    after
        .iter()
        .map(|row| {
            earlier
                .get(&(row.pid, row.started.as_str()))
                .map_or(row.cpu, |first| row.cpu.saturating_sub(*first))
        })
        .collect()
}

/// The character between the minutes and the seconds of a time token.
const MINUTES_SEPARATOR: char = ':';

/// The character between the seconds and the hundredths of a time token.
const HUNDREDTHS_SEPARATOR: char = '.';

/// The count of digits of the seconds and of the hundredths.
const DIGITS_OF_A_FIELD: usize = 2;

/// The seconds in one minute.
const SECONDS_PER_MINUTE: u64 = 60;

/// The hundredths of a second in one second.
const CENTISECONDS_PER_SECOND: u64 = 100;

/// The milliseconds in one hundredth of a second.
const MILLIS_PER_CENTISECOND: u128 = 10;

/// CPU time in hundredths of a second, as `ps -o time` gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct CpuTime(u64);

impl CpuTime {
    /// Makes a CPU time from a count of hundredths of a second.
    #[must_use]
    pub const fn from_centiseconds(centiseconds: u64) -> Self {
        Self(centiseconds)
    }

    /// Gives this CPU time as a count of hundredths of a second.
    #[must_use]
    pub const fn centiseconds(self) -> u64 {
        self.0
    }

    /// Reads a CPU time from a token of `ps -o time`.
    ///
    /// The layout is `M:SS.cc`. The minutes are ASCII digits and have no
    /// upper bound. The seconds are exactly two digits below 60. The
    /// hundredths are exactly two digits. Any other text gives `None`, so a
    /// layout that the parser does not know never gives a wrong time.
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        let (minutes, rest) = token.split_once(MINUTES_SEPARATOR)?;
        let (seconds, hundredths) = rest.split_once(HUNDREDTHS_SEPARATOR)?;
        let minutes: u64 = parse_ps_number(minutes)?;
        let seconds: u64 = fixed_digits(seconds)?;
        let hundredths: u64 = fixed_digits(hundredths)?;
        if seconds >= SECONDS_PER_MINUTE {
            return None;
        }
        minutes
            .checked_mul(SECONDS_PER_MINUTE)?
            .checked_add(seconds)?
            .checked_mul(CENTISECONDS_PER_SECOND)?
            .checked_add(hundredths)
            .map(Self)
    }

    /// Takes `other` from this CPU time. The result stops at zero.
    #[must_use]
    pub fn saturating_sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }

    /// Gives this CPU time as a percent of one processor over `window`.
    ///
    /// The result is `(cs * 100 + w / 2) / w`, where `w` is the window in
    /// hundredths of a second. This rounds half up. A window of zero, or a
    /// window shorter than one hundredth of a second, gives 0.
    #[must_use]
    pub fn percent_of(self, window: Duration) -> u64 {
        let window = u64::try_from(window.as_millis() / MILLIS_PER_CENTISECOND).unwrap_or(u64::MAX);
        if window == 0 {
            return 0;
        }
        let scaled = u128::from(self.0) * 100 + u128::from(window / 2);
        u64::try_from(scaled / u128::from(window)).unwrap_or(u64::MAX)
    }
}

/// Reads exactly two ASCII digits.
fn fixed_digits(token: &str) -> Option<u64> {
    if token.len() == DIGITS_OF_A_FIELD {
        parse_ps_number(token)
    } else {
        None
    }
}

/// One process, as one line of the output of `ps`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessRow {
    /// The process.
    pub pid: Pid,
    /// The account that owns the process.
    pub uid: Uid,
    /// The five tokens of `lstart`, joined by one space. It is an identity
    /// of the process and nothing else: the code never reads it as a time.
    pub started: String,
    /// The CPU time that the process used since it started.
    pub cpu: CpuTime,
    /// The resident memory of the process, in KiB.
    pub rss_kib: u64,
    /// The `comm` column, which is `argv[0]` of the process as `ps` prints it.
    /// A process that `launchd` starts has the full path of its executable
    /// here. A process that a shell starts can have a bare name, such as
    /// `claude`. It can hold spaces.
    pub comm: String,
}

impl ProcessRow {
    /// Gives the name of the process: the last part of `comm`, without the
    /// `-` that marks a login shell. Thus `-zsh` and `zsh` are one group.
    /// A `comm` of `-` alone stays `-`.
    #[must_use]
    pub fn name(&self) -> &str {
        let base = proctree::comm_basename(&self.comm);
        match base.strip_prefix('-') {
            Some(rest) if !rest.is_empty() => rest,
            _ => base,
        }
    }
}

/// The reason why the output of `ps` is not a process table.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SampleParseError {
    /// The output holds no row. A Mac always runs processes, so the output is
    /// incomplete.
    #[error("ps listed no process: a Mac always runs processes, so the output is incomplete")]
    NoRows,
    /// A line holds fewer than nine tokens before the command name.
    #[error(
        "ps printed a row of fewer than {FIELDS} fields, at line {number}: {line:?}. A row holds \
         the PID, the UID, the five fields of the start time, the CPU time, and the \
         RSS, then the command name"
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
        /// The name of the column, as `ps -o` names it.
        column: &'static str,
        /// The number of the line in the output, from 1.
        number: usize,
        /// The line as `ps` printed it.
        line: String,
    },
    /// The CPU time is not in the layout `M:SS.cc`.
    #[error("ps printed a CPU time that is not in the layout M:SS.cc, at line {number}: {line:?}")]
    MalformedTime {
        /// The number of the line in the output, from 1.
        number: usize,
        /// The line as `ps` printed it.
        line: String,
    },
}

/// The column of the PID, as `ps -o` names it.
const PID_COLUMN: &str = "pid";

/// The column of the UID of the owner.
const UID_COLUMN: &str = "uid";

/// The column of the resident memory, in KiB.
const RSS_COLUMN: &str = "rss";

/// The count of tokens before the `comm` column.
const FIELDS: usize = 9;

/// Reads the process table from the output of `ps`.
///
/// Each line that is not blank is one row, in the order of `ps`. A row is nine
/// tokens, then the `comm` column. The tokens are the PID, the UID,
/// the five tokens of the start time, the CPU time, and the RSS. The parser
/// splits on ASCII white space, because `ps` pads a day of one digit with a
/// space. The `comm` column is the rest of the line. It keeps the spaces
/// inside it.
///
/// # Errors
///
/// The parser reads the lines in order, and the first line that fails gives
/// the error. In one line, it checks in this order: the count of tokens, the
/// PID, the UID, the RSS, and the CPU time. When every line
/// passes, [`SampleParseError::NoRows`] means that the output holds no row.
/// The parser fails closed: it never gives a table that lacks a row.
pub fn parse(output: &str) -> Result<Vec<ProcessRow>, SampleParseError> {
    let rows: Vec<ProcessRow> = output
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim_ascii().is_empty())
        .map(|(index, line)| parse_row(line).map_err(|fault| fault.at(index + 1, line)))
        .collect::<Result<_, _>>()?;
    if rows.is_empty() {
        return Err(SampleParseError::NoRows);
    }
    Ok(rows)
}

/// The reason why one line is not a row.
enum RowFault {
    /// The line holds fewer than [`FIELDS`] tokens.
    TooFewFields,
    /// The numeric column of this name holds a value that is not a number.
    MalformedNumber(&'static str),
    /// The CPU time is not in the layout `M:SS.cc`.
    MalformedTime,
}

impl RowFault {
    /// Gives the error of this fault at the line `line`, whose number is
    /// `number`.
    fn at(self, number: usize, line: &str) -> SampleParseError {
        let line = line.to_owned();
        match self {
            Self::TooFewFields => SampleParseError::TooFewFields { number, line },
            Self::MalformedNumber(column) => SampleParseError::MalformedNumber {
                column,
                number,
                line,
            },
            Self::MalformedTime => SampleParseError::MalformedTime { number, line },
        }
    }
}

/// Reads one row.
fn parse_row(line: &str) -> Result<ProcessRow, RowFault> {
    let ([pid, uid, weekday, month, day, clock, year, time, rss], comm) =
        split_fields::<FIELDS>(line).ok_or(RowFault::TooFewFields)?;
    let pid = Pid::new(number(PID_COLUMN, pid)?);
    let uid = parse_ps_uid(uid).ok_or(RowFault::MalformedNumber(UID_COLUMN))?;
    let rss_kib = number(RSS_COLUMN, rss)?;
    let cpu = CpuTime::parse(time).ok_or(RowFault::MalformedTime)?;
    Ok(ProcessRow {
        pid,
        uid,
        started: [weekday, month, day, clock, year].join(" "),
        cpu,
        rss_kib,
        comm: comm.to_owned(),
    })
}

/// Reads the value `token` of the numeric column `column`.
fn number<T: FromStr>(column: &'static str, token: &str) -> Result<T, RowFault> {
    parse_ps_number(token).ok_or(RowFault::MalformedNumber(column))
}

impl std::ops::Add for CpuTime {
    type Output = Self;

    /// Adds two times. The sum stops at the largest value.
    fn add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }
}

impl std::iter::Sum for CpuTime {
    fn sum<I: Iterator<Item = Self>>(times: I) -> Self {
        times.fold(Self::default(), |total, time| total + time)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A token of `M:SS.cc` gives hundredths of a second. The minutes are
    /// not bounded: a process that ran for 6 days shows `9086:33.94`.
    #[test]
    fn a_time_token_gives_hundredths_of_a_second() {
        for (token, centiseconds) in [
            ("0:00.05", 5),
            ("1:15.84", 7_584),
            ("9086:33.94", 54_519_394),
            ("0:00.00", 0),
            ("0:59.99", 5_999),
        ] {
            assert_eq!(
                CpuTime::parse(token),
                Some(CpuTime::from_centiseconds(centiseconds)),
                "token {token:?}"
            );
        }
    }

    /// A token that is not exactly `M:SS.cc` is refused. A wrong guess of
    /// the layout would give a wrong CPU time that looks right.
    #[test]
    fn a_token_that_is_not_a_time_is_refused() {
        for token in [
            "1:60.00",
            "1:5.00",
            "1:05.5",
            "1:05",
            ":05.00",
            "a:05.00",
            "-1:05.00",
            "",
            "1:05.000",
            "1:05.0a",
            "+1:05.00",
            "1:+5.00",
            "日:05.00",
            "1:05.00 ",
        ] {
            assert_eq!(CpuTime::parse(token), None, "token {token:?}");
        }
    }

    /// A time as a percent of one processor over a window. The result is
    /// rounded half up.
    #[test]
    fn a_time_gives_a_percent_of_one_processor() {
        let one_second = Duration::from_secs(1);
        let three_seconds = Duration::from_secs(3);
        assert_eq!(CpuTime::from_centiseconds(290).percent_of(one_second), 290);
        assert_eq!(CpuTime::from_centiseconds(1).percent_of(three_seconds), 0);
        assert_eq!(CpuTime::from_centiseconds(2).percent_of(three_seconds), 1);
    }

    /// A window of zero gives zero, and does not divide by zero. A window
    /// shorter than one hundredth of a second counts as zero too.
    #[test]
    fn a_zero_window_gives_zero_percent() {
        let time = CpuTime::from_centiseconds(500);
        assert_eq!(time.percent_of(Duration::ZERO), 0);
        assert_eq!(time.percent_of(Duration::from_millis(9)), 0);
    }

    /// The sum of times adds them, and the subtraction stops at zero.
    #[test]
    fn times_add_sum_and_subtract() {
        let a = CpuTime::from_centiseconds(30);
        let b = CpuTime::from_centiseconds(12);
        assert_eq!(a + b, CpuTime::from_centiseconds(42));
        assert_eq!(
            [a, b, a].into_iter().sum::<CpuTime>(),
            CpuTime::from_centiseconds(72)
        );
        assert_eq!(a.saturating_sub(b), CpuTime::from_centiseconds(18));
        assert_eq!(b.saturating_sub(a), CpuTime::from_centiseconds(0));
        assert_eq!(CpuTime::default().centiseconds(), 0);
    }

    /// The fixture before the one second wait, as `ps` printed it. The column
    /// of the parent was removed from each line after the capture.
    const BEFORE: &str = include_str!("../tests/fixtures/ps-before.txt");

    /// The fixture after the one second wait, as `ps` printed it. The column
    /// of the parent was removed from each line after the capture.
    const AFTER: &str = include_str!("../tests/fixtures/ps-after.txt");

    /// Finds the row of `pid` in `rows`.
    fn row_of(rows: &[ProcessRow], pid: u32) -> &ProcessRow {
        rows.iter()
            .find(|row| row.pid == Pid::new(pid))
            .expect("the fixture holds the row")
    }

    /// Each line of a real capture is a row, and the order stays. A parser
    /// that drops a row would make a process look as if it had exited.
    #[test]
    fn every_line_of_the_fixtures_is_a_row() {
        for text in [BEFORE, AFTER] {
            let rows = parse(text).expect("the capture parses");
            assert_eq!(rows.len(), text.lines().count());
            assert_eq!(rows.first().map(|row| row.pid), Some(Pid::new(1)));
        }
    }

    /// The row of `launchd` has each field as the file shows it. The day of
    /// the start time has a padding space in `Mon Nov  2`, and the joined
    /// start has one space.
    #[test]
    fn the_row_of_launchd_has_each_field() {
        let rows = parse(BEFORE).expect("the capture parses");
        assert_eq!(
            row_of(&rows, 1),
            &ProcessRow {
                pid: Pid::new(1),
                uid: Uid::new(0),
                started: "Wed Oct 28 21:29:02 2026".to_owned(),
                cpu: CpuTime::from_centiseconds(1_797_047),
                rss_kib: 14_288,
                comm: "/sbin/launchd".to_owned(),
            }
        );
        assert_eq!(row_of(&rows, 1).name(), "launchd");
        assert_eq!(row_of(&rows, 56_844).started, "Mon Nov 2 01:37:25 2026");
    }

    /// A path with spaces stays whole, and its name is the last part.
    #[test]
    fn a_comm_with_spaces_stays_whole() {
        let rows = parse(BEFORE).expect("the capture parses");
        let row = row_of(&rows, 569);
        assert_eq!(
            row.comm,
            "/Applications/Menu Bar Controller for Sonos.app/Contents/MacOS/Menu Bar Controller for Sonos"
        );
        assert_eq!(row.name(), "Menu Bar Controller for Sonos");
    }

    /// The capture holds both human accounts of this Mac, and the accounts
    /// of the system.
    #[test]
    fn the_fixtures_hold_each_kind_of_account() {
        let rows = parse(AFTER).expect("the capture parses");
        for uid in [0, 88, 501, 503] {
            assert!(rows.iter().any(|row| row.uid == Uid::new(uid)), "uid {uid}");
        }
    }

    /// A row of `ps` text with the given tokens.
    fn line(pid: &str, uid: &str, time: &str, rss: &str) -> String {
        format!("{pid:>5} {uid:>5} Tue Aug 25 16:45:30 2026 {time:>12} {rss:>6} /usr/bin/tool")
    }

    /// A good row.
    fn good() -> String {
        line("700", "501", "0:01.00", "2048")
    }

    /// Output with `bad` at line 3, after a blank line.
    fn with_bad_line_at_3(bad: &str) -> String {
        format!("{}\n\n{bad}\n{}\n", good(), good())
    }

    /// An output with no row is refused, because an empty table makes each
    /// process of a sample look as if it had exited.
    #[test]
    fn an_output_with_no_row_is_refused() {
        for text in ["", "\n", "  \n\t\n"] {
            assert_eq!(parse(text), Err(SampleParseError::NoRows), "text {text:?}");
        }
    }

    /// A line of fewer than nine tokens is refused, with its number from 1.
    #[test]
    fn a_short_line_is_refused_with_its_number() {
        for bad in [
            "  700     1   501",
            "日本語 🎉 café",
            "700 501 Tue Aug 25 16:45:30 2026 0:01.00",
        ] {
            assert_eq!(
                parse(&with_bad_line_at_3(bad)),
                Err(SampleParseError::TooFewFields {
                    number: 3,
                    line: bad.to_owned()
                }),
                "line {bad:?}"
            );
        }
    }

    /// A bad PID, UID or RSS is refused, and the error names the
    /// column.
    #[test]
    fn a_bad_number_is_refused_with_its_column() {
        let cases = [
            ("pid", line("x", "501", "0:01.00", "2048")),
            ("uid", line("700", "+5", "0:01.00", "2048")),
            ("rss", line("700", "501", "0:01.00", "2k")),
        ];
        for (column, bad) in cases {
            assert_eq!(
                parse(&with_bad_line_at_3(&bad)),
                Err(SampleParseError::MalformedNumber {
                    column,
                    number: 3,
                    line: bad.clone()
                }),
                "column {column}"
            );
        }
    }

    /// A row is the six columns of `ps`: the PID, the UID, the five tokens
    /// of the start, the CPU time, the RSS, and the `comm` column.
    #[test]
    fn a_row_is_the_six_columns_of_ps() {
        let text = line("700", "501", "1:02.03", "2048");
        let rows = parse(&text).expect("the row parses");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pid, Pid::new(700));
        assert_eq!(rows[0].uid, Uid::new(501));
        assert_eq!(rows[0].cpu, CpuTime::from_centiseconds(6_203));
        assert_eq!(rows[0].rss_kib, 2048);
        assert_eq!(rows[0].comm, "/usr/bin/tool");
    }

    /// `ps` prints `argv[0]` in the `comm` column, and a login shell has a `-`
    /// in front of it. `-zsh` and `zsh` thus must give one name, and a bare
    /// `-` must stay as it is.
    #[test]
    fn a_login_shell_and_a_shell_have_one_name() {
        let name_of = |comm: &str| {
            let text = format!("  700   501 Tue Aug 25 16:45:30 2026 0:01.00 2048 {comm}");
            let rows = parse(&text).expect("the row parses");
            rows[0].name().to_owned()
        };
        for comm in ["-zsh", "zsh", "/bin/zsh", "-/bin/zsh"] {
            assert_eq!(name_of(comm), "zsh", "comm {comm:?}");
        }
        assert_eq!(name_of("-"), "-");
    }

    /// A CPU time that is not `M:SS.cc` is refused with its line.
    #[test]
    fn a_bad_time_is_refused_with_its_line() {
        for time in ["1:60.00", "1:05", "abc", "1:05.5"] {
            let bad = line("700", "501", time, "2048");
            assert_eq!(
                parse(&with_bad_line_at_3(&bad)),
                Err(SampleParseError::MalformedTime {
                    number: 3,
                    line: bad.clone()
                }),
                "time {time:?}"
            );
        }
    }

    /// The first bad line gives the error, also when a later line is bad
    /// in another way.
    #[test]
    fn the_first_bad_line_wins() {
        let first = line("x", "501", "0:01.00", "2048");
        let second = line("700", "501", "bad", "2048");
        let text = format!("{}\n{first}\n{second}\n", good());
        assert_eq!(
            parse(&text),
            Err(SampleParseError::MalformedNumber {
                column: "pid",
                number: 2,
                line: first
            })
        );
    }

    /// The command is exact: every process of every account, no limit on the
    /// width, and the six columns that the parser reads.
    #[test]
    fn the_arguments_ask_for_every_process_with_the_columns_of_a_row() {
        assert_eq!(PROGRAM, "/bin/ps");
        assert_eq!(
            arguments(),
            ["-A", "-ww", "-o", "pid=,uid=,lstart=,time=,rss=,comm="]
        );
        assert_eq!(ENVIRONMENT, [("TZ", "UTC"), ("LC_ALL", "en_US.UTF-8")]);
    }

    /// A row of PID `pid` that started at `started` and used `cs` hundredths.
    fn process(pid: u32, started: &str, cs: u64) -> ProcessRow {
        ProcessRow {
            pid: Pid::new(pid),
            uid: Uid::new(501),
            started: started.to_owned(),
            cpu: CpuTime::from_centiseconds(cs),
            rss_kib: 100,
            comm: "/usr/bin/tool".to_owned(),
        }
    }

    /// Gives the used time of each value, as hundredths.
    fn centiseconds(times: &[CpuTime]) -> Vec<u64> {
        times.iter().map(|time| time.centiseconds()).collect()
    }

    /// A process in both snapshots with the same identity gives the
    /// difference of its two CPU times.
    #[test]
    fn a_process_in_both_snapshots_gives_the_difference() {
        let before = [process(10, "A", 100), process(11, "B", 50)];
        let after = [process(10, "A", 390), process(11, "B", 50)];
        assert_eq!(centiseconds(&cpu_used(&before, &after)), [290, 0]);
    }

    /// A process that is only in the second snapshot started in the window,
    /// and it gives all of its CPU time.
    #[test]
    fn a_new_process_gives_all_of_its_time() {
        let before = [process(10, "A", 100)];
        let after = [process(10, "A", 100), process(12, "C", 77)];
        assert_eq!(centiseconds(&cpu_used(&before, &after)), [0, 77]);
    }

    /// A process that is only in the first snapshot gives no value. The
    /// result has one value for each row of the second snapshot, in order.
    #[test]
    fn an_exited_process_gives_no_value() {
        let before = [process(9, "Z", 500), process(10, "A", 100)];
        let after = [process(11, "B", 5), process(10, "A", 130)];
        assert_eq!(centiseconds(&cpu_used(&before, &after)), [5, 30]);
        assert!(cpu_used(&before, &[]).is_empty());
    }

    /// A PID with another start time is a new process that reuses the PID.
    /// It gives all of its CPU time, and not the difference with the old one.
    #[test]
    fn a_reused_pid_is_a_new_process() {
        let before = [process(10, "Mon Nov 2 01:00:00 2026", 900)];
        let after = [process(10, "Tue Nov 3 02:00:00 2026", 40)];
        assert_eq!(centiseconds(&cpu_used(&before, &after)), [40]);
    }

    /// A counter that went down gives 0, and does not wrap around.
    #[test]
    fn a_counter_that_went_down_gives_zero() {
        let before = [process(10, "A", 500)];
        let after = [process(10, "A", 300)];
        assert_eq!(centiseconds(&cpu_used(&before, &after)), [0]);
    }

    /// The sum over the two fixture files. Each of the 26 processes is in
    /// both files with the same start time, so each value is the difference
    /// of two CPU times. The CPU times changed for 10 processes, in
    /// hundredths of a second: 1, 1, 11, 5, 14, 1, 1, 1, 2 and 1, which is
    /// 38. The check by hand was `awk` over the time column of each file:
    /// the sum of the second file (53 507 317) minus the sum of the first
    /// (53 507 279).
    #[test]
    fn the_fixtures_give_the_sum_that_was_computed_by_hand() {
        let before = parse(BEFORE).expect("the capture parses");
        let after = parse(AFTER).expect("the capture parses");
        let used = cpu_used(&before, &after);
        assert_eq!(used.len(), after.len());
        assert_eq!(
            used.into_iter().sum::<CpuTime>(),
            CpuTime::from_centiseconds(38)
        );
    }
}
