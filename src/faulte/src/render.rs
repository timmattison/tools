//! The text that a person reads: the header lines, the table of rows, and the
//! small formatters that both of them use.
//!
//! Every function here gives text back. None of them prints. Thus a test reads
//! exactly what a person sees, and the caller decides where the text goes.
//!
//! The text carries no color and no attribute of a terminal. A number that a
//! reader must compare with another number is the whole product of this tool,
//! so each formatter states one rule and has its own tests.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use comfy_table::{presets, ContentArrangement, Table};

use crate::duration::Span;
use crate::pid::Uid;
use crate::ranking::{ClaudeView, RankedRow, Ranking};
use crate::vm::{SwapUsage, VmDelta};

/// The text in place of a value that `faulte` could not read.
///
/// One text for every absent value, the same as `occ`. A reader then learns
/// the mark once.
pub const ABSENT: &str = "—";

/// What stands between two groups of three digits.
const GROUP_SEPARATOR: char = ',';

/// Writes the digits of `digits` in groups of three, from the right.
///
/// The input is the text of a number, and not a number, because a rate is a
/// `f64`. A cast from `f64` to an integer truncates a value that does not fit,
/// and `format!` with no decimal gives the digits of any value.
fn separate(digits: &str) -> String {
    let (sign, digits) = match digits.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", digits),
    };
    let places = digits.chars().count();
    let mut grouped = String::with_capacity(sign.len() + places + places / 3);
    grouped.push_str(sign);
    for (place, digit) in digits.chars().enumerate() {
        if place > 0 && (places - place) % 3 == 0 {
            grouped.push(GROUP_SEPARATOR);
        }
        grouped.push(digit);
    }
    grouped
}

/// Gives `value` with a separator between each group of three digits, for
/// example `959,815`.
///
/// A count of page faults has six or seven digits on a Mac that is short of
/// memory. The groups let a reader compare two such counts at a glance.
#[must_use]
pub fn count(value: u64) -> String {
    separate(&value.to_string())
}

/// Gives `value`, a count of things, in groups of three digits.
///
/// A count of processes is a `usize`. The text of the number is the same
/// whatever the width of a `usize` on the platform, so nothing here casts one
/// number to another.
fn count_of(value: usize) -> String {
    separate(&value.to_string())
}

/// The rate below which a rate carries one decimal.
const SMALLEST_WHOLE_RATE: f64 = 10.0;

/// Gives `per_second` as a rate of page faults, for example `0.4` or
/// `239,953`.
///
/// A rate below ten carries one decimal, because the whole part of such a rate
/// says almost nothing. A rate of ten or more is a whole number with a
/// separator between each group of three digits, because the decimal of a
/// large rate says nothing at all.
///
/// A rate that is not a number gives [`ABSENT`]. The window of the ranking can
/// be no time, and a division by no time gives no number.
#[must_use]
pub fn rate(per_second: f64) -> String {
    if !per_second.is_finite() {
        return ABSENT.to_owned();
    }
    if per_second < SMALLEST_WHOLE_RATE {
        return format!("{per_second:.1}");
    }
    separate(&format!("{per_second:.0}"))
}

/// The text of a share of nothing.
const NO_SHARE: &str = "0%";

/// The text of a share of everything.
const WHOLE_SHARE: &str = "100%";

/// The text of a share that is above nothing and below the smallest share
/// that one decimal shows.
const BELOW_SMALLEST_SHARE: &str = "<0.1%";

/// The text of a share that is below everything and above the largest share
/// that one decimal shows.
const ABOVE_LARGEST_SHARE: &str = ">99.9%";

/// One decimal of a share of nothing.
const ROUNDED_NOTHING: &str = "0.0";

/// One decimal of a share of everything.
const ROUNDED_EVERYTHING: &str = "100.0";

/// Gives `fraction`, a value from 0 to 1, as a share in percent, for example
/// `5.2%`.
///
/// A share that is not zero and that rounds to zero gives `<0.1%`, and a share
/// that is not everything and that rounds to everything gives `>99.9%`. On
/// 2026-09-18, 213 sessions made 90.2% of all faults, and each one of them
/// made a share that one decimal shows as zero. A row that says `0.0%` about a
/// process which made 23,000 faults hides the cause of the load.
///
/// A share that is not a number gives [`ABSENT`].
#[must_use]
pub fn share(fraction: f64) -> String {
    if !fraction.is_finite() {
        return ABSENT.to_owned();
    }
    if fraction <= 0.0 {
        return NO_SHARE.to_owned();
    }
    if fraction >= 1.0 {
        return WHOLE_SHARE.to_owned();
    }
    let percent = format!("{:.1}", fraction * 100.0);
    if percent == ROUNDED_NOTHING {
        return BELOW_SMALLEST_SHARE.to_owned();
    }
    if percent == ROUNDED_EVERYTHING {
        return ABOVE_LARGEST_SHARE.to_owned();
    }
    format!("{percent}%")
}

/// The step from one unit of size to the next.
const SIZE_STEP: u64 = 1_024;

/// The name of each unit of size, from the smallest up.
const SIZE_UNITS: [&str; 7] = ["B", "KB", "MB", "GB", "TB", "PB", "EB"];

/// The place in [`SIZE_UNITS`] of the first unit that carries one decimal.
const FIRST_UNIT_WITH_A_DECIMAL: usize = 3;

