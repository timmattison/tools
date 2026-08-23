//! The printed view of the aggregate table, and the measure it stands on.
//!
//! A terminal holds columns, and a string holds bytes. The two counts agree
//! only while every character of the text is an ASCII one. One character of a
//! host name takes one column, or two columns when the glyph is a wide one, and
//! a name of two bytes for each character takes half the bytes it looks like it
//! takes. A cell that measured its text in bytes would therefore print a short
//! name over the column that follows it, and a cut by bytes would stop in the
//! middle of a character and panic. This module measures in columns and cuts on
//! a character, so every cell of the table keeps its column and every name
//! stays a string.
//!
//! A later slice prints the table on these helpers, and states there the
//! order in which the columns drop as the terminal gets narrow.
//!
//! The module also writes the one duration text of the crate. The resolved
//! configuration, the status line of a round, and the header line of the frame
//! each name a period of time, and a second writer of a duration would print
//! `1s` in one of those three places and `1000ms` in another.

#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the render of the table is the reader of these helpers, and that render arrives in a later slice of issue #370, so the tests of this module read them today"
    )
)]

use crate::{ROUND, SECONDS_PER_HOUR, SECONDS_PER_MINUTE, UNKNOWN};
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::time::Duration;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The start of the header line: one space, the name of the tool, and two more
/// spaces.
///
/// The leading space holds the name off the left edge of the terminal, where
/// the whole block — this line, the column header, and every row — then stands
/// one column in from the frame. The two spaces that follow are the widest gap
/// of the line, so the name of the tool reads as the name of the block and not
/// as the first field of it.
const HEADER_START: &str = " krt  ";

/// The text between two fields of the header line.
///
/// Three spaces, and not the two of a summary line. Two of these fields hold a
/// space of their own — `src 1.2.3.4` and `round 142` each do — so a narrower
/// gap would read as one sentence, and a reader would have to know the words to
/// find where one field stops.
const FIELD_SEPARATOR: &str = "   ";

/// The glyph between a destination and the address that it resolved to.
///
/// The destination is what the user typed, and the address is what the resolver
/// answered. The arrow says which is which. The summary line of a replay writes
/// the same pair as `name (address)`, because that line names a run, where this
/// one heads the table of the path to that address.
const RESOLVES_TO: &str = " → ";

/// The name of the field that holds the source address of the run.
const SOURCE: &str = "src";

/// What the header line of the frame names.
///
/// The line stands above the table, and it holds what every row of the table
/// has in common: what the run probed, where it probed from, how many rounds it
/// folded, how often it probed, and which file holds the record. A reader who
/// asks "what am I looking at" reads this one line, and a reader who asks "how
/// is hop 7 doing" reads the table below it. The split is what keeps the table
/// to one column for each number it prints.
///
/// Every field that a replay can fail to fill is an `Option`. A recorded file
/// whose `run` record is absent — a file that a run was still writing when the
/// machine stopped — names no destination, no address, no source, and no
/// interval. The line then writes one word in the place of each of them, and
/// the replay goes on to print the rounds that the file did record. A replay
/// that refused such a file would throw those rounds away.
pub(crate) struct Header<'a> {
    /// The destination as the command line named it. A run whose `run` record
    /// is absent names none.
    pub(crate) destination: Option<&'a str>,
    /// The address that the destination resolved to.
    pub(crate) address: Option<IpAddr>,
    /// The source address of the run.
    pub(crate) source: Option<IpAddr>,
    /// The number of rounds that the run recorded.
    pub(crate) rounds: usize,
    /// The period of one round.
    pub(crate) interval: Option<Duration>,
    /// The name of the recorded file, without its directory.
    ///
    /// The directory holds the columns that the table needs for its numbers,
    /// and it says nothing about the run: the user just named the file to the
    /// `replay` command, so the user knows where it stands.
    pub(crate) file: &'a str,
    /// The size of that file, in bytes. A file whose size did not read holds
    /// none.
    pub(crate) bytes: Option<u64>,
}

impl Header<'_> {
    /// The one line that stands above the table.
    ///
    /// Five fields stand in it: the target, the source, the count of the
    /// rounds, the period of one round, and the recorded file with its size.
    /// Each of them carries a label of its own, or a value that names itself, so
    /// no field depends on where it stands. A reader finds the source by the
    /// word `src`, and not by counting the gaps from the left.
    ///
    /// A target names a destination and an address together, or it names
    /// nothing. A destination with no address is a name that this run never
    /// probed, and an address with no name is a number that the user did not
    /// type, so half a target says less than the one word that stands for a
    /// target the file holds none of.
    ///
    /// The label of the rounds stays `round` for every count, where a summary
    /// line writes `142 rounds`. This line is a set of labelled fields and not a
    /// sentence, and a label that grew with the count would move the text of
    /// every field behind it as the run goes on.
    pub(crate) fn line(&self) -> String {
        let target = match (self.destination, self.address) {
            (Some(destination), Some(address)) => format!("{destination}{RESOLVES_TO}{address}"),
            _ => UNKNOWN.to_owned(),
        };
        let source = self
            .source
            .map_or_else(|| UNKNOWN.to_owned(), |address| address.to_string());
        let interval = self
            .interval
            .map_or_else(|| UNKNOWN.to_owned(), render_duration);
        let size = self.bytes.map_or_else(|| UNKNOWN.to_owned(), render_size);
        let fields = [
            target,
            format!("{SOURCE} {source}"),
            format!("{ROUND} {}", self.rounds),
            interval,
            format!("{} ({size})", self.file),
        ];
        format!("{HEADER_START}{}", fields.join(FIELD_SEPARATOR))
    }
}

/// The units of a file size, smallest first.
const SIZE_UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];

/// The number of bytes of one unit, in the unit below it.
const BYTES_PER_UNIT: f64 = 1024.0;

/// The number of decimal places that a size above one step prints.
const SIZE_DECIMALS: usize = 1;

/// The smallest size that one decimal place writes as a whole unit.
///
/// The print rounds, so every size from half of the last decimal place below
/// 1024 writes as `1024.0`. The scale steps to the next unit at that point, and
/// not at 1024 exactly, because `1024.0 KB` writes one megabyte in kilobytes:
/// one whole unit of the scale, in the units of the step below it.
const ROUNDS_UP_TO_A_WHOLE_UNIT: f64 = 1023.95;

/// Reads a size as the number that the scale divides.
///
/// Each step of the scale is a divide by 1024, so the size needs a number that
/// holds a fraction. `count_as_f64` of `stats.rs` reads a count the same way,
/// for the same reason.
#[expect(
    clippy::cast_precision_loss,
    reason = "an `f64` holds every whole number below 2^53, which is 8 petabytes. A recorded file above that point loses digits far below the one decimal place that the scale prints, and no run writes such a file: one round of one path records a few hundred bytes"
)]
fn bytes_as_f64(bytes: u64) -> f64 {
    bytes as f64
}

/// Writes the size of the recorded file, in the largest unit that holds it.
///
/// A size below one step reads as whole bytes. A file of 842 bytes is 842
/// bytes, and `0.8 KB` says less about it. Every larger size reads to one
/// decimal place, in the largest unit of the scale that holds it. A size above
/// the largest unit stays in that unit, and `5120.0 TB` is a number a reader
/// still reads.
///
/// This is `rr::format_size` (`src/rr/src/main.rs`), at one decimal place
/// instead of two. `krt` cannot call that one: `rr` is a binary crate, so it
/// exports nothing and there is no library of it to take a dependency on. Do
/// not go looking for one.
fn render_size(bytes: u64) -> String {
    let mut size = bytes_as_f64(bytes);
    if size < BYTES_PER_UNIT {
        // A whole number of bytes, and no decimal place: the bytes are the
        // measure, not a rounding of a larger unit.
        return format!("{bytes} {}", SIZE_UNITS[0]);
    }
    let mut unit = 0;
    while size >= ROUNDS_UP_TO_A_WHOLE_UNIT && unit < SIZE_UNITS.len() - 1 {
        size /= BYTES_PER_UNIT;
        unit += 1;
    }
    format!("{size:.SIZE_DECIMALS$} {}", SIZE_UNITS[unit])
}

