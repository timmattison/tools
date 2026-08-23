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

#[cfg(test)]
mod tests {
    use super::{
        display_width, render_duration, render_size, sparkline, truncate_to_width, Header,
    };
    use crate::testing::address;
    use std::time::Duration;

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
}