/// Gives `value` bytes as a size, for example `812 MB` or `9.3 GB`.
///
/// The step from one unit to the next is 1,024, the step that a Mac reports.
/// A size of a gigabyte or more carries one decimal, because a whole number of
/// gigabytes hides a difference of hundreds of megabytes. A smaller size is a
/// whole number, because the decimal of a size in kilobytes says nothing.
#[must_use]
pub fn bytes(value: u64) -> String {
    let mut unit = 0;
    let mut scale = 1_u64;
    while value / scale >= SIZE_STEP && unit + 1 < SIZE_UNITS.len() {
        scale *= SIZE_STEP;
        unit += 1;
    }
    let name = SIZE_UNITS[unit];
    if unit < FIRST_UNIT_WITH_A_DECIMAL {
        // The loop stops at the last unit, so the whole part is below 1,024
        // under every other unit, and 16 under the last one. No such number
        // needs a separator.
        return format!("{} {name}", value / scale);
    }
    format!("{:.1} {name}", value as f64 / scale as f64)
}

/// Gives `value` kibibytes as a size, for example `42 KB`.
///
/// `ps` gives the resident memory of a process in kibibytes. A size that no
/// memory can hold gives the largest size, and not a panic.
#[must_use]
pub fn kibibytes(value: u64) -> String {
    bytes(value.saturating_mul(SIZE_STEP))
}

/// Gives the age of a process that started at `started_at_epoch_secs`, at the
/// time `now`, for example `3h 12m`.
///
/// The format is the format of `occ`, so the two tools print an age the same
/// way. A process with no start time gives [`ABSENT`]: `ps` does not list the
/// kernel, so the row of PID 0 has no start time, and a guess there is a
/// number that a reader cannot tell from a measured one.
///
/// A start time after `now` gives an age of no time. The clock of this Mac can
/// move back between the read of the table and the read of `now`.
#[must_use]
pub fn age(started_at_epoch_secs: Option<u64>, now: SystemTime) -> String {
    let Some(started) = started_at_epoch_secs else {
        return ABSENT.to_owned();
    };
    let now_secs = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    occ::format_uptime(now_secs.saturating_sub(started))
}

/// The name of each account that runs a process in the ranking.
///
/// The table shows the owner of a process by name, because a reader knows the
/// accounts of this Mac by name and not by number. The caller reads the names
/// with `getpwuid_r`, which answers for some accounts and not for others, so
/// this map holds the names that it got and gives the bare number for the
/// rest. A number is never wrong, and a wrong name is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Accounts {
    /// The name of each account that the caller could read.
    names: HashMap<Uid, String>,
}

impl Accounts {
    /// Gives a map with no name in it. Every account then shows its number.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Gives this map, with `name` as the name of `uid`.
    #[must_use]
    pub fn with(mut self, uid: Uid, name: impl Into<String>) -> Self {
        self.names.insert(uid, name.into());
        self
    }

    /// Gives the name of `uid`, or the number of `uid` when the map has no
    /// name for it.
    #[must_use]
    pub fn name_of(&self, uid: Uid) -> String {
        self.names
            .get(&uid)
            .cloned()
            .unwrap_or_else(|| uid.to_string())
    }
}

impl FromIterator<(Uid, String)> for Accounts {
    /// Collects a name for each account. A later name of one account replaces
    /// an earlier one.
    fn from_iter<I: IntoIterator<Item = (Uid, String)>>(names: I) -> Self {
        Self {
            names: names.into_iter().collect(),
        }
    }
}

/// What holds the two halves of a header line apart.
const DASH: &str = "—";

/// What holds two measurements of one header line apart.
const DOT: &str = " · ";

/// The singular of the word for one entry of the ranking.
const PROCESS: &str = "process";

/// The plural of the word for one entry of the ranking.
const PROCESSES: &str = "processes";

/// Chooses the singular or the plural form for `count`.
fn plural<'a>(count: usize, singular: &'a str, many: &'a str) -> &'a str {
    if count == 1 {
        singular
    } else {
        many
    }
}

/// Gives `span` in seconds with one decimal, for example `4.0 s`.
///
/// The window of a sample is a measured time and not the time that the person
/// asked for. On a loaded Mac the two samples of `top` were 4 seconds apart
/// when the interval was 2 seconds, so the decimal is the difference between a
/// rate and a guess.
fn seconds(span: Duration) -> String {
    format!("{:.1} s", span.as_secs_f64())
}

/// Everything that the header lines give, other than the ranking.
///
/// `faulte` reads these numbers from the system, around the run of `top`. The
/// ranking says which processes made the faults, and these numbers say what
/// the memory of this Mac did over the same time.
#[derive(Debug, Clone, Copy)]
pub struct Measurement<'a> {
    /// The ranking of the same sample.
    pub ranking: &'a Ranking,
    /// The swap traffic over [`Measurement::swap_window`].
    pub swap: VmDelta,
    /// The swap file of this Mac, from `vm.swapusage`.
    pub usage: SwapUsage,
    /// The size of the compressor in bytes.
    pub compressor_bytes: u64,
    /// The time between the two reads of the counters of the system. It is
    /// longer than the window of the ranking, because `top` starts and stops
    /// inside it.
    pub swap_window: Duration,
    /// The interval that the person asked for.
    pub interval: Span,
}

/// Gives the lines above the table.
///
/// The header says what the whole Mac did over the sample, because no single
/// row of the table says it. On 2026-09-18, 213 Claude Code sessions made 90%
/// of all page faults, and the largest single row made a small part of that.
///
/// A line, or a part of a line, with a count of zero is not there. A Mac that
/// hides nothing says nothing, the same as `occ`.
#[must_use]
pub fn header(measurement: &Measurement<'_>) -> Vec<String> {
    let ranking = measurement.ranking;
    let processes = ranking.rows.len();
    let mut lines = vec![
        format!(
            "{} {} over a {} window (interval {}) {DASH} {} faults, {}/s",
            count_of(processes),
            plural(processes, PROCESS, PROCESSES),
            seconds(ranking.window),
            measurement.interval,
            count(ranking.total_faults),
            rate(ranking.faults_per_second(ranking.total_faults)),
        ),
        format!(
            "swap: {} in, {} out in {}{DOT}compressor {}{DOT}swap in use {} of {}",
            count(measurement.swap.swapins),
            count(measurement.swap.swapouts),
            seconds(measurement.swap_window),
            bytes(measurement.compressor_bytes),
            bytes(measurement.usage.used_bytes),
            bytes(measurement.usage.total_bytes),
        ),
        claude_line(ranking),
    ];
    lines.extend(skipped_line(ranking));
    lines
}