/// Writes the shortest text of a duration, to the millisecond.
///
/// A duration that carries milliseconds becomes milliseconds. A whole number of
/// hours becomes hours. A whole number of minutes becomes minutes. Every other
/// duration becomes seconds. The text reads like the text a user types, so
/// `Duration::from_secs(3600)` becomes `1h`. A duration that carries less than
/// one millisecond loses that remainder. No caller gives such a duration today,
/// because every duration comes from `parse_duration`, which stops at
/// milliseconds.
pub(crate) fn render_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if duration.subsec_millis() != 0 || seconds == 0 {
        return format!("{}ms", duration.as_millis());
    }
    if seconds.is_multiple_of(SECONDS_PER_HOUR) {
        return format!("{}h", seconds / SECONDS_PER_HOUR);
    }
    if seconds.is_multiple_of(SECONDS_PER_MINUTE) {
        return format!("{}m", seconds / SECONDS_PER_MINUTE);
    }
    format!("{seconds}s")
}

/// The number of terminal columns that the text occupies.
///
/// A wide glyph, as one of the Japanese or the emoji ones, takes two columns.
/// Every other printable character takes one, and a character that prints
/// nothing takes none.
pub(crate) fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// The text, cut to at most `width` terminal columns.
///
/// Text that already fits comes back unchanged. Text that does not fit loses
/// its tail, and the cut falls between two characters, never inside one. A wide
/// glyph that would take the one column past the limit goes away whole, so the
/// result never runs one column over the width it was given. A width of zero
/// gives an empty string.
///
/// The cut carries no ellipsis. The Host column of the table is narrow, and
/// three of its columns are three columns of the name. A name that lost its
/// tail already reads as a name that lost its tail.
///
/// This is not `termbar::truncate_filename`. That helper keeps the extension of
/// a file name and cuts the middle, because the extension of a file names the
/// kind of the file. A host name is not a file name: the tail of
/// `ae-1.core.example.net` is the domain that every router of that network
/// shares, and the head is the one part that tells the routers apart. The
/// helper would therefore keep the part that says the least.
pub(crate) fn truncate_to_width(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.to_string();
    }
    let mut kept = String::new();
    let mut taken = 0;
    for character in text.chars() {
        // A character that prints nothing takes no column, and it stays with
        // the character it belongs to.
        let columns = UnicodeWidthChar::width(character).unwrap_or(0);
        if taken + columns > width {
            // The loop stops here, and it does not look for a narrow character
            // behind this one. A cut that kept a later character would print a
            // name that the path never held.
            break;
        }
        taken += columns;
        kept.push(character);
    }
    kept
}

/// The number of bars that the sparkline draws with.
const LEVEL_COUNT: usize = 8;

/// The bars of the sparkline, lowest first.
///
/// There is no ASCII fallback, and `CLAUDE.md` asks for it that way. A fallback
/// of `.:|#` characters would draw a second, coarser picture of the same
/// numbers, so a reader of one terminal and a reader of another would argue
/// over a hop that the two pictures disagree about. Every terminal that this
/// tool draws a table on prints these eight glyphs, and each of them takes one
/// column, so the Recent column holds one sample for each column it is wide.
const BARS: [char; LEVEL_COUNT] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// The count of the bars, as the arithmetic of the scale reads it.
#[expect(
    clippy::cast_precision_loss,
    reason = "the count of the bars is 8, and an `f64` holds every whole number below 2^53"
)]
const LEVELS: f64 = LEVEL_COUNT as f64;

/// The recent round-trip times of one key, as a bar for each of them.
///
/// The bar shows the last `width` samples. A key that holds more samples than
/// that drops its oldest ones, because the column is as wide as the terminal
/// gives it and the most recent samples are the ones that say what the hop is
/// doing now.
///
/// The scale runs from the smallest to the largest sample **of the shown
/// window**, and not of the whole history: the smallest takes `▁` and the
/// largest takes `█`. A scale over the whole history would flatten the window
/// of a hop that was once slow and is not slow now, and that window is the part
/// a reader is looking at.
///
/// A window whose samples are all equal draws `▁` for each of them. Nothing
/// varies, and a flat line at the floor says that. The alternative, a flat line
/// at the top, would draw the quietest hop of the path as the loudest one.
///
/// A key with no sample, and a `width` of zero, each draw an empty string.
///
/// A sample that is not a finite number — `f64::NAN`, or an infinity — draws
/// the lowest bar, and the scale does not read it at all. Such a sample does
/// not compare, so a scale that took it would carry a limit that no bar can
/// stand on, and every bar of the key would then read the same. One bad sample
/// would hide the whole history of the hop, which is a worse answer than one
/// bar that reads low.
pub(crate) fn sparkline(samples: impl ExactSizeIterator<Item = f64>, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    // The iterator states its length before it gives a sample, so the oldest
    // samples go away as the iterator runs. The window is therefore the only
    // copy that this function makes, and the history of the key stays where it
    // is.
    let skipped = samples.len().saturating_sub(width);
    let shown: Vec<f64> = samples.skip(skipped).collect();

    // The scale reads only the samples that compare.
    let mut lowest = f64::INFINITY;
    let mut highest = f64::NEG_INFINITY;
    for sample in shown.iter().copied().filter(|sample| sample.is_finite()) {
        lowest = lowest.min(sample);
        highest = highest.max(sample);
    }
    let span = highest - lowest;

    shown
        .iter()
        .map(|&sample| {
            // A window of one sample, and a window whose samples are all equal,
            // each give a span of zero. A window that holds no sample which
            // compares gives a span below zero, because the two limits stayed
            // at the infinities that the fold started them at. Neither window
            // divides.
            if span <= 0.0 || !sample.is_finite() {
                BARS[0]
            } else {
                bar_at((sample - lowest) / span)
            }
        })
        .collect()
}

/// The bar of one sample, at its part of the span of the window.
///
/// The part of the largest sample is 1, so a multiply by the count of the bars
/// gives one past the last of them. The limit below puts that sample on the
/// highest bar, which is where the largest sample of a window belongs.
fn bar_at(part: f64) -> char {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the cast of a float to an integer saturates in Rust: a number below zero and a number that is not a number each give 0, and a number too large gives `usize::MAX`. The limit below then holds the result inside the array, so no part reads past the last bar"
    )]
    let level = (part * LEVELS) as usize;
    BARS[level.min(LEVEL_COUNT - 1)]
}

/// One rendered view of one folded run.
///
/// The frame holds what a reader needs and nothing that the reader must give
/// back: the header line names the run, the table folds it, and the two maps
/// name the addresses. A caller builds one of these and asks for the lines at
/// the width of its terminal.
pub(crate) struct Frame<'a> {
    /// What the line above the table names.
    pub(crate) header: Header<'a>,
    /// The folded aggregate of the run.
    pub(crate) table: &'a crate::stats::HopTable,
    /// The host name of each address that a `name` record named.
    pub(crate) names: &'a BTreeMap<IpAddr, String>,
    /// The address of the destination. The row that answered from it takes the
    /// star.
    pub(crate) destination: Option<IpAddr>,
}

impl Frame<'_> {
    /// The lines of the frame at a terminal width.
    pub(crate) fn lines(&self, width: u16) -> Vec<String> {
        let _ = width;
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        display_width, render_duration, render_size, sparkline, truncate_to_width, Frame, Header,
    };
    use crate::record::RoundRecord;
    use crate::stats::HopTable;
    use crate::testing::{address, round};
    use std::collections::BTreeMap;
    use std::net::IpAddr;
    use std::time::Duration;
    use unicode_width::UnicodeWidthChar;

    /// The start of the header line: one space, the name of the tool, and two
    /// more spaces.
    ///
    /// The test spells the text, and the module spells it again. That is on
    /// purpose, as it is for the bars of the sparkline: a test that read the
    /// constant of the module would agree with every start the module ever
    /// holds, and the start is a part of the line that a reader of the table
    /// sees.
    const HEADER_START: &str = " krt  ";

    /// The text between two fields of the header line.
    const FIELD_SEPARATOR: &str = "   ";

    /// The name of the recorded file of every header below.
    const FILE: &str = "1.2.3.4-example.com.jsonl";

    /// The size of that file, in bytes.
    ///
    /// 2 200 000 / 1024 is 2148.4, and / 1024 again is 2.0980, which one
    /// decimal place writes as `2.1`.
    const FILE_BYTES: u64 = 2_200_000;

    /// The header line of a run that filled every field, character for
    /// character.
    const WHOLE_LINE: &str = " krt  example.com → 93.184.216.34   src 1.2.3.4   round 142   1s   1.2.3.4-example.com.jsonl (2.1 MB)";

    /// The header of a run that filled every field of the line.
    fn whole_header() -> Header<'static> {
        Header {
            destination: Some("example.com"),
            address: Some(address("93.184.216.34")),
            source: Some(address("1.2.3.4")),
            rounds: 142,
            interval: Some(Duration::from_secs(1)),
            file: FILE,
            bytes: Some(FILE_BYTES),
        }
    }

    /// The fields of a header line, without the name of the tool.
    ///
    /// The split reads the separator, so a line that holds the wrong number of
    /// spaces gives the wrong number of fields, and a test that asserts on one
    /// field says which field it read.
    fn fields(line: &str) -> Vec<&str> {
        line.strip_prefix(HEADER_START)
            .unwrap_or(line)
            .split(FIELD_SEPARATOR)
            .collect()
    }

    /// The field of a header line at an index, or an empty text when the line
    /// holds no such field.
    ///
    /// A line of the wrong number of fields must fail the assertion of the test
    /// that reads one of them, and must not stop that test with an index. The
    /// reader of the failure then sees the field the test wanted beside the
    /// text the line gave.
    fn field(line: &str, index: usize) -> &str {
        fields(line).get(index).copied().unwrap_or_default()
    }

    #[test]
    fn the_header_line_names_the_target_the_source_the_rounds_the_interval_and_the_file() {
        assert_eq!(
            whole_header().line(),
            WHOLE_LINE,
            "the header line of a whole run reads as the module documentation draws it"
        );
    }

    #[test]
    fn a_run_whose_record_is_absent_names_no_target_no_source_and_no_interval() {
        let header = Header {
            destination: None,
            address: None,
            source: None,
            rounds: 0,
            interval: None,
            file: FILE,
            bytes: Some(FILE_BYTES),
        };
        assert_eq!(
            header.line(),
            " krt  unknown   src unknown   round 0   unknown   1.2.3.4-example.com.jsonl (2.1 MB)",
            "a file that holds no `run` record still names its rounds and its file"
        );
    }

    #[test]
    fn half_a_target_names_no_target() {
        for header in [
            Header {
                address: None,
                ..whole_header()
            },
            Header {
                destination: None,
                ..whole_header()
            },
        ] {
            let line = header.line();
            assert_eq!(
                field(&line, 0),
                "unknown",
                "a target of one half names nothing: {line}"
            );
        }
    }

    #[test]
    fn the_name_of_the_tool_stands_two_spaces_before_the_first_field() {
        let line = whole_header().line();
        assert!(
            line.starts_with(HEADER_START),
            "the line starts with one space, the name of the tool, and two more: {line}"
        );
        assert!(
            !line.starts_with(" krt   "),
            "two spaces stand after the name of the tool, and not three: {line}"
        );
    }

    #[test]
    fn three_spaces_stand_between_the_fields() {
        let line = whole_header().line();
        assert_eq!(
            fields(&line),
            [
                "example.com → 93.184.216.34",
                "src 1.2.3.4",
                "round 142",
                "1s",
                "1.2.3.4-example.com.jsonl (2.1 MB)",
            ],
            "the separator holds three spaces, so the five fields split on it"
        );
    }

    #[test]
    fn a_count_of_one_round_keeps_the_word_of_the_label() {
        // `round` is the label of a field, and not the noun of a sentence, so
        // it stays the same word for every count.
        let header = Header {
            rounds: 1,
            ..whole_header()
        };
        let line = header.line();
        assert_eq!(field(&line, 2), "round 1", "one round reads `round 1`");
    }

    #[test]
    fn the_interval_reads_as_the_duration_of_a_command_line() {
        let header = Header {
            interval: Some(Duration::from_millis(500)),
            ..whole_header()
        };
        let line = header.line();
        assert_eq!(
            field(&line, 3),
            "500ms",
            "the interval reads as the text the user typed"
        );
    }

    #[test]
    fn a_file_whose_size_is_absent_reads_one_word() {
        let header = Header {
            bytes: None,
            ..whole_header()
        };
        let line = header.line();
        assert_eq!(
            field(&line, 4),
            "1.2.3.4-example.com.jsonl (unknown)",
            "a file that did not measure still names itself"
        );
    }

    #[test]
    fn a_size_below_one_step_reads_as_whole_bytes() {
        assert_eq!(render_size(0), "0 B", "an empty file holds no byte");
        assert_eq!(
            render_size(842),
            "842 B",
            "a file of 842 bytes is 842 bytes, and `0.8 KB` says less about it"
        );
        assert_eq!(
            render_size(1023),
            "1023 B",
            "the largest size below one step keeps its bytes"
        );
    }

    #[test]
    fn one_step_reads_one_decimal_place() {
        assert_eq!(
            render_size(1024),
            "1.0 KB",
            "the first size of the second unit reads as one of it"
        );
    }

    #[test]
    fn a_size_reads_in_the_largest_unit_that_holds_it() {
        // 2 200 000 / 1024 is 2148.4, and / 1024 again is 2.0980.
        assert_eq!(
            render_size(FILE_BYTES),
            "2.1 MB",
            "two steps of 1024 leave a number at or above one"
        );
        assert_eq!(
            render_size(1024_u64.pow(3)),
            "1.0 GB",
            "three steps of 1024 read as gigabytes"
        );
        assert_eq!(
            render_size(1024_u64.pow(4)),
            "1.0 TB",
            "four steps of 1024 read as terabytes"
        );
    }

    #[test]
    fn a_size_past_the_largest_unit_stays_in_it() {
        assert_eq!(
            render_size(5 * 1024_u64.pow(5)),
            "5120.0 TB",
            "five petabytes read as 5120 terabytes, because the scale stops at terabytes"
        );
        assert_eq!(
            render_size(u64::MAX),
            "16777216.0 TB",
            "the largest size that a `u64` holds reads in terabytes as well"
        );
    }

    #[test]
    fn a_size_that_the_print_would_round_up_steps_to_the_next_unit() {
        // 1 048 575 / 1024 is 1023.999, which one decimal place writes as
        // `1024.0`. `1024.0 KB` writes one megabyte in kilobytes.
        assert_eq!(
            render_size(1024 * 1024 - 1),
            "1.0 MB",
            "a size that rounds up to a whole unit reads in that unit"
        );
        // 1 047 950 / 1024 is 1023.38, which stays below the round.
        assert_eq!(
            render_size(1_047_950),
            "1023.4 KB",
            "a size that rounds to less than a whole unit keeps its unit"
        );
    }

    #[test]
    fn renders_milliseconds() {
        assert_eq!(render_duration(Duration::from_millis(500)), "500ms");
    }

    #[test]
    fn renders_one_second() {
        assert_eq!(render_duration(Duration::from_secs(1)), "1s");
    }

    #[test]
    fn renders_many_seconds() {
        assert_eq!(render_duration(Duration::from_secs(90)), "90s");
    }

    #[test]
    fn renders_minutes() {
        assert_eq!(render_duration(Duration::from_mins(2)), "2m");
    }

    #[test]
    fn renders_one_hour() {
        assert_eq!(render_duration(Duration::from_hours(1)), "1h");
    }

    #[test]
    #[allow(
        clippy::duration_suboptimal_units,
        reason = "the seconds are the behavior: a duration of 3600 seconds renders as hours"
    )]
    fn renders_seconds_that_make_a_whole_hour_as_hours() {
        assert_eq!(render_duration(Duration::from_secs(3600)), "1h");
    }

    #[test]
    fn renders_whole_hours_as_hours() {
        assert_eq!(render_duration(Duration::from_hours(2)), "2h");
    }

    #[test]
    fn renders_a_duration_that_carries_milliseconds_as_milliseconds() {
        assert_eq!(render_duration(Duration::from_millis(1500)), "1500ms");
    }

    /// A name of wide glyphs. Each of the eight characters takes two columns,
    /// so the name takes 16 columns and 24 bytes.
    const JAPANESE: &str = "日本語のホスト名";

    /// A name of three emoji and three wide Japanese characters. Each of the
    /// six characters takes two columns, so the name takes 12 columns.
    const EMOJI: &str = "🎉🎊🎁ホスト";

    /// A name whose fourth character takes two bytes and one column.
    ///
    /// A cut by bytes at the fourth byte lands inside the `é` and panics. A cut
    /// by characters at the fourth character gives `café`.
    const ACCENTED: &str = "café-router.example";

    /// A name of ASCII characters, one column for each byte.
    const ASCII: &str = "router.lan";

    #[test]
    fn the_width_of_an_ascii_name_is_the_count_of_its_characters() {
        assert_eq!(
            display_width(ASCII),
            10,
            "each of the ten ASCII characters takes one column"
        );
    }

    #[test]
    fn the_width_of_a_wide_name_is_two_columns_for_each_glyph() {
        assert_eq!(
            display_width(JAPANESE),
            16,
            "each of the eight glyphs takes two columns"
        );
        assert_eq!(
            display_width(EMOJI),
            12,
            "each of the six glyphs takes two columns"
        );
    }

    #[test]
    fn the_width_of_a_mixed_name_counts_the_bytes_of_no_character() {
        // The name holds 19 characters, and the `é` holds two bytes. A measure
        // in bytes would give 20.
        assert_eq!(
            display_width(ACCENTED),
            19,
            "an accented character takes one column and two bytes"
        );
        assert_eq!(
            display_width("ttl 日本"),
            8,
            "four ASCII characters and two wide glyphs take 4 + 4 columns"
        );
    }

    #[test]
    fn a_name_that_fits_comes_back_whole() {
        assert_eq!(
            truncate_to_width(ASCII, 30),
            ASCII,
            "a name below the width keeps every character"
        );
        assert_eq!(
            truncate_to_width(ASCII, 10),
            ASCII,
            "a name of exactly the width keeps every character"
        );
        assert_eq!(
            truncate_to_width(JAPANESE, 16),
            JAPANESE,
            "a wide name of exactly the width keeps every glyph"
        );
    }

    #[test]
    fn a_width_of_zero_gives_an_empty_name() {
        for text in [ASCII, JAPANESE, EMOJI, ACCENTED] {
            assert_eq!(
                truncate_to_width(text, 0),
                "",
                "no column holds no character of {text}"
            );
        }
    }

    #[test]
    fn a_wide_glyph_that_crosses_the_limit_goes_away_whole() {
        // Two glyphs take four columns, and the third would take the fifth and
        // the sixth. A limit of five therefore holds two of the glyphs.
        assert_eq!(
            truncate_to_width(JAPANESE, 5),
            "日本",
            "the glyph that would cross the limit goes away whole"
        );
        assert_eq!(
            display_width(&truncate_to_width(JAPANESE, 5)),
            4,
            "the cut name stops one column short of the odd limit"
        );
        assert_eq!(
            truncate_to_width(JAPANESE, 6),
            "日本語",
            "an even limit holds three of the glyphs"
        );
    }

    #[test]
    fn an_emoji_name_cuts_on_a_glyph() {
        // Three emoji take six columns, and the fourth glyph would take the
        // seventh and the eighth.
        assert_eq!(
            truncate_to_width(EMOJI, 7),
            "🎉🎊🎁",
            "the wide glyph that would cross the limit goes away whole"
        );
        assert_eq!(
            truncate_to_width(EMOJI, 2),
            "🎉",
            "two columns hold one emoji"
        );
        assert_eq!(
            truncate_to_width(EMOJI, 1),
            "",
            "one column holds no wide glyph"
        );
    }

    #[test]
    fn an_accented_name_cuts_on_a_character_and_not_on_a_byte() {
        // The fourth character is the `é`, and it holds two bytes. A cut at the
        // fourth byte lands inside it.
        assert_eq!(
            truncate_to_width(ACCENTED, 4),
            "café",
            "the cut keeps the whole of the accented character"
        );
        assert_eq!(
            truncate_to_width(ACCENTED, 3),
            "caf",
            "the cut before the accented character keeps three characters"
        );
        assert_eq!(
            truncate_to_width(ACCENTED, 12),
            "café-router.",
            "the cut counts the accented character as one column"
        );
    }

    #[test]
    fn a_cut_name_never_runs_over_the_width() {
        for text in [ASCII, JAPANESE, EMOJI, ACCENTED, "日ab🎉c"] {
            for width in 0..=display_width(text) + 2 {
                let cut = truncate_to_width(text, width);
                assert!(
                    display_width(&cut) <= width,
                    "the cut of {text} to {width} columns takes {} of them",
                    display_width(&cut)
                );
                assert!(
                    text.starts_with(&cut),
                    "the cut of {text} to {width} columns keeps its head"
                );
            }
        }
    }

    #[test]
    fn a_cut_name_keeps_as_many_columns_as_it_holds() {
        // The name takes seven columns: two for the wide glyph, one for each of
        // the two ASCII letters, two for the emoji, and one for the last
        // letter. A limit of six therefore holds every character but the last
        // one.
        let text = "日ab🎉c";
        assert_eq!(display_width(text), 7, "the name takes seven columns");
        assert_eq!(
            truncate_to_width(text, 6),
            "日ab🎉",
            "the cut keeps every character that fits"
        );
        assert_eq!(
            truncate_to_width(text, 4),
            "日ab",
            "the emoji would take the fifth and the sixth column"
        );
    }

    /// The eight bars of the sparkline, lowest first.
    ///
    /// The test states them, and the module states them again. The two are on
    /// purpose: a test that read the constant of the module would agree with
    /// every set of glyphs the module ever holds, and the set of glyphs is the
    /// part of the sparkline a reader of the table sees.
    const BARS: &str = "▁▂▃▄▅▆▇█";

    /// The bar of a set of samples, at a width.
    fn bar(samples: &[f64], width: usize) -> String {
        sparkline(samples.iter().copied(), width)
    }

    #[test]
    fn a_rising_ramp_draws_every_bar_of_the_set() {
        // The samples run from 1 to 8, so the span is 7. The bar of a sample is
        // its distance from the smallest one, over the span, times the eight
        // bars: 0/7, 8/7, 16/7 ... which cut to 0, 1, 2, 3, 4, 5, 6, and the
        // largest sample gives 8, which the clamp puts on the last bar.
        assert_eq!(
            bar(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0], 9),
            BARS,
            "eight samples one step apart draw each of the eight bars once"
        );
    }

    #[test]
    fn the_smallest_sample_takes_the_lowest_bar_and_the_largest_takes_the_highest() {
        // The samples run from 10 to 40, so the span is 30. The bar of 20 is
        // (20 - 10) / 30 * 8 = 2.67, which cuts to the third bar. The bar of 30
        // is (30 - 10) / 30 * 8 = 5.33, which cuts to the sixth bar.
        let drawn = bar(&[10.0, 20.0, 30.0, 40.0], 9);
        assert_eq!(drawn, "▁▃▆█", "the middle samples take the middle bars");
        assert_eq!(
            drawn.chars().next(),
            Some('▁'),
            "the smallest sample takes the lowest bar"
        );
        assert_eq!(
            drawn.chars().next_back(),
            Some('█'),
            "the largest sample takes the highest bar"
        );
    }

    #[test]
    fn no_sample_draws_nothing() {
        assert_eq!(
            bar(&[], 9),
            "",
            "a key with no round-trip time draws no bar"
        );
    }

    #[test]
    fn a_width_of_zero_draws_nothing() {
        assert_eq!(
            bar(&[1.0, 2.0, 3.0], 0),
            "",
            "no column holds no bar, however many samples the key holds"
        );
    }

    #[test]
    fn samples_that_are_all_equal_draw_the_lowest_bar() {
        // The smallest sample and the largest one are the same, so the span is
        // zero and no sample stands above another one. A flat line at the floor
        // says that.
        assert_eq!(
            bar(&[5.0, 5.0, 5.0], 9),
            "▁▁▁",
            "a key whose round-trip time never moved draws a flat line"
        );
        assert_eq!(
            bar(&[0.0, 0.0], 9),
            "▁▁",
            "a span of zero at the floor draws the lowest bar as well"
        );
    }

    #[test]
    fn the_bar_holds_the_last_samples_of_a_longer_history() {
        // The first two samples are far above the last four. A window of four
        // therefore drops them, and the scale of the window runs from 1 to 4:
        // (2 - 1) / 3 * 8 = 2.67 cuts to the third bar, and (3 - 1) / 3 * 8 =
        // 5.33 cuts to the sixth.
        let history = [100.0, 200.0, 1.0, 2.0, 3.0, 4.0];
        assert_eq!(
            bar(&history, 4),
            "▁▃▆█",
            "the window holds the last four samples, and its scale reads only them"
        );

        // The same history at a width that holds all of it reads a scale from 1
        // to 200, and the four small samples then crowd on the lowest bar. The
        // two results differ, so the window drops the oldest samples and not
        // the most recent ones.
        assert_eq!(
            bar(&history, 6),
            "▄█▁▁▁▁",
            "a window that holds the whole history reads the large samples too"
        );
    }

    #[test]
    fn one_sample_draws_one_bar() {
        // One sample is the smallest and the largest at once, so the span is
        // zero and the rule of the flat line gives the lowest bar.
        assert_eq!(bar(&[42.0], 9), "▁", "one round-trip time draws one bar");
    }

    #[test]
    fn a_sample_that_is_not_a_number_keeps_the_bar_of_every_other_sample() {
        // A sample that does not compare takes the lowest bar and stays out of
        // the scale. The ramp of eight therefore keeps each of its bars.
        assert_eq!(
            bar(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, f64::NAN], 9),
            "▁▂▃▄▅▆▇█▁",
            "the sample that is not a number draws the lowest bar and moves no other bar"
        );
        assert_eq!(
            bar(&[1.0, f64::NAN, 8.0], 9),
            "▁▁█",
            "the smallest and the largest sample keep their bars around a sample that is not a number"
        );
        assert_eq!(
            bar(&[1.0, 8.0], 9),
            "▁█",
            "the same two samples without it draw the same two bars"
        );
        assert_eq!(
            bar(&[f64::NAN, f64::NAN], 9),
            "▁▁",
            "a window of samples that none of them compare draws a flat line"
        );
        assert_eq!(
            bar(&[1.0, f64::INFINITY, 8.0], 9),
            "▁▁█",
            "an infinity does not compare either, and it takes the lowest bar"
        );
        assert_eq!(
            bar(&[1.0, f64::NEG_INFINITY, 8.0], 9),
            "▁▁█",
            "an infinity below zero takes the lowest bar and holds the scale off the floor"
        );
    }

    #[test]
    fn the_bar_holds_no_character_outside_the_set() {
        let drawn = bar(&[3.0, 1.0, 4.0, 1.0, 5.0, 9.0, 2.0, 6.0, f64::NAN], 9);
        assert_eq!(drawn.chars().count(), 9, "one bar stands for one sample");
        for character in drawn.chars() {
            assert!(
                BARS.contains(character),
                "the bar {drawn} holds {character}, which is not one of the eight block elements"
            );
        }
        assert!(
            !drawn.is_ascii(),
            "the sparkline has no ASCII fallback, so no bar of {drawn} is an ASCII character"
        );
    }

    #[test]
    fn the_bar_takes_one_column_for_each_sample_it_draws() {
        // A block element takes one terminal column, so the Recent column holds
        // as many samples as it is wide.
        let history = [3.0, 1.0, 4.0, 1.0, 5.0, 9.0, 2.0, 6.0, 5.0, 3.0, 5.0];
        for width in 0..=history.len() + 2 {
            let drawn = bar(&history, width);
            assert_eq!(
                display_width(&drawn),
                width.min(history.len()),
                "a width of {width} over {} samples draws {drawn}",
                history.len()
            );
        }
    }

    /// The width that the measured layout of the module documentation stands
    /// on.
    ///
    /// The test spells the number, and the module derives it from the widths of
    /// its columns. The two are on purpose: a test that read the constant of
    /// the module would agree with every width the module ever holds, and this
    /// width is the one a reader of a full-size terminal sees.
    const NOMINAL_WIDTH: u16 = 97;

    /// The terminal column that the Host column starts in.
    ///
    /// Four columns of TTL, and two of the gap behind it.
    const HOST_START: usize = 6;

    /// The number of columns between two columns of the table.
    const COLUMN_SPACING: usize = 2;

    /// The width of the Host column at the nominal width of the frame.
    const HOST_WIDTH: usize = 30;

    /// The number of fields that stand behind the Sent column: the five
    /// round-trip times and the sparkline.
    const TAIL_FIELDS: usize = 6;

    /// The address of the first router of the golden path.
    const FIRST_HOP: &str = "192.168.1.1";

    /// The address of the one router of the golden path that no name record
    /// named.
    const BARE_HOP: &str = "10.0.0.1";

    /// The address that answers first at the one TTL of the golden path that
    /// two routers answer at.
    const LEFT_ROUTER: &str = "203.0.113.8";

    /// The address that answers second at that TTL.
    const RIGHT_ROUTER: &str = "203.0.113.9";

    /// The address of the destination of the golden path.
    const TARGET: &str = "93.184.216.34";

    /// The line of the frame that holds the column header.
    const COLUMN_HEADER_LINE: usize = 2;

    /// The line of the golden frame that holds the TTL that never answered.
    const SILENT_TTL_LINE: usize = 4;

    /// The line of the golden frame that holds the TTL of one bare address.
    const BARE_TTL_LINE: usize = 5;

    /// The line of the golden frame that holds the TTL of two routers.
    ///
    /// The two address rows of that TTL stand under it, in the two lines that
    /// follow.
    const SHARED_TTL_LINE: usize = 6;

    /// The whole golden frame, line for line.
    ///
    /// The rounds of [`golden_table`] state the arithmetic of every number
    /// here. The frame holds a named host, a TTL that never answered, a bare
    /// address, a TTL of two routers with an address row for each of them, the
    /// star of the destination, and a sparkline.
    const GOLDEN_FRAME: [&str; 10] = [
        " krt  example.com → 93.184.216.34   src 1.2.3.4   round 4   1s   1.2.3.4-example.com.jsonl (2.1 MB)",
        "",
        " TTL  Host                             Loss%   Sent   Last    Min    Avg    Max  StDev  Recent",
        "   1  router.lan (192.168.1.1)          0.0%      4    5.0    1.0    3.0    5.0    2.0  ▁▁██",
        "   2  ???                             100.0%      4      -      -      -      -      -",
        "   3  10.0.0.1                         50.0%      4   12.0    8.0   10.0   12.0    2.0  ▁█",
        "   4  ae1.net (203.0.113.8) (+1)        0.0%      4   70.0   10.0   40.0   70.0   22.4  ▁▆▃█",
        "      ├ ae1.net (203.0.113.8)          50.0%▹     2   30.0   10.0   20.0   30.0   10.0  ▁█",
        "      └ 203.0.113.9                    50.0%▹     2   70.0   50.0   60.0   70.0   10.0  ▁█",
        "   5  example.com (93.184.216.34) ★     0.0%      4   60.0   40.0   50.0   60.0   10.0  ▁▁██",
    ];

    /// Folds every round into one table.
    fn table_of(rounds: &[RoundRecord]) -> HopTable {
        let mut table = HopTable::new();
        for record in rounds {
            table.observe(record);
        }
        table
    }

    /// The header of every frame that the tests below render.
    ///
    /// Four rounds, so the header line of the golden frame reads `round 4`.
    fn golden_header() -> Header<'static> {
        Header {
            rounds: 4,
            ..whole_header()
        }
    }

    /// The folded run of the golden frame.
    ///
    /// Four rounds probe TTL 1 to TTL 5. The numbers of every row follow.
    ///
    /// TTL 1 answers each round, from `192.168.1.1`, at 1.0, 1.0, 5.0, and 5.0.
    /// The loss is 0 / 4 = 0.0 percent. The mean is 12 / 4 = 3.0. The distances
    /// from the mean are -2, -2, 2, and 2, whose squares sum to 16, so the
    /// variance is 16 / 4 = 4.0 and the deviation is 2.0. The window of the
    /// sparkline runs from 1.0 to 5.0, so the two smallest samples take the
    /// lowest bar and the two largest take the highest.
    ///
    /// TTL 2 answers no round. The loss is 4 / 4 = 100.0 percent, and every
    /// statistic of it holds no value.
    ///
    /// TTL 3 answers two of the four rounds, from `10.0.0.1`, at 8.0 and 12.0.
    /// The loss is 2 / 4 = 50.0 percent. The mean is 20 / 2 = 10.0, the
    /// distances are -2 and 2, whose squares sum to 8, so the variance is
    /// 8 / 2 = 4.0 and the deviation is 2.0.
    ///
    /// TTL 4 answers each round, from two routers. `203.0.113.8` answers the
    /// first and the third round at 10.0 and 30.0, and `203.0.113.9` answers
    /// the second and the fourth at 50.0 and 70.0. Each of them therefore took
    /// 2 / 4 = 50.0 percent of the answers of the TTL. The mean of the TTL is
    /// 160 / 4 = 40.0, the distances are -30, 10, -10, and 30, whose squares
    /// sum to 2000, so the variance is 2000 / 4 = 500 and the deviation is the
    /// square root of 500, which is 22.36 and prints as 22.4. The mean of the
    /// left router is 40 / 2 = 20.0 and the mean of the right one is
    /// 120 / 2 = 60.0, and each of them stands 10.0 from both of its samples,
    /// so each deviation is 10.0.
    ///
    /// TTL 5 answers each round, from the destination, at 40.0, 40.0, 60.0, and
    /// 60.0. The mean is 200 / 4 = 50.0, and each sample stands 10.0 from it,
    /// so the deviation is 10.0.
    fn golden_table() -> HopTable {
        table_of(&[
            round(
                1,
                5,
                &[
                    (1, FIRST_HOP, 1.0),
                    (3, BARE_HOP, 8.0),
                    (4, LEFT_ROUTER, 10.0),
                    (5, TARGET, 40.0),
                ],
            ),
            round(
                1,
                5,
                &[
                    (1, FIRST_HOP, 1.0),
                    (4, RIGHT_ROUTER, 50.0),
                    (5, TARGET, 40.0),
                ],
            ),
            round(
                1,
                5,
                &[
                    (1, FIRST_HOP, 5.0),
                    (3, BARE_HOP, 12.0),
                    (4, LEFT_ROUTER, 30.0),
                    (5, TARGET, 60.0),
                ],
            ),
            round(
                1,
                5,
                &[
                    (1, FIRST_HOP, 5.0),
                    (4, RIGHT_ROUTER, 70.0),
                    (5, TARGET, 60.0),
                ],
            ),
        ])
    }

    /// The names that the `name` records of the golden run gave.
    ///
    /// `10.0.0.1` and `203.0.113.9` carry no name, so the two of them print
    /// their address alone.
    fn golden_names() -> BTreeMap<IpAddr, String> {
        names_of(&[
            (FIRST_HOP, "router.lan"),
            (LEFT_ROUTER, "ae1.net"),
            (TARGET, "example.com"),
        ])
    }

    /// One name for each address that a test names.
    fn names_of(names: &[(&str, &str)]) -> BTreeMap<IpAddr, String> {
        names
            .iter()
            .map(|(addr, name)| (address(addr), (*name).to_owned()))
            .collect()
    }

    /// The lines of a frame over one table, at a width.
    fn lines_of(
        table: &HopTable,
        names: &BTreeMap<IpAddr, String>,
        destination: Option<IpAddr>,
        width: u16,
    ) -> Vec<String> {
        Frame {
            header: golden_header(),
            table,
            names,
            destination,
        }
        .lines(width)
    }

    /// The lines of the golden frame at a width.
    fn golden_lines(width: u16) -> Vec<String> {
        let table = golden_table();
        let names = golden_names();
        lines_of(&table, &names, Some(address(TARGET)), width)
    }

    /// Every run of text of a line that holds no space, with the terminal
    /// column it starts in.
    ///
    /// The columns of the table are the measure of the frame, and a test that
    /// counted the characters of a line would read a wide glyph as one column.
    fn fields_with_columns(line: &str) -> Vec<(usize, String)> {
        let mut fields: Vec<(usize, String)> = Vec::new();
        let mut column = 0;
        let mut inside = false;
        for character in line.chars() {
            if character == ' ' {
                inside = false;
            } else {
                if inside {
                    if let Some(field) = fields.last_mut() {
                        field.1.push(character);
                    }
                } else {
                    fields.push((column, character.to_string()));
                    inside = true;
                }
            }
            column += UnicodeWidthChar::width(character).unwrap_or(0);
        }
        fields
    }

    /// The terminal column that each field of a line starts in.
    fn columns_of(line: &str) -> Vec<usize> {
        fields_with_columns(line)
            .into_iter()
            .map(|(column, _)| column)
            .collect()
    }

    /// The terminal column that the six fields behind the Sent column start in.
    fn tail_columns(line: &str) -> Vec<usize> {
        let columns = columns_of(line);
        assert!(
            columns.len() >= TAIL_FIELDS,
            "the line must hold the five times and the sparkline: {line}"
        );
        columns[columns.len() - TAIL_FIELDS..].to_vec()
    }

    /// The terminal column that one named field of a line starts in, or none
    /// when the line holds no such field.
    ///
    /// A frame that holds the wrong line must fail the assertion of the test
    /// that measures it, and must not stop that test with a panic. The reader
    /// of the failure then sees the column the test wanted beside the answer
    /// the frame gave.
    fn field_start(line: &str, wanted: &str) -> Option<usize> {
        fields_with_columns(line)
            .into_iter()
            .find(|(_, text)| text == wanted)
            .map(|(column, _)| column)
    }

    /// The line of a frame at an index, or an empty text when the frame holds
    /// no such line.
    fn line(lines: &[String], index: usize) -> &str {
        lines.get(index).map_or("", String::as_str)
    }

    /// The lines of a frame from an index, or none when the frame is shorter
    /// than that.
    fn lines_from(lines: &[String], index: usize) -> &[String] {
        lines.get(index..).unwrap_or_default()
    }

    /// The terminal column just behind the percent sign of a line.
    ///
    /// The Loss% of a TTL row and the Share% of an address row both end here,
    /// which is what puts the digits of the one under the digits of the other.
    fn percent_end(line: &str) -> usize {
        let mut column = 0;
        for character in line.chars() {
            column += UnicodeWidthChar::width(character).unwrap_or(0);
            if character == '%' {
                return column;
            }
        }
        panic!("the line must hold a percentage: {line}")
    }

    /// The text that the Host column of a line holds, without its padding.
    fn host_column(line: &str, host_width: usize) -> String {
        let mut text = String::new();
        let mut column = 0;
        for character in line.chars() {
            let width = UnicodeWidthChar::width(character).unwrap_or(0);
            if column >= HOST_START && column + width <= HOST_START + host_width {
                text.push(character);
            }
            column += width;
        }
        text.trim_end().to_owned()
    }

    #[test]
    fn the_frame_reads_as_the_layout_of_the_module_documentation_draws_it() {
        assert_eq!(
            golden_lines(NOMINAL_WIDTH),
            GOLDEN_FRAME,
            "the frame of a folded run reads as the module documentation draws it"
        );
    }

    #[test]
    fn an_address_row_lines_its_numbers_up_under_the_numbers_of_its_ttl() {
        let lines = golden_lines(NOMINAL_WIDTH);
        assert_eq!(
            lines.len(),
            GOLDEN_FRAME.len(),
            "the golden frame holds one line for each row of the folded path"
        );
        let ttl_row = line(&lines, SHARED_TTL_LINE);
        for offset in 1..=2 {
            let address_row = line(&lines, SHARED_TTL_LINE + offset);
            assert_eq!(
                percent_end(address_row),
                percent_end(ttl_row),
                "the share of {address_row} must end where the loss of {ttl_row} ends"
            );
            assert_eq!(
                tail_columns(address_row),
                tail_columns(ttl_row),
                "the times of {address_row} must stand under the times of {ttl_row}"
            );
        }
    }

    #[test]
    fn a_table_that_folded_no_round_prints_the_column_header_and_no_row() {
        let table = HopTable::new();
        let names = BTreeMap::new();
        let lines = lines_of(&table, &names, None, NOMINAL_WIDTH);
        assert_eq!(
            lines,
            [
                GOLDEN_FRAME[0],
                GOLDEN_FRAME[1],
                GOLDEN_FRAME[COLUMN_HEADER_LINE],
            ],
            "a run that folded no round still names itself and its columns"
        );
    }

    /// A host name of wide glyphs, one round long.
    const ONE_ROUND_TAIL: &str = "  0.0%      1    1.0    1.0    1.0    1.0    0.0  ▁";

    #[test]
    fn a_long_host_cuts_on_a_character_and_holds_the_column_behind_it() {
        // One round probes one TTL, and the router answers it at 1.0. The loss
        // is 0 / 1 = 0.0 percent, every time is 1.0, and one sample deviates
        // from itself by 0.0.
        //
        // Each name below is longer than the 30 columns of the Host column, so
        // each of them loses its tail. The Japanese name takes 22 of those
        // columns in 11 glyphs, and ` (192.16` takes the other 8. The nine
        // emoji take 18 columns, and ` (192.168.1.` takes the other 12. The
        // accented name is 23 characters of one column each, and ` (192.1`
        // takes the other 7: a cut by bytes would stop inside the `é`.
        let table = table_of(&[round(1, 1, &[(1, FIRST_HOP, 1.0)])]);
        let cases = [
            ("日本語のホスト名前一覧", "日本語のホスト名前一覧 (192.16"),
            ("🎉🎊🎁🎉🎊🎁🎉🎊🎁", "🎉🎊🎁🎉🎊🎁🎉🎊🎁 (192.168.1."),
            ("café-router.example.lan", "café-router.example.lan (192.1"),
        ];
        let mut starts = Vec::new();
        for (name, cut) in cases {
            let names = names_of(&[(FIRST_HOP, name)]);
            let lines = lines_of(&table, &names, None, NOMINAL_WIDTH);
            let row = line(&lines, COLUMN_HEADER_LINE + 1);
            assert_eq!(
                row,
                format!("   1  {cut}{COLUMN_SPACES}{ONE_ROUND_TAIL}"),
                "the host of {name} cuts on a character"
            );
            assert_eq!(
                display_width(cut),
                HOST_WIDTH,
                "the cut host of {name} fills the Host column and no more"
            );
            starts.push(field_start(row, "0.0%"));
        }
        assert_eq!(
            starts,
            vec![starts[0]; cases.len()],
            "the column behind the Host one starts in the same place for every name"
        );
    }

    /// The gap between two columns of the table, as a text.
    const COLUMN_SPACES: &str = "  ";

    #[test]
    fn the_star_marks_the_row_of_the_destination_and_no_other_row() {
        let lines = golden_lines(NOMINAL_WIDTH);
        let starred: Vec<&str> = lines
            .iter()
            .filter(|text| text.contains('★'))
            .map(String::as_str)
            .collect();
        assert_eq!(
            starred,
            [line(&lines, GOLDEN_FRAME.len() - 1)],
            "the last TTL of the golden path answered from the destination"
        );
    }

    #[test]
    fn the_star_reads_every_address_of_a_row_and_not_the_first_one_alone() {
        // The right router of TTL 4 answered second, so a check of the first
        // tracked address alone would leave the row unmarked.
        let table = golden_table();
        let names = golden_names();
        let lines = lines_of(&table, &names, Some(address(RIGHT_ROUTER)), NOMINAL_WIDTH);
        assert_eq!(
            host_column(line(&lines, SHARED_TTL_LINE), HOST_WIDTH),
            "ae1.net (203.0.113.8) (+1) ★",
            "the row of a TTL that answered from the destination takes the star"
        );
        let starred = lines.iter().filter(|text| text.contains('★')).count();
        assert_eq!(starred, 1, "one row of the path holds the destination");
    }

    #[test]
    fn a_frame_that_names_no_destination_marks_no_row() {
        let table = golden_table();
        let names = golden_names();
        let lines = lines_of(&table, &names, None, NOMINAL_WIDTH);
        assert_eq!(
            lines.len(),
            GOLDEN_FRAME.len(),
            "the frame still holds one line for each row of the folded path"
        );
        assert!(
            !lines.iter().any(|text| text.contains('★')),
            "a replay that never resolved a destination marks no row"
        );
    }

    #[test]
    fn a_ttl_of_three_routers_counts_two_more_participants() {
        // Three rounds probe TTL 1, and a different router answers each of
        // them. The row therefore tracks three addresses, and the two behind
        // the first one are the count that the host of the row carries.
        let table = table_of(&[
            round(1, 1, &[(1, "10.0.0.1", 10.0)]),
            round(1, 1, &[(1, "10.0.0.2", 20.0)]),
            round(1, 1, &[(1, "10.0.0.3", 30.0)]),
        ]);
        let names = BTreeMap::new();
        let lines = lines_of(&table, &names, None, NOMINAL_WIDTH);
        assert_eq!(
            host_column(line(&lines, COLUMN_HEADER_LINE + 1), HOST_WIDTH),
            "10.0.0.1 (+2)",
            "the host of the row names the first router and counts the other two"
        );
        let hosts: Vec<String> = lines_from(&lines, COLUMN_HEADER_LINE + 2)
            .iter()
            .map(|text| host_column(text, HOST_WIDTH))
            .collect();
        assert_eq!(
            hosts,
            ["├ 10.0.0.1", "├ 10.0.0.2", "└ 10.0.0.3"],
            "the last address row of the TTL closes the set"
        );
    }

    /// The number of addresses that one TTL keeps an entry for.
    ///
    /// The bound lives in `stats.rs`, and the test states it again: the row of
    /// the `others` line stands for the answers past this count, so a reader of
    /// the test reads what the count is.
    const TRACKED_ADDRESSES: usize = 32;

    /// The number of routers that answer at the crowded TTL of the test below.
    const CROWDED_ROUTERS: usize = 40;

    /// The round-trip time of every answer of that TTL.
    const CROWDED_RTT: f64 = 1.5;

    #[test]
    fn the_answers_that_no_tracked_address_holds_take_the_last_address_row() {
        // Forty rounds probe TTL 1, and a different router answers each of
        // them at 1.5. The row tracks the first 32 of those routers, so each of
        // them took 1 / 40 = 2.5 percent of the answers, and the other
        // 40 - 32 = 8 answers took 8 / 40 = 20.0 percent. The two counts sum to
        // the whole: 32 * 2.5 + 20.0 = 100.0.
        let rounds: Vec<RoundRecord> = (1..=CROWDED_ROUTERS)
            .map(|host| round(1, 1, &[(1, format!("10.0.0.{host}").as_str(), CROWDED_RTT)]))
            .collect();
        let table = table_of(&rounds);
        let names = BTreeMap::new();
        let lines = lines_of(&table, &names, None, NOMINAL_WIDTH);
        let address_rows = lines_from(&lines, COLUMN_HEADER_LINE + 2);
        assert_eq!(
            address_rows.len(),
            TRACKED_ADDRESSES + 1,
            "one row for each tracked router, and one for the answers of the rest"
        );
        assert_eq!(
            host_column(line(&lines, COLUMN_HEADER_LINE + 1), HOST_WIDTH),
            "10.0.0.1 (+32)",
            "the answers that no tracked address holds count as one more participant"
        );
        assert_eq!(
            address_rows.last().map_or("", String::as_str),
            "      └ others                         20.0%▹     8      -      -      -      -      -",
            "the row of the untracked answers stands last, and it holds no time"
        );
        let printed: f64 = address_rows.iter().map(|text| share_of(text)).sum();
        assert!(
            (printed - 100.0).abs() < 1e-9,
            "the printed shares of the TTL sum to the whole, and they sum to {printed}"
        );
    }

    /// The share that one address row prints, as a number.
    fn share_of(line: &str) -> f64 {
        let field = fields_with_columns(line)
            .into_iter()
            .map(|(_, text)| text)
            .find(|text| text.ends_with("%▹"))
            .unwrap_or_else(|| panic!("the address row must hold a share: {line}"));
        field
            .trim_end_matches('▹')
            .trim_end_matches('%')
            .parse()
            .unwrap_or_else(|_| panic!("the share of {line} must read as a number"))
    }

    /// A terminal wider than the nominal one.
    const WIDE_TERMINAL: u16 = 120;

    /// The columns that the Host column takes at that width.
    ///
    /// The frame without the Host column is 67 columns wide, so the Host column
    /// takes the other 120 - 67 = 53.
    const WIDE_HOST: usize = 53;

    /// A terminal too narrow for the frame at the floor of the Host column.
    const NARROW_TERMINAL: u16 = 70;

    /// The floor of the Host column.
    const HOST_MIN: usize = 12;

    #[test]
    fn the_host_column_absorbs_a_change_of_the_terminal_width() {
        let nominal = golden_lines(NOMINAL_WIDTH);
        let wide = golden_lines(WIDE_TERMINAL);
        assert_eq!(
            field_start(line(&wide, SILENT_TTL_LINE), "100.0%"),
            Some(HOST_START + WIDE_HOST + COLUMN_SPACING),
            "the Host column takes every column that the wider terminal added"
        );
        let grew = WIDE_HOST - HOST_WIDTH;
        let before = columns_of(line(&nominal, BARE_TTL_LINE));
        let after = columns_of(line(&wide, BARE_TTL_LINE));
        assert_eq!(
            before.len(),
            BARE_TTL_FIELDS,
            "the row of a TTL of one bare address holds {BARE_TTL_FIELDS} fields"
        );
        assert_eq!(
            after.len(),
            before.len(),
            "the wider frame holds the same fields"
        );
        // The TTL column and the Host column keep their place, and every column
        // behind the Host one moves by what the Host column took.
        let moved: Vec<usize> = before
            .iter()
            .enumerate()
            .map(|(index, column)| {
                if index < HELD_COLUMNS {
                    *column
                } else {
                    column + grew
                }
            })
            .collect();
        assert_eq!(
            after, moved,
            "the Host column is the one column that absorbs the change"
        );
    }

    /// The number of fields that the row of a TTL of one bare address holds:
    /// the TTL, the host, the loss, the count of the probes, the five times,
    /// and the sparkline.
    const BARE_TTL_FIELDS: usize = 10;

    /// The number of fields of such a row that a wider terminal does not move:
    /// the TTL and the host.
    const HELD_COLUMNS: usize = 2;

    #[test]
    fn the_host_column_stops_at_its_floor() {
        let narrow = golden_lines(NARROW_TERMINAL);
        assert_eq!(
            field_start(line(&narrow, SILENT_TTL_LINE), "100.0%"),
            Some(HOST_START + HOST_MIN + COLUMN_SPACING),
            "a terminal too narrow for the frame renders at the floor of the Host column"
        );
    }

    #[test]
    fn a_ttl_that_no_round_probed_prints_no_loss() {
        // The round probes TTL 1 and TTL 2, and a hop answers at TTL 5. The
        // row of TTL 5 therefore holds one answer and no probe, and a loss over
        // no probe is no number at all.
        let table = table_of(&[round(1, 2, &[(5, BARE_HOP, 1.0)])]);
        let names = BTreeMap::new();
        let lines = lines_of(&table, &names, None, NOMINAL_WIDTH);
        assert_eq!(
            lines.last().map_or("", String::as_str),
            "   5  10.0.0.1                             -      0    1.0    1.0    1.0    1.0    0.0  ▁",
            "a TTL that no round probed prints one word in the place of its loss"
        );
    }
}