/// Gives the line of the processes that the ranking left out, or nothing when
/// it left nothing out.
///
/// The issue demands that each process is a row or a count. A tool that drops
/// a process without a word reports a clean Mac that is not clean, so each
/// count is here. A count of zero adds no part, and a Mac that hides nothing
/// says nothing, the same as `occ`.
fn skipped_line(ranking: &Ranking) -> Option<String> {
    let skipped = ranking.skipped;
    let mut parts: Vec<String> = Vec::new();
    if skipped.exited > 0 {
        parts.push(format!(
            "{} exited before faulte read {} ({} of the faults)",
            count_of(skipped.exited),
            plural(skipped.exited, "it", "them"),
            share(ranking.share(skipped.exited_faults)),
        ));
    }
    if skipped.zombies > 0 {
        parts.push(format!(
            "{} {}",
            count_of(skipped.zombies),
            plural(skipped.zombies, "zombie", "zombies"),
        ));
    }
    if skipped.unsampled > 0 {
        parts.push(format!(
            "{} {} not in the top sample",
            count_of(skipped.unsampled),
            plural(skipped.unsampled, "was", "were"),
        ));
    }
    if parts.is_empty() {
        return None;
    }
    Some(format!("skipped: {}", parts.join(", ")))
}

/// The singular of the word for one Claude Code process of the ranking.
const SESSION: &str = "session";

/// The plural of the word for one Claude Code process of the ranking.
const SESSIONS: &str = "sessions";

/// Gives the line of the total of Claude Code.
///
/// The issue demands this line. On 2026-09-18, 213 sessions made 90% of all
/// page faults on this Mac, the median one of them made 23,000 faults in 20
/// seconds, and no single row said what the 213 of them did together.
fn claude_line(ranking: &Ranking) -> String {
    let total = ranking.claude;
    if total.processes == 0 {
        return format!("Claude: no {SESSION} is running");
    }
    // A count of zero adds nothing. The parenthesis is there to say that some
    // of the sessions are out of reach, and `(none of another account)` says
    // that about no session at all.
    let other_account = if total.other_account == 0 {
        String::new()
    } else {
        format!(" ({} of another account)", count_of(total.other_account))
    };
    format!(
        "Claude: {} {} made {} of all faults{other_account}",
        count_of(total.processes),
        plural(total.processes, SESSION, SESSIONS),
        share(ranking.share(total.faults)),
    )
}

/// The name of each column of the table, in the order that the issue lists
/// them.
const COLUMNS: [&str; 10] = [
    "PID",
    "OWNER",
    "FAULTS/S",
    "SHARE",
    "RSS",
    "AGE",
    "COMMAND",
    "SESSION",
    "STATE",
    "DIRECTORY",
];

/// What the `SESSION` column gives for a Claude Code process of another
/// account.
const OTHER_ACCOUNT: &str = "other account — run with sudo";

/// The width of the table when nothing states a width and no terminal answers.
///
/// A pipe and a file carry no window. The table holds ten columns, so it needs
/// room, and a width of no columns makes each column as wide as its widest
/// cell.
pub const DEFAULT_WIDTH: u16 = 120;

/// The width that the table takes.
///
/// `stated` is the value of `COLUMNS`, and it wins. POSIX says a value there
/// overrides the width that the system selects, and `ls`, `git` and `less`
/// obey that rule. `terminal` is the width that the terminal reports, which is
/// absent for a pipe and for a file. [`DEFAULT_WIDTH`] answers when neither
/// source does, so the table is always bounded.
#[must_use]
pub fn table_width(stated: Option<&str>, terminal: Option<u16>) -> u16 {
    DEFAULT_WIDTH
}

/// The greatest number of characters that the `COMMAND` column gives.
///
/// A ranking names the process that faults, and the first characters of a
/// command name it. A command line can be very long: one process of this Mac
/// carries more than 3,000 characters, and a table that gives such a command
/// whole is as wide as that command.
pub const COMMAND_LIMIT: usize = 120;

/// Gives the command of a row, cut at [`COMMAND_LIMIT`] characters.
///
/// The cut counts characters, never bytes. A cut inside one character panics,
/// and the command comes from another process, so a cut by bytes stops
/// `faulte` at the first such process on the Mac. A command that the cut made
/// shorter ends with [`MORE`], so a reader sees that the row gives a part.
#[must_use]
pub fn command(text: &str) -> String {
    let mut cut: String = text.chars().take(COMMAND_LIMIT).collect();
    if text.chars().nth(COMMAND_LIMIT).is_some() {
        cut.push(MORE);
    }
    cut
}

/// Gives the table of `rows`, and the line that counts the rows past `limit`.
///
/// `rows` is the order that the caller wants. The ranking sorts its rows, and
/// the plan of `faulte kill` gives its own order, so nothing here sorts
/// anything. `ranking` gives the window and the total that each rate and each
/// share divide by, and it can hold rows that `rows` does not.
///
/// `limit` keeps the first N rows. One line under the table then counts the
/// rest and gives their share of all faults, so a reader learns what the
/// limit hid. `None` shows every row.
///
/// `width` wraps the table at that many columns. `None` makes each column as
/// wide as its widest cell, which is what a test wants and what a pipe wants.
/// The caller reads the width of the terminal. A read of the terminal here
/// makes the text of this function depend on where the tool runs, and a test
/// that compares text then passes through a pipe and fails on a terminal.
#[must_use]
pub fn rows(
    ranking: &Ranking,
    rows: &[RankedRow],
    limit: Option<usize>,
    accounts: &Accounts,
    now: SystemTime,
    width: Option<u16>,
) -> String {
    let shown = limit.map_or(rows, |limit| &rows[..limit.min(rows.len())]);
    let mut table = Table::new();
    table
        .load_preset(presets::UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        // The table must not read the terminal itself. `comfy_table` reads it
        // through `crossterm::terminal::size`, which this workspace bans: a
        // terminal that carries no window answers that call with zero columns.
        .force_no_tty()
        .set_header(COLUMNS);
    if let Some(width) = width {
        table.set_width(width);
    }
    for row in shown {
        let (session, state, directory) = claude_cells(&row.claude);
        table.add_row([
            row.pid.to_string(),
            accounts.name_of(row.uid),
            rate(ranking.faults_per_second(row.faults)),
            share(ranking.share(row.faults)),
            row.rss_kib.map_or_else(|| ABSENT.to_owned(), kibibytes),
            age(row.started_at_epoch_secs, now),
            command(&row.command),
            session,
            state,
            directory,
        ]);
    }
    let drawn = table.to_string();
    match more_line(ranking, &rows[shown.len()..]) {
        Some(line) => format!("{drawn}\n{line}"),
        None => drawn,
    }
}

/// What the line under the table starts with.
const MORE: char = '…';

/// Gives the line that counts the rows which the limit hid, or nothing when it
/// hid no row.
///
/// The limit hides the small rates, and on 2026-09-18 the sum of the small
/// rates was the load. A reader must see what the limit took away, so the line
/// gives the share of all faults that those rows made.
fn more_line(ranking: &Ranking, hidden: &[RankedRow]) -> Option<String> {
    if hidden.is_empty() {
        return None;
    }
    let faults = hidden
        .iter()
        .fold(0_u64, |total, row| total.saturating_add(row.faults));
    Some(format!(
        "{MORE} {} more {} made {} of all faults",
        count_of(hidden.len()),
        plural(hidden.len(), PROCESS, PROCESSES),
        share(ranking.share(faults)),
    ))
}

/// Gives the `SESSION`, the `STATE`, and the `DIRECTORY` cells of `claude`.
///
/// A process that is not a Claude Code session gives three empty cells. A
/// session with no registry record gives three empty cells too: the issue
/// demands that such a row shows no session, because a guess names the wrong
/// session and nothing in the text says that it is a guess.
fn claude_cells(claude: &ClaudeView) -> (String, String, String) {
    match claude {
        ClaudeView::NotClaude | ClaudeView::NoRecord => {
            (String::new(), String::new(), String::new())
        }
        // The account is the reason why the state and the directory are
        // absent, so the one cell that says so is the session cell.
        ClaudeView::OtherAccount => (OTHER_ACCOUNT.to_owned(), String::new(), String::new()),
        ClaudeView::Session {
            id,
            state,
            directory,
        } => (
            id.to_string(),
            state.to_string(),
            directory
                .as_ref()
                .map_or_else(|| ABSENT.to_owned(), |path| path.display().to_string()),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use occ::SessionId;

    use crate::pid::Pid;
    use crate::ranking::{ClaudeTotal, ClaudeView, Skipped};
    use crate::state::SessionState;

    /// A count carries a separator between each group of three digits, from
    /// the right. A count below a thousand carries none.
    #[test]
    fn a_count_gets_a_separator_between_each_group_of_three_digits() {
        let cases = [
            (0, "0"),
            (1, "1"),
            (999, "999"),
            (1_000, "1,000"),
            (1_549, "1,549"),
            (12_345, "12,345"),
            (959_815, "959,815"),
            (1_000_000, "1,000,000"),
            (u64::MAX, "18,446,744,073,709,551,615"),
        ];

        for (value, text) in cases {
            assert_eq!(count(value), text, "the count {value}");
        }
    }

    /// A rate below ten carries one decimal. A rate of ten or more is a whole
    /// number in groups of three digits. A rate that is not a number gives the
    /// text of an absent value.
    #[test]
    fn a_rate_below_ten_carries_one_decimal_and_a_larger_rate_is_whole() {
        let cases = [
            (0.0, "0.0"),
            (0.4, "0.4"),
            (0.75, "0.8"),
            (9.7, "9.7"),
            (10.0, "10"),
            (10.4, "10"),
            (999.0, "999"),
            (1_000.0, "1,000"),
            (239_952.6, "239,953"),
            (f64::INFINITY, ABSENT),
            (f64::NAN, ABSENT),
        ];

        for (value, text) in cases {
            assert_eq!(rate(value), text, "the rate {value}");
        }
    }

    /// A share carries one decimal. Nothing and everything carry none. A share
    /// that is above nothing and rounds to nothing says so, and a share that
    /// is below everything and rounds to everything says so.
    #[test]
    fn a_share_that_is_not_zero_never_prints_as_zero() {
        let cases = [
            (0.0, "0%"),
            (1.0, "100%"),
            (0.052, "5.2%"),
            (0.902, "90.2%"),
            (0.004, "0.4%"),
            (0.001, "0.1%"),
            (0.0004, "<0.1%"),
            (0.000_001, "<0.1%"),
            (0.9999, ">99.9%"),
            (f64::NAN, ABSENT),
            (f64::INFINITY, ABSENT),
        ];

        for (fraction, text) in cases {
            assert_eq!(share(fraction), text, "the share {fraction}");
        }
    }

    /// A size steps by 1,024. A size of a gigabyte or more carries one
    /// decimal, and a smaller size is a whole number. The largest size is the
    /// last unit, and no size gives a panic.
    #[test]
    fn a_size_steps_by_1024_and_carries_a_decimal_from_a_gigabyte_up() {
        let cases = [
            (0, "0 B"),
            (1, "1 B"),
            (1_023, "1023 B"),
            (1_024, "1 KB"),
            (43_008, "42 KB"),
            (1_048_575, "1023 KB"),
            (1_048_576, "1 MB"),
            (851_443_712, "812 MB"),
            (1_073_741_824, "1.0 GB"),
            (9_985_798_963, "9.3 GB"),
            (11_811_160_064, "11.0 GB"),
            (28_991_029_248, "27.0 GB"),
            (u64::MAX, "16.0 EB"),
        ];

        for (value, text) in cases {
            assert_eq!(bytes(value), text, "the size of {value} bytes");
        }
    }

    /// A size in kibibytes is the same size, 1,024 times larger. A count of
    /// kibibytes that no memory can hold gives the largest size, and not a
    /// panic.
    #[test]
    fn a_size_in_kibibytes_is_the_same_size_1024_times_larger() {
        let cases = [
            (0, "0 B"),
            (1, "1 KB"),
            (42, "42 KB"),
            (1_023, "1023 KB"),
            (1_024, "1 MB"),
            (831_488, "812 MB"),
            (9_752_733, "9.3 GB"),
            (u64::MAX, "16.0 EB"),
        ];

        for (value, text) in cases {
            assert_eq!(kibibytes(value), text, "the size of {value} kibibytes");
        }
    }

    /// The age is the time from the start of the process to now, in the format
    /// of `occ`. A process with no start time gives the text of an absent
    /// value, and a start time after now gives no time.
    #[test]
    fn the_age_is_the_time_from_the_start_of_the_process_to_now() {
        let started = 1_780_000_000;
        let cases = [
            (None, ABSENT),
            (Some(started), "0s"),
            (Some(started + 5), "0s"),
            (Some(started - 45), "45s"),
            (Some(started - (3 * 3_600 + 12 * 60 + 59)), "3h 12m"),
            (Some(started - (2 * 86_400 + 5 * 3_600)), "2d 5h"),
        ];
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(started);

        for (started_at_epoch_secs, text) in cases {
            assert_eq!(
                age(started_at_epoch_secs, now),
                text,
                "the start {started_at_epoch_secs:?}"
            );
        }
    }

    /// An account shows its name. An account that the caller read no name for
    /// shows its bare number, because a number is never wrong and a guessed
    /// name is.
    #[test]
    fn an_account_shows_its_name_and_a_bare_number_when_it_has_none() {
        let accounts = Accounts::new()
            .with(Uid::new(501), "tim")
            .with(Uid::new(0), "root");

        assert_eq!(accounts.name_of(Uid::new(501)), "tim");
        assert_eq!(accounts.name_of(Uid::new(0)), "root");
        assert_eq!(accounts.name_of(Uid::new(502)), "502");
        assert_eq!(Accounts::new().name_of(Uid::new(501)), "501");

        let collected: Accounts = [(Uid::new(502), "work".to_owned())].into_iter().collect();
        assert_eq!(collected.name_of(Uid::new(502)), "work");
        assert_eq!(collected.name_of(Uid::new(501)), "501");
    }

    /// The window of the ranking in the tests. On 2026-09-18 the two samples
    /// of `top` were 4 seconds apart.
    const WINDOW: Duration = Duration::from_secs(4);

    /// The time that the counters of the system cover in the tests. It is
    /// longer than [`WINDOW`], because `top` starts and stops inside it.
    const SWAP_WINDOW: Duration = Duration::from_millis(5_200);

    /// The UID of the account that runs the tool in the tests.
    const VIEWER_UID: u32 = 501;

    /// The time when each process of the tests started, in seconds since the
    /// Unix epoch.
    const STARTED: u64 = 1_780_000_000;

    /// Gives the interval that the person asked for in the tests.
    fn interval() -> Span {
        "5s".parse().expect("5s is a span")
    }

    /// Gives the row of a process that is not Claude Code.
    fn row(pid: u32, faults: u64) -> RankedRow {
        RankedRow {
            pid: Pid::new(pid),
            uid: Uid::new(VIEWER_UID),
            faults,
            rss_kib: Some(831_488),
            started_at_epoch_secs: Some(STARTED),
            command: format!("/usr/bin/process-{pid} --flag"),
            claude: ClaudeView::NotClaude,
        }
    }

    /// Gives a ranking of `rows` that made `total_faults` over [`WINDOW`].
    fn ranking(rows: Vec<RankedRow>, total_faults: u64) -> Ranking {
        Ranking {
            rows,
            window: WINDOW,
            total_faults,
            claude: ClaudeTotal::default(),
            skipped: Skipped::default(),
        }
    }

    /// Gives the measurement of a Mac whose ranking is `ranking`. The swap
    /// numbers are the ones that this Mac counted on 2026-09-18.
    fn measurement(ranking: &Ranking) -> Measurement<'_> {
        Measurement {
            ranking,
            swap: VmDelta {
                swapins: 46_564,
                swapouts: 40_156,
            },
            usage: SwapUsage {
                total_bytes: 11_811_160_064,
                used_bytes: 9_985_798_963,
            },
            compressor_bytes: 28_991_029_248,
            swap_window: SWAP_WINDOW,
            interval: interval(),
        }
    }

    /// The first line gives the processes of the ranking, the window that the
    /// tool measured, the interval that the person asked for, and the faults
    /// of every process over that window.
    #[test]
    fn the_first_line_gives_the_processes_the_window_and_the_faults() {
        let many = ranking(
            vec![row(10, 500_000), row(20, 300_000), row(30, 159_812)],
            959_812,
        );
        let one = ranking(vec![row(10, 3)], 3);

        assert_eq!(
            header(&measurement(&many)).first().map(String::as_str),
            Some("3 processes over a 4.0 s window (interval 5s) — 959,812 faults, 239,953/s")
        );
        assert_eq!(
            header(&measurement(&one)).first().map(String::as_str),
            Some("1 process over a 4.0 s window (interval 5s) — 3 faults, 0.8/s")
        );
    }

    /// The second line gives the swap traffic over the time that the tool
    /// measured it, the size of the compressor, and the swap in use. Those
    /// numbers say whether this Mac is short of memory, so the line is there
    /// even when every number is zero.
    #[test]
    fn the_second_line_gives_the_swap_traffic_the_compressor_and_the_swap_in_use() {
        let ranking = ranking(vec![row(10, 3)], 3);
        let quiet = Measurement {
            swap: VmDelta::default(),
            usage: SwapUsage::default(),
            compressor_bytes: 0,
            ..measurement(&ranking)
        };

        assert_eq!(
            header(&measurement(&ranking)).get(1).map(String::as_str),
            Some(
                "swap: 46,564 in, 40,156 out in 5.2 s · compressor 27.0 GB · swap in use 9.3 GB of 11.0 GB"
            )
        );
        assert_eq!(
            header(&quiet).get(1).map(String::as_str),
            Some("swap: 0 in, 0 out in 5.2 s · compressor 0 B · swap in use 0 B of 0 B")
        );
    }

    /// Gives `ranking`, with `claude` as its total of Claude Code.
    fn with_claude(mut ranking: Ranking, claude: ClaudeTotal) -> Ranking {
        ranking.claude = claude;
        ranking
    }

    /// The third line gives the total of Claude Code: the sessions, their
    /// share of all faults, and how many of them another account owns. The
    /// issue demands this line, because on 2026-09-18 the answer was the sum
    /// of 213 rows and no single row showed it.
    ///
    /// A count of another account that is zero leaves the parenthesis out. A
    /// Mac with no Claude Code process says so.
    #[test]
    fn the_third_line_gives_the_total_that_no_single_row_shows() {
        let base = ranking(
            vec![row(10, 500_000), row(20, 300_000), row(30, 159_812)],
            959_812,
        );
        let both = with_claude(
            base.clone(),
            ClaudeTotal {
                processes: 213,
                other_account: 100,
                faults: 865_752,
            },
        );
        let mine = with_claude(
            base.clone(),
            ClaudeTotal {
                processes: 213,
                other_account: 0,
                faults: 865_752,
            },
        );
        let one = with_claude(
            base.clone(),
            ClaudeTotal {
                processes: 1,
                other_account: 0,
                faults: 3_839,
            },
        );

        assert_eq!(
            header(&measurement(&both)).get(2).map(String::as_str),
            Some("Claude: 213 sessions made 90.2% of all faults (100 of another account)")
        );
        assert_eq!(
            header(&measurement(&mine)).get(2).map(String::as_str),
            Some("Claude: 213 sessions made 90.2% of all faults")
        );
        assert_eq!(
            header(&measurement(&one)).get(2).map(String::as_str),
            Some("Claude: 1 session made 0.4% of all faults")
        );
        assert_eq!(
            header(&measurement(&base)).get(2).map(String::as_str),
            Some("Claude: no session is running")
        );
    }

    /// Gives `ranking`, with `skipped` as the processes that it left out.
    fn with_skipped(mut ranking: Ranking, skipped: Skipped) -> Ranking {
        ranking.skipped = skipped;
        ranking
    }

    /// The fourth line names each process that the ranking left out, and the
    /// share of the faults that the processes which exited made. A count of
    /// zero is not there, and a Mac that left nothing out has no such line.
    ///
    /// The whole header is four lines here, in the order that a reader reads
    /// them.
    #[test]
    fn the_fourth_line_names_each_skipped_count_and_leaves_out_a_count_of_zero() {
        let base = ranking(
            vec![row(10, 500_000), row(20, 300_000), row(30, 155_973)],
            959_812,
        );
        let loaded = with_skipped(
            with_claude(
                base.clone(),
                ClaudeTotal {
                    processes: 213,
                    other_account: 100,
                    faults: 865_752,
                },
            ),
            Skipped {
                exited: 3,
                exited_faults: 3_839,
                zombies: 72,
                unsampled: 5,
            },
        );
        let zombies_only = with_skipped(
            base.clone(),
            Skipped {
                zombies: 72,
                ..Skipped::default()
            },
        );
        let one_of_each = with_skipped(
            base.clone(),
            Skipped {
                exited: 1,
                exited_faults: 3_839,
                zombies: 1,
                unsampled: 1,
            },
        );

        assert_eq!(
            header(&measurement(&loaded)),
            [
                "3 processes over a 4.0 s window (interval 5s) — 959,812 faults, 239,953/s",
                "swap: 46,564 in, 40,156 out in 5.2 s · compressor 27.0 GB · swap in use 9.3 GB of 11.0 GB",
                "Claude: 213 sessions made 90.2% of all faults (100 of another account)",
                "skipped: 3 exited before faulte read them (0.4% of the faults), 72 zombies, 5 were not in the top sample",
            ]
        );
        assert_eq!(
            header(&measurement(&zombies_only))
                .get(3)
                .map(String::as_str),
            Some("skipped: 72 zombies")
        );
        assert_eq!(
            header(&measurement(&one_of_each))
                .get(3)
                .map(String::as_str),
            Some(
                "skipped: 1 exited before faulte read it (0.4% of the faults), 1 zombie, 1 was not in the top sample"
            )
        );
        assert_eq!(
            header(&measurement(&base)).len(),
            3,
            "a Mac that left nothing out says nothing about what it left out"
        );
    }

    /// The outer border of one row of the table.
    const EDGE: char = '│';

    /// What holds two cells of one row apart.
    const BETWEEN_CELLS: char = '┆';

    /// Gives the cells of each row of `table`, the row of the names first.
    ///
    /// The frame and the padding say nothing about the ranking, and their
    /// width follows the widest cell of each column. The cells are what a
    /// reader reads.
    fn cells(table: &str) -> Vec<Vec<String>> {
        table
            .lines()
            .filter(|line| line.starts_with(EDGE))
            .map(|line| {
                line.trim_matches(EDGE)
                    .split(BETWEEN_CELLS)
                    .map(|cell| cell.trim().to_owned())
                    .collect()
            })
            .collect()
    }

    /// Gives the cells of one row as the assertions write them.
    fn row_of(cells: [&str; 10]) -> Vec<String> {
        cells.map(str::to_owned).to_vec()
    }

    /// Gives the time now in the tests: two days and five hours after each
    /// process of the tests started.
    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(STARTED + 2 * 86_400 + 5 * 3_600)
    }

    /// Gives the accounts of the tests: the viewer, and no other.
    fn accounts() -> Accounts {
        Accounts::new().with(Uid::new(VIEWER_UID), "tim")
    }

    /// The table names each column in the order that the issue lists them, and
    /// each row gives the facts of one process. The three columns of Claude
    /// Code are empty for a process that is not Claude Code.
    #[test]
    fn each_row_gives_the_facts_of_one_process_under_the_names_of_the_columns() {
        let ranking = ranking(vec![row(45_646, 500_000), row(20, 3)], 1_000_000);

        let drawn = rows(&ranking, &ranking.rows, None, &accounts(), now(), None);

        assert_eq!(cells(&drawn).first(), Some(&row_of(COLUMNS)));
        assert_eq!(
            cells(&drawn).get(1),
            Some(&row_of([
                "45646",
                "tim",
                "125,000",
                "50.0%",
                "812 MB",
                "2d 5h",
                "/usr/bin/process-45646 --flag",
                "",
                "",
                "",
            ]))
        );
        assert_eq!(
            cells(&drawn).get(2),
            Some(&row_of([
                "20",
                "tim",
                "0.8",
                "<0.1%",
                "812 MB",
                "2d 5h",
                "/usr/bin/process-20 --flag",
                "",
                "",
                "",
            ]))
        );
    }

    /// The session of a record in the tests.
    const SESSION_ID: &str = "d3b0d921-f0a1-41fc-b309-c11aa30c1173";

    /// The working directory of a record in the tests.
    const DIRECTORY: &str = "/Volumes/SamsungSSDs/code/tools";

    /// The UID of the other account of the tests.
    const OTHER_UID: u32 = 502;

    /// Gives the row of a Claude Code process of the account `uid`.
    fn claude_row(pid: u32, faults: u64, uid: u32, claude: ClaudeView) -> RankedRow {
        RankedRow {
            uid: Uid::new(uid),
            claude,
            ..row(pid, faults)
        }
    }

    /// Gives the session of [`SESSION_ID`].
    fn session() -> SessionId {
        SessionId::parse(SESSION_ID).expect("the test ID is a UUID")
    }

    /// A row of a Claude Code session gives its session, its state, and its
    /// directory. A session with no record gives none of the three, because a
    /// guess names the wrong session. A process of another account gives the
    /// command to run under `sudo`, and no state and no directory.
    #[test]
    fn a_row_of_claude_code_gives_its_session_its_state_and_its_directory() {
        let idle = SessionState::Idle {
            for_: Some(Duration::from_secs(3 * 3_600 + 12 * 60)),
        };
        let ranked = vec![
            claude_row(
                30,
                400_000,
                VIEWER_UID,
                ClaudeView::Session {
                    id: session(),
                    state: idle.clone(),
                    directory: Some(PathBuf::from(DIRECTORY)),
                },
            ),
            claude_row(31, 300_000, VIEWER_UID, ClaudeView::NoRecord),
            claude_row(32, 200_000, OTHER_UID, ClaudeView::OtherAccount),
            claude_row(
                33,
                100_000,
                VIEWER_UID,
                ClaudeView::Session {
                    id: session(),
                    state: idle,
                    directory: None,
                },
            ),
        ];
        let ranking = ranking(ranked, 1_000_000);

        let drawn = rows(&ranking, &ranking.rows, None, &accounts(), now(), None);
        let cells = cells(&drawn);

        assert_eq!(
            cells.get(1),
            Some(&row_of([
                "30",
                "tim",
                "100,000",
                "40.0%",
                "812 MB",
                "2d 5h",
                "/usr/bin/process-30 --flag",
                SESSION_ID,
                "idle 3h 12m",
                DIRECTORY,
            ]))
        );
        assert_eq!(
            cells.get(2),
            Some(&row_of([
                "31",
                "tim",
                "75,000",
                "30.0%",
                "812 MB",
                "2d 5h",
                "/usr/bin/process-31 --flag",
                "",
                "",
                "",
            ]))
        );
        assert_eq!(
            cells.get(3),
            Some(&row_of([
                "32",
                "502",
                "50,000",
                "20.0%",
                "812 MB",
                "2d 5h",
                "/usr/bin/process-32 --flag",
                OTHER_ACCOUNT,
                "",
                "",
            ]))
        );
        assert_eq!(
            cells.get(4),
            Some(&row_of([
                "33",
                "tim",
                "25,000",
                "10.0%",
                "812 MB",
                "2d 5h",
                "/usr/bin/process-33 --flag",
                SESSION_ID,
                "idle 3h 12m",
                ABSENT,
            ]))
        );
    }

    /// Gives the five rows that the tests of the limit share. Their faults add
    /// up to a million.
    fn five_rows() -> Vec<RankedRow> {
        vec![
            row(10, 500_000),
            row(11, 300_000),
            row(12, 100_000),
            row(13, 60_000),
            row(14, 40_000),
        ]
    }

    /// A limit keeps the first rows, and one line under the table counts the
    /// rows that it hid and gives their share of all faults. A limit that no
    /// row passes adds no line, and neither does no limit at all.
    #[test]
    fn the_limit_keeps_the_first_rows_and_counts_what_it_hid() {
        let ranking = ranking(five_rows(), 1_000_000);
        let drawn = |limit| rows(&ranking, &ranking.rows, limit, &accounts(), now(), None);

        let two = drawn(Some(2));
        assert_eq!(cells(&two).len(), 3, "the names and two rows");
        assert_eq!(
            two.lines().next_back(),
            Some("… 3 more processes made 20.0% of all faults")
        );

        let four = drawn(Some(4));
        assert_eq!(cells(&four).len(), 5, "the names and four rows");
        assert_eq!(
            four.lines().next_back(),
            Some("… 1 more process made 4.0% of all faults")
        );

        let every = drawn(None);
        assert_eq!(cells(&every).len(), 6, "the names and five rows");
        assert_eq!(drawn(Some(5)), every, "a limit of every row hides nothing");
        assert_eq!(
            drawn(Some(9)),
            every,
            "a limit above every row hides nothing"
        );
        assert_eq!(
            drawn(Some(0)).lines().next_back(),
            Some("… 5 more processes made 100% of all faults")
        );
    }

    /// The place of the `COMMAND` column in a row.
    const COMMAND_CELL: usize = 6;

    /// A statement of the width wins, then the terminal, then the default.
    ///
    /// A run through a pipe carries no window, and a table of no width is as
    /// wide as its widest cell. So the default is a width, never `None`.
    #[test]
    fn a_stated_width_wins_and_a_pipe_gets_the_default() {
        assert_eq!(table_width(Some("150"), Some(80)), 150);
        assert_eq!(table_width(Some("150"), None), 150);
        assert_eq!(table_width(None, Some(80)), 80);
        assert_eq!(table_width(None, None), DEFAULT_WIDTH);

        // A statement that is not a width says nothing, so the terminal
        // answers. A window of no columns is not a width either.
        for junk in ["", " ", "0", "-1", "wide", "80.5", "日本語", "99999999"] {
            assert_eq!(
                table_width(Some(junk), Some(80)),
                80,
                "{junk:?} states no width"
            );
            assert_eq!(table_width(Some(junk), None), DEFAULT_WIDTH);
        }
        assert_eq!(table_width(None, Some(0)), DEFAULT_WIDTH);
    }

    /// A command longer than the limit is cut, and the cut is marked.
    ///
    /// One process of this Mac carries a command line of more than 3,000
    /// characters. A table that gives such a command whole is as wide as that
    /// command, and one run of `faulte` through a pipe printed 178 kilobytes
    /// for that one row. A ranking names the process that faults, and the
    /// first characters of a command name it.
    #[test]
    fn a_command_longer_than_the_limit_is_cut_and_marked() {
        let long: String = "/usr/bin/node --a-very-long-flag ".repeat(200);
        let cut = command(&long);

        assert_eq!(cut.chars().count(), COMMAND_LIMIT + 1);
        assert!(cut.ends_with(MORE), "the cut must be marked: {cut}");
        assert!(long.starts_with(cut.trim_end_matches(MORE)));

        // A command of many bytes for one character keeps whole characters,
        // and a cut by bytes inside one character panics.
        let japanese: String = "日本語🎉café ".repeat(200);
        let cut = command(&japanese);
        assert_eq!(cut.chars().count(), COMMAND_LIMIT + 1);

        // A command at the limit, and a command below it, stay whole.
        let short: String = "a".repeat(COMMAND_LIMIT);
        assert_eq!(command(&short), short);
        assert_eq!(command("claude"), "claude");
    }

    /// A command of many bytes for one character, and a command of many
    /// characters, stay whole. A cut by bytes inside one character panics, and
    /// a cut by characters loses part of the command.
    ///
    /// `ps` escapes every byte outside printable ASCII under `LC_ALL=C`, so
    /// this text is not what the sampler reads today. The command comes from
    /// another process, and a tool that panics on the text of another process
    /// stops at the first such process on the Mac.
    #[test]
    fn a_command_of_many_bytes_or_many_characters_stays_whole() {
        let japanese = "/Applications/日本語.app/Contents/MacOS/日本語 --flag 🎉 café";
        let long = format!("/usr/bin/node {}", "--a-very-long-flag ".repeat(60));
        let ranked = vec![
            RankedRow {
                command: japanese.to_owned(),
                ..row(10, 500_000)
            },
            RankedRow {
                command: long.clone(),
                ..row(11, 500_000)
            },
        ];
        let ranking = ranking(ranked, 1_000_000);

        let wide = rows(&ranking, &ranking.rows, None, &accounts(), now(), None);
        let commands: Vec<String> = cells(&wide)
            .iter()
            .skip(1)
            .filter_map(|row| row.get(COMMAND_CELL).cloned())
            .collect();
        assert_eq!(commands, [japanese.to_owned(), command(&long)]);

        // A narrow terminal wraps the command over several lines. Each line
        // holds whole characters, so every character of the command is still
        // there.
        let narrow = rows(&ranking, &ranking.rows, None, &accounts(), now(), Some(60));
        let drawn: String = narrow
            .chars()
            .filter(|glyph| !glyph.is_whitespace())
            .collect();
        for glyph in japanese.chars().filter(|glyph| !glyph.is_whitespace()) {
            assert!(
                drawn.contains(glyph),
                "the character {glyph:?} of {japanese}"
            );
        }
    }
}
