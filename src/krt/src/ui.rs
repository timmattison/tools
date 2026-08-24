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
//! The table stands on those helpers. It holds nine columns at a terminal wide
//! enough for every one of them: the TTL, the host, one column that carries the
//! percentage with its mark and its count, the five round-trip times, and the
//! sparkline. The percentage, the mark, and the count are one column and not
//! three, because no gap stands between them: the mark says whether the
//! percentage is a loss or a share, so a gap in front of it would read as a
//! column of its own. The Host column is the one column that absorbs a change
//! of the terminal width.
//!
//! A host too wide for that column loses the tail of its name, and it keeps
//! the count of the other routers and the star of the destination. The name
//! takes the columns that the two marks leave. Both marks stand at the end of
//! the text, so a cut of the whole text takes them first, and a name with its
//! address fills the column of a run that resolves names: such a cut therefore
//! takes the marks off the common row and not off the rare one. A cut name
//! still reads as a name, and nothing else on the screen carries either mark.
//! The branch glyph of an address row stands at the start of its text, where
//! the cut spares it already.
//!
//! A terminal too narrow for the frame at the floor of the Host column drops
//! columns, one at a time, until the frame fits. The order, first dropped
//! first: `Recent`, `StDev`, `Max`, `Min`, `Last`, `Sent`, `Loss%`. It runs
//! from the column that says the least about one hop to the column that says
//! the most, so each drop gives up the cheapest column that stands. The TTL,
//! the Host, and the Avg never drop. The TTL and the host name the hop, and the
//! average is the one number that answers "how slow is this hop", which is the
//! question this tool exists to answer. A reader who lost every other column
//! still reads which hop is slow.
//!
//! The count of the probes goes away one step in front of the percentage, and
//! the two of them share one column with the mark between them. A column that
//! lost its count keeps the percentage and the mark. A column that lost the
//! percentage as well goes away whole, and the mark goes with it: the mark says
//! that a percentage is a share and not a loss, so it marks nothing where no
//! percentage stands. The tree glyph of the Host column still tells an address
//! row from a TTL row, so nothing goes away that a reader needs.
//!
//! The widths of the columns, the headings, and the cells of every row come out
//! of one list of columns, and not out of three lists that a reader must keep
//! in step. A column that leaves the list therefore takes its heading and its
//! cells with it. Three lists would agree at every width but one, and at that
//! width every cell behind the dropped column would land under the heading of
//! the column in front of it: a table that reads as though the run measured
//! something else. A frame of one width tells a reader nothing about that,
//! which is why the list is one list and not a rule to obey.
//!
//! A terminal too narrow even for the last set of columns gets the frame at the
//! floor of the Host column, and the terminal clips it. Nothing narrower says
//! more, and a frame of no columns tells a reader nothing.
//!
//! The terminal says how wide the frame is, and a run whose standard output is
//! a pipe or a file has no terminal to ask. A terminal that carries no window
//! says nothing either: it reports a width of zero, which measures no window at
//! all. Each of those runs draws at the nominal width, which is every column of
//! the table with a Host column of 30. A reader who redirects a replay asked for
//! the whole table, and a frame cut to the window that the run started in would
//! drop columns into a file that nothing ever gets back. It also makes the
//! output of one recorded file one text on every machine, which is what a test
//! of the binary reads.
//!
//! The address rows of one TTL line their Share% up under the Loss% of the row
//! above them, digit for digit. The drawing this table came from puts those
//! digits one column further right, and that is the one place where the render
//! and the drawing part company. An address row stands under its TTL row so a
//! reader compares the two by column, and a percentage one column off its own
//! heading defeats the whole reason for the row. Do not "correct" it back.
//!
//! A TTL that answered from more addresses than it tracks closes its set of
//! address rows with one more row, whose host is the word `others`. That row
//! carries the count of those answers and the share they took, and no time at
//! all: `stats.rs` keeps no times for an answer that holds no tracked address.
//! A render that dropped the row would leave the printed shares of the TTL
//! short of the whole, and nothing on the screen would say why.
//!
//! The render draws through `ratatui`, into a buffer that no terminal ever
//! sees, and reads the lines back out of it. A `ratatui` buffer keeps the style
//! of a cell beside the symbol of that cell and never inside it, so a read of
//! the symbols alone gives glyphs alone. The read is therefore the one place
//! that writes a color code, and it writes one only for the one cell of the
//! table that carries a color: a probe that no hop answered, which the Recent
//! column draws red. Nothing else of the render holds a code, and no code ever
//! reaches the buffer, where it would take columns of the table that no reader
//! sees.
//!
//! The color is a decision of the caller, which [`Paint`] carries, and not a
//! decision that this module takes off the terminal. A live table asks for the
//! color, because it draws on a terminal by construction: the run holds that
//! terminal in raw mode on the alternate screen. A replay asks for the plain
//! lines. This crate thus takes no `colored` dependency, and no test of this
//! render needs `testcolor`: the two codes are constants of this module, and a
//! test names them.
//!
//! The loss carries a glyph of its own as well as the color, because the run
//! that prints no color is the normal one. A headless run, a pipe, a file, and
//! a replay each print text with no color, and a red `▇` alone would read on
//! every one of them as the slowest sample of the window.
//!
//! The module also writes the one duration text of the crate. The resolved
//! configuration, the status line of a round, and the header line of the frame
//! each name a period of time, and a second writer of a duration would print
//! `1s` in one of those three places and `1000ms` in another.

use crate::stats::{Address, HopStats, Sample, TtlRow};
use crate::{ROUND, SECONDS_PER_HOUR, SECONDS_PER_MINUTE, UNKNOWN};
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Cell, Row, Table, Widget};
use std::collections::BTreeMap;
use std::fmt;
use std::io::IsTerminal;
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
/// Three spaces, and not the two of a status line. Two of these fields hold a
/// space of their own — `src 1.2.3.4` and `round 142` each do — so a narrower
/// gap would read as one sentence, and a reader would have to know the words to
/// find where one field stops.
const FIELD_SEPARATOR: &str = "   ";

/// The glyph between a destination and the address that it resolved to.
///
/// The destination is what the user typed, and the address is what the resolver
/// answered. The arrow says which is which. A row of the table writes the same
/// pair as `name (address)`, because a row names one router of the path, where
/// this line heads the whole path to that one address.
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

/// The control sequence that paints what follows it red.
const RED: &str = "\u{1b}[31m";

/// The control sequence that gives the foreground of the terminal back.
///
/// The code names the default foreground of the terminal, and not a reset of
/// every attribute, so it takes the red back and touches nothing else that the
/// terminal holds.
const PLAIN: &str = "\u{1b}[39m";

/// Whether the lines of a frame carry the color of a terminal.
///
/// The caller decides, because the caller knows where its lines go. A live
/// table draws on a terminal by construction, and a replay prints into whatever
/// scrollback the terminal keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Paint {
    /// The lines hold glyphs alone.
    Plain,
    /// The mark of a lost probe stands between the codes that paint it red.
    Colored,
}

/// The number of bars that the sparkline draws with.
const LEVEL_COUNT: usize = 7;

/// The bars of the sparkline, lowest first.
///
/// The set stops at `▇` and holds no `█`. A `█` paints the whole height of its
/// cell, and the rows of the table stand one under the other, so a `█` of one
/// row touches the bar of the row above it. The line between the two rows goes
/// away, and a reader reads one block of ink. The empty top eighth that `▇`
/// leaves is that line. The scale runs from the smallest sample of a window to
/// the largest one, so the drop costs one step of resolution and no range at
/// all.
///
/// There is no ASCII fallback, and `CLAUDE.md` asks for it that way. A fallback
/// of `.:|#` characters would draw a second, coarser picture of the same
/// numbers, so a reader of one terminal and a reader of another would argue
/// over a hop that the two pictures disagree about. Every terminal that this
/// tool draws a table on prints these seven glyphs, and each of them takes one
/// column, so the Recent column holds one sample for each column it is wide.
const BARS: [char; LEVEL_COUNT] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇'];

/// The mark of a probe that no hop answered.
///
/// The mark is no bar of [`BARS`], and that is what it is for. The color of a
/// terminal is not always there: a headless run, a pipe, a file, and a replay
/// each print text with no color. A red `▇` alone would read as the slowest
/// sample of the window on every one of those runs, and a reader would take a
/// lost probe for a slow answer.
///
/// The glyph takes one terminal column, as each bar does, so one sample of the
/// history stands in one column of the Recent column either way.
const NO_ANSWER: char = '╳';

/// The count of the bars, as the arithmetic of the scale reads it.
#[expect(
    clippy::cast_precision_loss,
    reason = "the count of the bars is 7, and an `f64` holds every whole number below 2^53"
)]
const LEVELS: f64 = LEVEL_COUNT as f64;

/// One glyph of the Recent column, and what that glyph stands for.
///
/// The mark carries the meaning and not the color, so the one place that names
/// a color is [`Mark::style`]. A column of glyphs alone would have to carry the
/// codes of the color inside its text, and those codes would then reach the
/// `ratatui` buffer as glyphs and take columns of the table that no reader ever
/// sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mark {
    /// A glyph that stands in the foreground of the terminal: a bar of a
    /// round-trip time, or a character of the heading of the column.
    Glyph(char),
    /// A probe that no hop answered.
    Lost,
}

impl Mark {
    /// The glyph that the mark draws.
    fn glyph(self) -> char {
        match self {
            Self::Glyph(glyph) => glyph,
            Self::Lost => NO_ANSWER,
        }
    }

    /// The style that the mark draws in.
    ///
    /// A bar takes the foreground of the terminal, whatever that is. A reader
    /// sets the colors of their own terminal, and a table that painted every
    /// bar would argue with that choice for no gain: the height of the bar
    /// already says what the bar says.
    fn style(self) -> Style {
        match self {
            Self::Glyph(_) => Style::default(),
            Self::Lost => Style::default().fg(Color::Red),
        }
    }
}

/// The Recent column of one row: one mark for each sample of the window.
///
/// The column travels as marks and not as text, because the glyphs of it do not
/// all share a color. It is the one column of the table that carries a color at
/// all.
pub(crate) struct Recent(Vec<Mark>);

impl Recent {
    /// The column of a row that draws no sample at all.
    fn empty() -> Self {
        Self(Vec::new())
    }

    /// The column that draws one text in the foreground of the terminal.
    ///
    /// The column header takes this. Its heading is a word and not a picture of
    /// a history, so every glyph of it stands in the color the reader set.
    fn text(text: &str) -> Self {
        Self(text.chars().map(Mark::Glyph).collect())
    }

    /// The cell of the table that draws this column.
    ///
    /// One span for each mark, so the `ratatui` buffer keeps the color of a
    /// loss beside the symbol of it, and the read of the buffer finds the color
    /// there. A cell of one style would paint the whole column or none of it.
    fn cell(&self, alignment: Alignment) -> Cell<'static> {
        let spans: Vec<Span<'static>> = self
            .0
            .iter()
            .map(|mark| Span::styled(mark.glyph().to_string(), mark.style()))
            .collect();
        Cell::from(Text::from(Line::from(spans)).alignment(alignment))
    }
}

impl fmt::Display for Recent {
    /// The glyphs of the column, with no color at all.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for mark in &self.0 {
            write!(formatter, "{}", mark.glyph())?;
        }
        Ok(())
    }
}

/// The recent round-trip times of one key, as a bar for each of them.
///
/// The bar shows the last `width` samples. A key that holds more samples than
/// that drops its oldest ones, because the column is as wide as the terminal
/// gives it and the most recent samples are the ones that say what the hop is
/// doing now.
///
/// The scale runs from the smallest to the largest sample **of the shown
/// window**, and not of the whole history: the smallest takes `▁` and the
/// largest takes `▇`. A scale over the whole history would flatten the window
/// of a hop that was once slow and is not slow now, and that window is the part
/// a reader is looking at.
///
/// A window whose samples are all equal draws `▁` for each of them. Nothing
/// varies, and a flat line at the floor says that. The alternative, a flat line
/// at the top, would draw the quietest hop of the path as the loudest one.
///
/// A probe that no hop answered draws [`NO_ANSWER`] at the place of that probe,
/// and it takes no part in the scale: such a probe measured no time, so no
/// limit of the window can read it.
///
/// A key with no sample, and a `width` of zero, each draw an empty column.
///
/// A sample that is not a finite number — `f64::NAN`, or an infinity — draws
/// the lowest bar, and the scale does not read it at all. Such a sample does
/// not compare, so a scale that took it would carry a limit that no bar can
/// stand on, and every bar of the key would then read the same. One bad sample
/// would hide the whole history of the hop, which is a worse answer than one
/// bar that reads low.
pub(crate) fn sparkline(samples: impl ExactSizeIterator<Item = Sample>, width: usize) -> Recent {
    if width == 0 {
        return Recent::empty();
    }

    // The iterator states its length before it gives a sample, so the oldest
    // samples go away as the iterator runs. The window is therefore the only
    // copy that this function makes, and the history of the key stays where it
    // is.
    let skipped = samples.len().saturating_sub(width);
    let shown: Vec<Sample> = samples.skip(skipped).collect();

    // The scale reads only the answers that compare. A lost probe measured no
    // time, so it names no limit of the window.
    let mut lowest = f64::INFINITY;
    let mut highest = f64::NEG_INFINITY;
    for time in shown.iter().filter_map(finite_time) {
        lowest = lowest.min(time);
        highest = highest.max(time);
    }
    let span = highest - lowest;

    Recent(
        shown
            .iter()
            .map(|sample| {
                let Sample::Time(time) = *sample else {
                    return Mark::Lost;
                };
                // A window of one sample, and a window whose samples are all
                // equal, each give a span of zero. A window that holds no
                // sample which compares gives a span below zero, because the
                // two limits stayed at the infinities that the fold started
                // them at. Neither window divides.
                if span <= 0.0 || !time.is_finite() {
                    Mark::Glyph(BARS[0])
                } else {
                    Mark::Glyph(bar_at((time - lowest) / span))
                }
            })
            .collect(),
    )
}

/// The round-trip time of one sample, when that sample holds a time which
/// compares.
///
/// A lost probe measured no time, and a time that is not a finite number does
/// not compare, so the scale of a window reads neither of them.
fn finite_time(sample: &Sample) -> Option<f64> {
    match *sample {
        Sample::Time(time) if time.is_finite() => Some(time),
        _ => None,
    }
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

/// The number of columns of the TTL column.
///
/// A TTL is a number of three digits at most, and the fourth column is the one
/// space that holds the whole block one column in from the left edge of the
/// terminal, as the header line above it stands.
const TTL_WIDTH: u16 = 4;

/// The fewest columns that the Host column ever takes.
///
/// A host cut to fewer columns than this names no router: `ae-1.core.exam` is
/// already a guess, and half of that is a shape. A terminal too narrow to hold
/// the frame at this floor gets the frame at the floor anyway, and it clips the
/// rest. A frame that shrank the column further would print a table that reads
/// as though the path had changed.
const HOST_MIN: u16 = 12;

/// The number of columns that a percentage takes.
///
/// `100.0%` is the longest one the table prints, and it fills the six.
const PERCENT_WIDTH: u16 = 6;

/// The number of columns of the mark that tells a share from a loss.
const MARK_WIDTH: u16 = 1;

/// The number of columns that a count of probes or of answers takes.
const SENT_WIDTH: u16 = 6;

/// The number of columns of the column that carries a percentage and its mark,
/// after a narrow terminal dropped the count behind them.
///
/// The two stand next to each other with no gap, so the table holds them as one
/// column and not as two.
const MARKED_PERCENT_WIDTH: u16 = PERCENT_WIDTH + MARK_WIDTH;

/// The number of columns of the one column that carries a percentage, its mark,
/// and its count.
///
/// The three stand next to each other with no gap, so the table holds them as
/// one column and not as three.
const COUNTS_WIDTH: u16 = MARKED_PERCENT_WIDTH + SENT_WIDTH;

/// The number of columns that one round-trip time takes.
///
/// `999.9` fills them. A hop slower than one second loses its leading digits,
/// and every such hop is one a reader already reads as broken from the loss
/// beside it.
const TIME_WIDTH: u16 = 5;

/// The number of columns of the table that hold a round-trip time: `Last`,
/// `Min`, `Avg`, `Max`, and `StDev`.
///
/// There is no `Jitter` column. The jitter reads the last two answers alone, so
/// it moves with every round and it says nothing over a folded run that the
/// deviation does not say better.
const TIME_COLUMNS: usize = 5;

/// The number of columns of the sparkline.
const RECENT_WIDTH: u16 = 9;

/// The number of columns between two columns of the table.
///
/// Two, and not one. Every column but the Host one holds a number, and a
/// number that stands one column from the number beside it reads as one longer
/// number.
const COLUMN_SPACING: u16 = 2;

/// The number of lines that stand above the table: the header line, and the
/// blank line under it.
const HEADER_LINES: u16 = 2;

/// The number of lines of the column header.
const COLUMN_HEADER_LINES: u16 = 1;

/// The number of lines of a frame that stand above the rows of the path.
///
/// A caller that fits a frame to the window of a terminal keeps these lines and
/// drops rows, so the reader keeps the destination, the count of the rounds,
/// the size of the recorded file, and the name of every column. The count comes
/// off the two constants above, because a head that grew by one line would
/// otherwise take one row of the path with it and no line of that caller would
/// say so.
pub(crate) const HEAD_LINES: u16 = HEADER_LINES + COLUMN_HEADER_LINES;

/// The host of a TTL that never answered.
const NO_HOST: &str = "???";

/// The value of a number that the run holds none of.
const NO_NUMBER: &str = "-";

/// The sign that ends a percentage.
const PERCENT_SIGN: &str = "%";

/// The number of decimal places of every round-trip time and every percentage.
const DECIMALS: usize = 1;

/// The whole of a percentage.
const PERCENT: f64 = 100.0;

/// The mark of a TTL row.
///
/// One space. The percentage of a TTL row is a loss, and the column header
/// above it says `Loss%`, so the row needs no mark of its own.
const LOSS_MARK: &str = " ";

/// The mark of an address row.
///
/// The percentage of an address row is a share of the answers of its TTL, and
/// not a loss. The glyph points at the row above, where the whole that the
/// share is a part of stands.
const SHARE_MARK: &str = "▹";

/// The start of the Host column of an address row that another one follows.
const BRANCH: &str = "├ ";

/// The start of the Host column of the last address row of a TTL.
const LAST_BRANCH: &str = "└ ";

/// The host of the row that stands for the answers of a TTL that no tracked
/// address holds.
const OTHERS: &str = "others";

/// The mark of the row that answered from the destination.
const DESTINATION_MARK: &str = "★";

/// The heading of the TTL column.
const TTL_HEADER: &str = "TTL";

/// The heading of the Host column.
const HOST_HEADER: &str = "Host";

/// The heading of the percentage that a TTL row prints.
const LOSS_HEADER: &str = "Loss%";

/// The heading of the count that a TTL row prints.
const SENT_HEADER: &str = "Sent";

/// The heading of the round-trip time of the most recent answer.
const LAST_HEADER: &str = "Last";

/// The heading of the shortest round-trip time.
const MIN_HEADER: &str = "Min";

/// The heading of the mean round-trip time.
const AVG_HEADER: &str = "Avg";

/// The heading of the longest round-trip time.
const MAX_HEADER: &str = "Max";

/// The heading of the deviation of the round-trip times.
const STDEV_HEADER: &str = "StDev";

/// The heading of the five round-trip time columns, in the order they stand.
const TIME_HEADERS: [&str; TIME_COLUMNS] = [
    LAST_HEADER,
    MIN_HEADER,
    AVG_HEADER,
    MAX_HEADER,
    STDEV_HEADER,
];

/// The heading of the sparkline column.
const RECENT_HEADER: &str = "Recent";

/// The number of columns that a narrow terminal drops.
const DROPPABLE_COLUMNS: usize = 7;

/// The columns that a narrow terminal drops, first dropped first.
///
/// A heading names each of them, because the heading is what a reader of the
/// table sees of a column and no two headings read the same. The order runs
/// from the column that says the least about one hop to the column that says
/// the most, so every drop gives up the cheapest column that stands.
///
/// The TTL, the Host, and the Avg stand nowhere in this list, and that absence
/// is how the module says that no terminal drops them: the TTL and the host
/// name the hop, and the average says how slow that hop is.
const DROP_ORDER: [&str; DROPPABLE_COLUMNS] = [
    RECENT_HEADER,
    STDEV_HEADER,
    MAX_HEADER,
    MIN_HEADER,
    LAST_HEADER,
    SENT_HEADER,
    LOSS_HEADER,
];

/// Reads a count as the number that a percentage divides.
#[expect(
    clippy::cast_precision_loss,
    reason = "an `f64` holds every whole number below 2^53, and a probe run counts one answer for one TTL of one round, so no count of a run reaches that point"
)]
fn count_as_f64(count: u64) -> f64 {
    count as f64
}

/// Reads a number of terminal columns as a width of the buffer.
///
/// A terminal is never as wide as `u16::MAX` columns, and a text that somehow
/// were takes the whole buffer and loses nothing that a reader would see.
fn buffer_columns(text: &str) -> u16 {
    u16::try_from(display_width(text)).unwrap_or(u16::MAX)
}

/// Writes one round-trip time, to `DECIMALS` decimal places.
///
/// A key that holds no such time takes one word in its place. A TTL that
/// answered no probe holds none of the times.
fn render_time(value: Option<f64>) -> String {
    value.map_or_else(
        || NO_NUMBER.to_owned(),
        |value| format!("{value:.DECIMALS$}"),
    )
}

/// Writes one percentage, to `DECIMALS` decimal places.
fn render_percent(value: f64) -> String {
    format!("{value:.DECIMALS$}{PERCENT_SIGN}")
}

/// The one column that carries a percentage, its mark, and its count.
///
/// The percentage takes the columns in front of the mark and the count takes
/// the columns behind it, so the digits of a share land under the digits of the
/// loss of the row above it however long either number is. A terminal that
/// dropped the count gives none, and the column then ends at the mark.
fn counts_text(percent: &str, mark: &str, count: Option<&str>) -> String {
    let count = count.map_or_else(String::new, |count| {
        format!(
            "{count:>count_width$}",
            count_width = usize::from(SENT_WIDTH)
        )
    });
    format!(
        "{percent:>percent_width$}{mark}{count}",
        percent_width = usize::from(PERCENT_WIDTH),
    )
}

/// One column of the table, as the render draws it.
///
/// The slot carries the width of the column, the side of the column that its
/// text holds to, and the field of a row that fills it. The constraints of the
/// table, the headings, and the cells of every row therefore come out of one
/// list of these: a column that leaves the list takes its cells with it, and no
/// cell of a row can land under the heading of another column.
#[derive(Clone, Copy)]
struct Slot {
    /// The number of terminal columns that the column takes.
    width: u16,
    /// The side of the column that its text holds to.
    alignment: Alignment,
    /// The field of a row that fills the column.
    field: Field,
}

/// The field of a row that one column of the table draws.
#[derive(Clone, Copy)]
enum Field {
    /// The TTL of the row.
    Ttl,
    /// The router that answered.
    Host,
    /// The percentage with its mark, and the count of the probes or of the
    /// answers behind them while that count stands.
    Counts {
        /// Whether the count stands.
        sent: bool,
    },
    /// One round-trip time, at the place it stands among the times.
    Time(usize),
    /// The sparkline of the recent round-trip times.
    Recent,
}

impl Field {
    /// The cell that the field draws out of one row.
    ///
    /// A time reads the place it stands among the times, and
    /// [`standing_slots`] takes that place from [`TIME_HEADERS`]. The times of
    /// a row stand in the order of that same list, so the heading of a time and
    /// the number under it always name the same statistic.
    ///
    /// The Recent column builds its own cell. It is the one column of the table
    /// whose glyphs do not all share a color, so it holds marks and not text.
    fn cell(self, row: &RowText, alignment: Alignment) -> Cell<'static> {
        let text = match self {
            Self::Ttl => row.ttl.clone(),
            Self::Host => row.host.clone(),
            Self::Counts { sent } => {
                counts_text(&row.percent, row.mark, sent.then_some(row.sent.as_str()))
            }
            Self::Time(index) => row.times[index].clone(),
            Self::Recent => return row.recent.cell(alignment),
        };
        cell(text, alignment)
    }
}

/// The columns that stand after the first `dropped` columns of [`DROP_ORDER`]
/// went away, in the order the columns stand.
///
/// The Host column takes the width the caller gives it. Every other column
/// knows the widest text it ever prints, so its width is a constant of this
/// module.
fn standing_slots(dropped: usize, host: u16) -> Vec<Slot> {
    let gone = DROP_ORDER.get(..dropped).unwrap_or(&DROP_ORDER);
    let holds = |heading: &str| !gone.contains(&heading);
    let mut slots = vec![
        Slot {
            width: TTL_WIDTH,
            alignment: Alignment::Right,
            field: Field::Ttl,
        },
        Slot {
            width: host,
            alignment: Alignment::Left,
            field: Field::Host,
        },
    ];
    if holds(LOSS_HEADER) {
        // The count goes away one step in front of the percentage, so this
        // column loses six of its columns before it goes away whole.
        let sent = holds(SENT_HEADER);
        slots.push(Slot {
            width: if sent {
                COUNTS_WIDTH
            } else {
                MARKED_PERCENT_WIDTH
            },
            alignment: Alignment::Left,
            field: Field::Counts { sent },
        });
    }
    for (index, heading) in TIME_HEADERS.iter().enumerate() {
        if holds(heading) {
            slots.push(Slot {
                width: TIME_WIDTH,
                alignment: Alignment::Right,
                field: Field::Time(index),
            });
        }
    }
    if holds(RECENT_HEADER) {
        slots.push(Slot {
            width: RECENT_WIDTH,
            alignment: Alignment::Left,
            field: Field::Recent,
        });
    }
    slots
}

/// The number of columns that a set of columns takes, with the gaps between
/// them.
///
/// One gap stands between two columns, so a set of one column holds no gap and
/// an empty set holds none either.
fn total_width(slots: &[Slot]) -> u16 {
    let gaps = u16::try_from(slots.len().saturating_sub(1)).unwrap_or(u16::MAX);
    slots
        .iter()
        .fold(gaps.saturating_mul(COLUMN_SPACING), |total, slot| {
            total.saturating_add(slot.width)
        })
}

/// The number of columns that the frame takes with a Host column of `host`
/// columns, after `dropped` columns of the order went away.
///
/// The width reads the same list of columns that the render draws, so a column
/// that changes its width, or that goes away, moves the frame with it. No line
/// of this module counts the columns or the gaps by hand.
///
/// The nominal frame is 97 columns wide, which leaves the Host column 30 of
/// them. Neither number stands in this module as a constant: the terminal says
/// how wide the frame is, and [`Layout::at`] says what the columns then take.
fn frame_width(dropped: usize, host: u16) -> u16 {
    total_width(&standing_slots(dropped, host))
}

/// The number of columns of the Host column of a frame that no terminal
/// measured.
///
/// Thirty columns hold `ae-1.core.example.net`, which is the shape of the name
/// that a router of a backbone carries. The frame is then 97 columns wide, and
/// both numbers come out of the drawing that this table stands on.
const HOST_NOMINAL: u16 = 30;

/// The number of terminal columns that a frame draws in.
///
/// A terminal answers with its own width, so the table fills the window and no
/// more. Standard output that is a pipe or a file holds no width to ask for,
/// and it answers with the nominal frame, for the reason that the module
/// documentation states above.
///
/// A terminal that reports no size answers with the nominal frame as well. Two
/// terminals report no size: one that the probe failed to read, and one that
/// carries no window. The second answers the `TIOCGWINSZ` ioctl with zero
/// columns, and that ioctl succeeds; a pseudo-terminal that nobody ever sized is
/// such a terminal, and `script -q /dev/null` makes one. `termsize` gives `None`
/// for both, which is why the probe of this function is `termsize` and not the
/// raw call. The nominal frame then stands too wide or too narrow for the one
/// window, and the terminal clips it, where a frame of no columns would drop
/// every column that drops and cut the Host column to its floor.
pub(crate) fn frame_columns() -> u16 {
    if !std::io::stdout().is_terminal() {
        return frame_columns_of(None);
    }
    frame_columns_of(termsize::controlling_columns())
}

/// The number of terminal columns that a frame draws in, and the number of
/// rows that the window holds.
///
/// The columns follow the rule of [`frame_columns`], and the rows come straight
/// off the probe. The rows are `None` for a window that no probe measured: a
/// run whose standard output is no terminal, a terminal that the probe failed
/// to read, and a terminal that carries no window. A caller that holds no row
/// count holds no height to fit a frame to, and it draws every line of that
/// frame.
///
/// One probe answers both numbers, because the terminal answers both of them in
/// one ioctl. A second call would ask the same terminal the same question, and
/// a window that changed size between the two would answer with a width of one
/// window and a height of another.
///
/// A live table asks this question, because it draws its frames into a window
/// that holds a fixed number of rows. A replay asks [`frame_columns`], because
/// it prints its lines into whatever scrollback the terminal keeps.
pub(crate) fn frame_size() -> (u16, Option<u16>) {
    if !std::io::stdout().is_terminal() {
        return (frame_columns_of(None), None);
    }
    let size = termsize::controlling_size();
    (
        frame_columns_of(size.map(|(columns, _)| columns)),
        size.map(|(_, rows)| rows),
    )
}

/// The number of terminal columns that a frame draws in, from the answer that
/// the terminal gave.
///
/// The read of the terminal stands apart from this decision, so a test names
/// the answer of a terminal without a terminal to name it with.
///
/// `None` is a run that measured no terminal, and it draws the nominal frame. A
/// width of zero draws the nominal frame as well, because no character of a row
/// prints into no column.
///
/// `termsize` already gives `None` for a width of zero, so no zero reaches this
/// function through [`frame_columns`]. The rule stays here because this function
/// is where `krt` states what it draws at, and the test of the rule is what
/// keeps it true if the probe of [`frame_columns`] ever changes.
fn frame_columns_of(answer: Option<u16>) -> u16 {
    match answer {
        Some(columns) if columns > 0 => columns,
        _ => frame_width(0, HOST_NOMINAL),
    }
}

/// The columns that a terminal width holds, and the width of each of them.
///
/// The layout is the one answer to "what does this terminal hold". The widths
/// of the columns, the headings, and the cells of every row all read it, so no
/// two of the three can disagree about which columns stand.
struct Layout {
    /// The columns that stand, in the order they stand.
    slots: Vec<Slot>,
    /// The number of columns of the Host column.
    host: u16,
}

impl Layout {
    /// The layout of the frame at a terminal width.
    ///
    /// The columns go away in the order of [`DROP_ORDER`], one at a time, while
    /// the frame at the floor of the Host column is wider than the terminal.
    /// The Host column then takes every column that the rest of the frame
    /// leaves, and never less than that floor: every other column holds a
    /// number whose widest print the run already knows.
    ///
    /// A terminal too narrow even for the last set of columns gets the frame at
    /// the floor, and the terminal clips it. A table that is one column too
    /// wide still reads, a table whose hosts are three characters long does
    /// not, and a table of no columns says nothing at all.
    fn at(width: u16) -> Self {
        let mut dropped = 0;
        while dropped < DROPPABLE_COLUMNS && frame_width(dropped, HOST_MIN) > width {
            dropped += 1;
        }
        let host = width.saturating_sub(frame_width(dropped, 0)).max(HOST_MIN);
        Self {
            slots: standing_slots(dropped, host),
            host,
        }
    }

    /// The number of columns that the frame takes.
    fn width(&self) -> u16 {
        total_width(&self.slots)
    }

    /// The number of columns of the Host column.
    ///
    /// Every row cuts its host to it. A row that cut to the terminal width
    /// instead would print a name over the column behind it.
    fn host(&self) -> u16 {
        self.host
    }

    /// The width of every column that stands, in the order the columns stand.
    fn constraints(&self) -> Vec<Constraint> {
        self.slots
            .iter()
            .map(|slot| Constraint::Length(slot.width))
            .collect()
    }

    /// The row that the table draws for one set of texts.
    ///
    /// The TTL and the times hold to the right of their columns, because a
    /// reader of a column of numbers compares the last digit of one against the
    /// last digit of the next. The host and the sparkline hold to the left,
    /// because both of them read from their first character. The counts column
    /// aligns itself: its text fills the column exactly.
    fn row(&self, text: &RowText) -> Row<'static> {
        Row::new(
            self.slots
                .iter()
                .map(|slot| slot.field.cell(text, slot.alignment))
                .collect::<Vec<Cell<'static>>>(),
        )
    }
}

/// One cell of the table, with its text held to one side of its column.
fn cell(text: String, alignment: Alignment) -> Cell<'static> {
    Cell::from(Text::from(text).alignment(alignment))
}

/// The text of every column of one row of the table.
///
/// The row is text alone, and it holds the text of every column that the table
/// ever draws. The widths and the alignments belong to the columns, so a row
/// that carried them would state the layout once for each row of the path, and
/// the [`Layout`] says which of these fields a terminal holds. Nothing of a row
/// changes with the width of the terminal but the cut of its host.
struct RowText {
    /// The TTL of the row. An address row names none.
    ttl: String,
    /// The host of the row, already cut to the Host column.
    host: String,
    /// The percentage of the row: the loss of a TTL, or the share of one router
    /// of that TTL.
    percent: String,
    /// The mark that tells a share from a loss.
    mark: &'static str,
    /// The count of the probes of a TTL, or of the answers of one router.
    sent: String,
    /// The five round-trip times, in the order the columns stand.
    times: [String; TIME_COLUMNS],
    /// The sparkline of the recent samples.
    recent: Recent,
}

/// The headings that stand above the rows of the path.
///
/// The headings are a row like every other row, and the [`Layout`] draws them
/// with the list of columns that draws every row. A heading can therefore never
/// stand over the cells of another column.
fn column_header() -> RowText {
    RowText {
        ttl: TTL_HEADER.to_owned(),
        host: HOST_HEADER.to_owned(),
        percent: LOSS_HEADER.to_owned(),
        mark: LOSS_MARK,
        sent: SENT_HEADER.to_owned(),
        times: TIME_HEADERS.map(str::to_owned),
        recent: Recent::text(RECENT_HEADER),
    }
}

/// The five round-trip times of one key, in the order the columns stand.
fn time_texts(stats: &HopStats) -> [String; TIME_COLUMNS] {
    [
        stats.last(),
        stats.min(),
        stats.avg(),
        stats.max(),
        stats.stddev(),
    ]
    .map(render_time)
}

/// The number of routers that answered at one TTL, as the table counts them.
///
/// The routers that the row tracks, and one more for the answers of the rest.
/// The count of the routers behind that bound is a count the fold does not
/// keep, and cannot keep in a bounded amount of memory, so the answers of all
/// of them stand as one participant of the TTL.
fn participants(row: &TtlRow) -> usize {
    row.addresses().len() + usize::from(row.untracked() > 0)
}

/// The share of the answers of a TTL that no tracked address holds.
///
/// The divisor is never zero. A row counts an untracked answer only after it
/// folded that same answer into the statistics of the TTL, so a row with an
/// untracked answer holds an answer.
fn untracked_share(row: &TtlRow) -> f64 {
    count_as_f64(row.untracked()) / count_as_f64(row.stats().recv()) * PERCENT
}

/// One rendered view of one folded run.
///
/// The frame holds what a reader needs and nothing that the reader must give
/// back: the header line names the run, the table folds it, and the map of
/// names says what each address is called. A caller builds one of these and
/// asks for the lines at the width of its terminal.
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
    ///
    /// The header line stands first, one blank line stands under it, and the
    /// table stands under that. The header line is no part of the table and it
    /// never gets cut: a long file name would lose the size that stands behind
    /// it, and that size is what tells a reader whether the run recorded
    /// anything at all. The buffer is therefore as wide as the wider of the two.
    ///
    /// A terminal too narrow for the whole table drops columns, in the order
    /// the module documentation states. The [`Layout`] holds that answer, and
    /// the header line, the widths, the headings, and the rows all read the one
    /// layout.
    ///
    /// Every line drops its trailing spaces. A terminal prints them as nothing.
    ///
    /// `paint` says whether the lines carry the color of a terminal. The one
    /// cell of the table that holds a color is the mark of a lost probe, which
    /// [`Paint::Colored`] stands between the two codes that paint it red.
    pub(crate) fn lines(&self, width: u16, paint: Paint) -> Vec<String> {
        let header_line = self.header.line();
        let layout = Layout::at(width);
        let table_width = layout.width();
        let rows = self.rows(layout.host());
        // A path holds 255 TTLs at most, and one TTL holds a bounded number of
        // rows, so the height of the frame stays far below the limit. The
        // arithmetic below says so anyway: a height that ran over would be a
        // panic in a debug build, over a number no run reaches.
        let height = u16::try_from(rows.len())
            .unwrap_or(u16::MAX)
            .saturating_add(HEADER_LINES + COLUMN_HEADER_LINES);
        let buffer_width = table_width.max(buffer_columns(&header_line));

        let mut buffer = Buffer::empty(Rect::new(0, 0, buffer_width, height));
        buffer.set_string(0, 0, &header_line, Style::default());
        let table = Table::new(rows.iter().map(|row| layout.row(row)), layout.constraints())
            .header(layout.row(&column_header()))
            .column_spacing(COLUMN_SPACING);
        Widget::render(
            &table,
            Rect::new(0, HEADER_LINES, table_width, height - HEADER_LINES),
            &mut buffer,
        );

        (0..height)
            .map(|line| read_line(&buffer, line, buffer_width, paint))
            .collect()
    }

    /// Every row of the table, in TTL order.
    ///
    /// One row for each TTL of the path, and one row for each participant of a
    /// TTL that more than one of them answered at.
    fn rows(&self, host: u16) -> Vec<RowText> {
        let mut rows = Vec::new();
        for row in self.table.rows() {
            rows.push(self.ttl_row(row, host));
            rows.extend(self.address_rows(row, host));
        }
        rows
    }

    /// The row of one TTL of the path.
    ///
    /// The host names the router that answered first, so a path that flaps
    /// keeps the name a reader already read. The count behind it is the
    /// participants of the TTL minus that one, which is also the address rows
    /// of the TTL minus that one, and the star says that one of them is the
    /// destination. A row that answered from one router alone carries no
    /// count, and a row that answered from no destination carries no star.
    ///
    /// A host too wide for the Host column loses the tail of its name, and it
    /// keeps both marks. The name therefore takes the columns that the marks
    /// leave. A cut of the whole text takes the marks first, because the two
    /// of them stand at the end of it, and a name with its address fills the
    /// column of a run that resolves names. A cut name still reads as a name.
    /// Nothing else on the screen carries either mark.
    ///
    /// A Host column narrower than the two marks together keeps as much of the
    /// marks as it holds, and it prints no name. The floor of the column is
    /// [`HOST_MIN`] columns and the two marks take far fewer, so no terminal
    /// reaches that corner.
    fn ttl_row(&self, row: &TtlRow, host: u16) -> RowText {
        let named = row
            .addresses()
            .next()
            .map_or_else(|| NO_HOST.to_owned(), |first| self.host_of(first.addr()));
        let others = participants(row).saturating_sub(1);
        let more = if others > 0 {
            format!(" (+{others})")
        } else {
            String::new()
        };
        let star = if self.holds_the_destination(row) {
            format!(" {DESTINATION_MARK}")
        } else {
            String::new()
        };
        // The name takes the columns that the two marks leave, and the marks
        // then stand whole behind it. The cut below holds the whole text
        // inside the column anyway, so a Host column narrower than the marks
        // keeps as much of them as it holds and prints no name at all.
        let marks = format!("{more}{star}");
        let for_the_name = usize::from(host).saturating_sub(display_width(&marks));
        let host_text = format!("{}{marks}", truncate_to_width(&named, for_the_name));
        RowText {
            ttl: row.ttl().to_string(),
            host: truncate_to_width(&host_text, usize::from(host)),
            percent: row
                .loss()
                .map_or_else(|| NO_NUMBER.to_owned(), render_percent),
            mark: LOSS_MARK,
            sent: row.sent().to_string(),
            times: time_texts(row.stats()),
            recent: sparkline(row.stats().recent(), usize::from(RECENT_WIDTH)),
        }
    }

    /// The rows of the routers that answered at one TTL.
    ///
    /// A TTL of one participant takes none of them: that one router is already
    /// the host of the row of the TTL, and a second line of the same numbers
    /// tells a reader nothing.
    ///
    /// The last row of the set closes it with a different glyph, so a reader
    /// finds where the routers of one TTL stop without counting the rows
    /// against the count in the host above them.
    fn address_rows(&self, row: &TtlRow, host: u16) -> Vec<RowText> {
        if participants(row) < 2 {
            return Vec::new();
        }
        let mut rows: Vec<RowText> = row.addresses().map(|held| self.address_row(held)).collect();
        if row.untracked() > 0 {
            rows.push(others_row(row));
        }
        let last = rows.len().saturating_sub(1);
        for (index, address_row) in rows.iter_mut().enumerate() {
            let branch = if index == last { LAST_BRANCH } else { BRANCH };
            address_row.host =
                truncate_to_width(&format!("{branch}{}", address_row.host), usize::from(host));
        }
        rows
    }

    /// The row of one router that answered at a TTL.
    ///
    /// The count is the answers of that one router, and not a count of probes:
    /// a probe reaches a TTL and not a router, so whichever router answers
    /// takes that answer.
    fn address_row(&self, address: Address<'_>) -> RowText {
        RowText {
            ttl: String::new(),
            host: self.host_of(address.addr()),
            percent: render_percent(address.share()),
            mark: SHARE_MARK,
            sent: address.stats().recv().to_string(),
            times: time_texts(address.stats()),
            recent: sparkline(address.stats().recent(), usize::from(RECENT_WIDTH)),
        }
    }

    /// The host of one address: the name that a `name` record gave it with the
    /// address beside it, or the address alone.
    ///
    /// The address stays beside the name because a name is what a resolver said
    /// and an address is what answered. A reader who chases a slow hop needs
    /// the number that reaches it.
    fn host_of(&self, addr: IpAddr) -> String {
        self.names
            .get(&addr)
            .map_or_else(|| addr.to_string(), |name| format!("{name} ({addr})"))
    }

    /// Whether one TTL answered from the destination.
    ///
    /// Every router the row tracks counts, and not the first one alone: a path
    /// that reaches its target through a load balancer answers the last TTL
    /// from the target second as often as it answers from it first.
    fn holds_the_destination(&self, row: &TtlRow) -> bool {
        let Some(destination) = self.destination else {
            return false;
        };
        row.addresses().any(|held| held.addr() == destination)
    }
}

/// The row that stands for the answers of a TTL that no tracked address holds.
///
/// Every time column of it holds one word, because the fold keeps no time for
/// such an answer: the answers came from routers the row has no entry for, and
/// an entry is where the times live. The share is what the row is for. Without
/// it, the printed shares of a crowded TTL sum to less than the whole and
/// nothing on the screen says where the rest went.
fn others_row(row: &TtlRow) -> RowText {
    RowText {
        ttl: String::new(),
        host: OTHERS.to_owned(),
        percent: render_percent(untracked_share(row)),
        mark: SHARE_MARK,
        sent: row.untracked().to_string(),
        times: TIME_HEADERS.map(|_| NO_NUMBER.to_owned()),
        recent: Recent::empty(),
    }
}

/// One row of the buffer, as the text that a terminal prints for it.
///
/// A wide glyph fills its first cell and hides the cells under the columns
/// behind it, and a hidden cell reports the one space that the empty buffer
/// left there. The walk therefore steps over as many cells as the symbol is
/// wide, or the line would grow one column for every wide glyph of it and a
/// Japanese host name would push the numbers of its own row to the right.
///
/// A `ratatui` buffer keeps the style of a cell beside the symbol of that cell,
/// so this walk is where a color of the table reaches the text. A run of red
/// cells opens with one code and closes with one, and a line that ends inside
/// such a run closes it at the end. No code ever stands where a cell holds
/// none, so a `Paint::Plain` line reads character for character as it always
/// did.
fn read_line(buffer: &Buffer, line: u16, width: u16, paint: Paint) -> String {
    let mut text = String::new();
    let mut column = 0;
    let mut red = false;
    while column < width {
        let held = buffer.cell((column, line));
        let symbol = held.map_or(" ", ratatui::buffer::Cell::symbol);
        let wanted = paint == Paint::Colored && held.is_some_and(|cell| cell.fg == Color::Red);
        if wanted != red {
            text.push_str(if wanted { RED } else { PLAIN });
            red = wanted;
        }
        text.push_str(symbol);
        // A symbol that prints nothing still holds its cell, so the walk moves
        // on by one and never stands still.
        column += buffer_columns(symbol).max(1);
    }
    if red {
        text.push_str(PLAIN);
    }
    // The trailing spaces go away, and no code goes with them: a red cell holds
    // the mark of a loss and never a space, so the code that closes a run of
    // them already stands in front of the first space of the line.
    text.trim_end_matches(' ').to_owned()
}

#[cfg(test)]
mod tests {
    use super::{
        display_width, frame_columns_of, render_duration, render_size, sparkline,
        truncate_to_width, Frame, Header, Paint,
    };
    use crate::record::RoundRecord;
    use crate::stats::{HopTable, Sample};
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

    /// The seven bars of the sparkline, lowest first.
    ///
    /// The test states them, and the module states them again. The two are on
    /// purpose: a test that read the constant of the module would agree with
    /// every set of glyphs the module ever holds, and the set of glyphs is the
    /// part of the sparkline a reader of the table sees.
    const BARS: &str = "▁▂▃▄▅▆▇";

    /// The glyphs that a set of round-trip times draws, at a width.
    ///
    /// The glyphs and not the marks: the set of glyphs is the part of the
    /// sparkline a reader of the table sees, and the color of a mark is what
    /// the tests of the frame below read.
    fn bar(samples: &[f64], width: usize) -> String {
        let times: Vec<Sample> = samples.iter().copied().map(Sample::Time).collect();
        sparkline(times.into_iter(), width).to_string()
    }

    #[test]
    fn a_rising_ramp_draws_every_bar_of_the_set() {
        // The samples run from 1 to 7, so the span is 6. The bar of a sample is
        // its distance from the smallest one, over the span, times the seven
        // bars: 0/6, 7/6, 14/6 ... which cut to 0, 1, 2, 3, 4, 5, and the
        // largest sample gives 7, which the clamp puts on the last bar.
        assert_eq!(
            bar(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0], 9),
            BARS,
            "seven samples one step apart draw each of the seven bars once"
        );
    }

    #[test]
    fn the_highest_bar_leaves_the_top_of_its_cell_empty() {
        // The rows of the table stand one under the other, so a bar that
        // painted the whole height of its cell would touch the bar of the row
        // above it. The line between the two rows would then go away, and a
        // reader would read one block of ink.
        assert_eq!(
            bar(&[1.0, 2.0], 9),
            "▁▇",
            "the largest sample of a window takes a bar that leaves the top eighth of its cell empty"
        );
    }

    #[test]
    fn the_smallest_sample_takes_the_lowest_bar_and_the_largest_takes_the_highest() {
        // The samples run from 10 to 40, so the span is 30. The bar of 20 is
        // (20 - 10) / 30 * 7 = 2.33, which cuts to the third bar. The bar of 30
        // is (30 - 10) / 30 * 7 = 4.67, which cuts to the fifth bar.
        let drawn = bar(&[10.0, 20.0, 30.0, 40.0], 9);
        assert_eq!(drawn, "▁▃▅▇", "the middle samples take the middle bars");
        assert_eq!(
            drawn.chars().next(),
            Some('▁'),
            "the smallest sample takes the lowest bar"
        );
        assert_eq!(
            drawn.chars().next_back(),
            Some('▇'),
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
        // (2 - 1) / 3 * 7 = 2.33 cuts to the third bar, and (3 - 1) / 3 * 7 =
        // 4.67 cuts to the fifth.
        let history = [100.0, 200.0, 1.0, 2.0, 3.0, 4.0];
        assert_eq!(
            bar(&history, 4),
            "▁▃▅▇",
            "the window holds the last four samples, and its scale reads only them"
        );

        // The same history at a width that holds all of it reads a scale from 1
        // to 200, and the four small samples then crowd on the lowest bar. The
        // two results differ, so the window drops the oldest samples and not
        // the most recent ones.
        assert_eq!(
            bar(&history, 6),
            "▄▇▁▁▁▁",
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
        // the scale. The ramp of seven therefore keeps each of its bars.
        assert_eq!(
            bar(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, f64::NAN], 9),
            "▁▂▃▄▅▆▇▁",
            "the sample that is not a number draws the lowest bar and moves no other bar"
        );
        assert_eq!(
            bar(&[1.0, f64::NAN, 8.0], 9),
            "▁▁▇",
            "the smallest and the largest sample keep their bars around a sample that is not a number"
        );
        assert_eq!(
            bar(&[1.0, 8.0], 9),
            "▁▇",
            "the same two samples without it draw the same two bars"
        );
        assert_eq!(
            bar(&[f64::NAN, f64::NAN], 9),
            "▁▁",
            "a window of samples that none of them compare draws a flat line"
        );
        assert_eq!(
            bar(&[1.0, f64::INFINITY, 8.0], 9),
            "▁▁▇",
            "an infinity does not compare either, and it takes the lowest bar"
        );
        assert_eq!(
            bar(&[1.0, f64::NEG_INFINITY, 8.0], 9),
            "▁▁▇",
            "an infinity below zero takes the lowest bar and holds the scale off the floor"
        );
    }

    #[test]
    fn the_mark_of_a_loss_is_no_bar_of_a_time() {
        // A run that prints no color must still show the loss. A headless run,
        // a pipe, a file, and a replay each print text with no color, so the
        // mark of a lost probe stands apart from every bar by its glyph and not
        // by its color alone. A mark that were one of the bars would read on
        // every one of those runs as a round-trip time that the hop never gave.
        let mut window: Vec<Sample> = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]
            .into_iter()
            .map(Sample::Time)
            .collect();
        window.push(Sample::Lost);
        let drawn = sparkline(window.into_iter(), 9).to_string();

        let glyphs: Vec<char> = drawn.chars().collect();
        let (bars, loss) = glyphs.split_at(glyphs.len().saturating_sub(1));
        assert_eq!(
            bars.iter().collect::<String>(),
            BARS,
            "the seven times of the window draw the seven bars: {drawn}"
        );
        let mark = loss
            .first()
            .copied()
            .expect("the window holds the lost probe");
        assert!(
            !BARS.contains(mark),
            "the mark {mark} of a lost probe is one of the bars {BARS}, so a run that prints no color reads that loss as a time"
        );
        assert_eq!(
            UnicodeWidthChar::width(mark),
            Some(1),
            "the mark {mark} of a lost probe takes one terminal column, as each bar does, so one sample of the history stands in one column of the Recent column"
        );
    }

    #[test]
    fn the_bar_holds_no_character_outside_the_set() {
        let drawn = bar(&[3.0, 1.0, 4.0, 1.0, 5.0, 9.0, 2.0, 6.0, f64::NAN], 9);
        assert_eq!(drawn.chars().count(), 9, "one bar stands for one sample");
        for character in drawn.chars() {
            assert!(
                BARS.contains(character),
                "the bar {drawn} holds {character}, which is not one of the seven block elements"
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

    #[test]
    fn a_run_that_measured_no_terminal_draws_the_nominal_frame() {
        assert_eq!(
            frame_columns_of(None),
            NOMINAL_WIDTH,
            "a run with no terminal to ask holds no width of a window, and a reader who redirects a replay asked for the whole table"
        );
    }

    #[test]
    fn a_terminal_that_reports_no_columns_draws_the_nominal_frame() {
        assert_eq!(
            frame_columns_of(Some(0)),
            NOMINAL_WIDTH,
            "a terminal that carries no window reports zero columns, and a frame of no columns drops every column that drops and says nothing at all"
        );
    }

    #[test]
    fn a_terminal_that_reports_a_width_draws_at_that_width() {
        assert_eq!(
            frame_columns_of(Some(WIDE_TERMINAL)),
            WIDE_TERMINAL,
            "a terminal that measured a window fills that window and no more"
        );
    }

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
    /// star of the destination, a sparkline, and the mark of a lost probe.
    const GOLDEN_FRAME: [&str; 10] = [
        " krt  example.com → 93.184.216.34   src 1.2.3.4   round 4   1s   1.2.3.4-example.com.jsonl (2.1 MB)",
        "",
        " TTL  Host                             Loss%   Sent   Last    Min    Avg    Max  StDev  Recent",
        "   1  router.lan (192.168.1.1)          0.0%      4    5.0    1.0    3.0    5.0    2.0  ▁▁▇▇",
        "   2  ???                             100.0%      4      -      -      -      -      -  ╳╳╳╳",
        "   3  10.0.0.1                         50.0%      4   12.0    8.0   10.0   12.0    2.0  ▁╳▇╳",
        "   4  ae1.net (203.0.113.8) (+1)        0.0%      4   70.0   10.0   40.0   70.0   22.4  ▁▅▃▇",
        "      ├ ae1.net (203.0.113.8)          50.0%▹     2   30.0   10.0   20.0   30.0   10.0  ▁▇",
        "      └ 203.0.113.9                    50.0%▹     2   70.0   50.0   60.0   70.0   10.0  ▁▇",
        "   5  example.com (93.184.216.34) ★     0.0%      4   60.0   40.0   50.0   60.0   10.0  ▁▁▇▇",
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
    /// statistic of it holds no value. Its window holds four lost probes, so
    /// its sparkline draws four marks of a loss and no bar.
    ///
    /// TTL 3 answers the first and the third round, from `10.0.0.1`, at 8.0 and
    /// 12.0, and it loses the second and the fourth. The loss is
    /// 2 / 4 = 50.0 percent. The mean is 20 / 2 = 10.0, the distances are -2
    /// and 2, whose squares sum to 8, so the variance is 8 / 2 = 4.0 and the
    /// deviation is 2.0. Its window holds the two answers and the two lost
    /// probes in the order the rounds arrived.
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
        .lines(width, Paint::Plain)
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
            } else if inside {
                if let Some(field) = fields.last_mut() {
                    field.1.push(character);
                }
            } else {
                fields.push((column, character.to_string()));
                inside = true;
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

    /// The Recent column of one row, which is the last field of its line.
    ///
    /// Every row that draws a sparkline ends its line with that sparkline, and
    /// a sparkline holds no space, so the last field of the line is the whole
    /// of the column.
    fn recent_of(line: &str) -> String {
        fields_with_columns(line)
            .pop()
            .map_or_else(String::new, |(_, text)| text)
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

    /// A name of the destination that fills more than the Host column.
    ///
    /// The name takes 23 columns, and the address of the golden target adds
    /// ` (93.184.216.34)`, which takes 16 more. The two of them therefore take
    /// 23 + 16 = 39 of the 30 columns that the Host column holds, and the star
    /// wants 2 more.
    const LONG_DESTINATION_NAME: &str = "edge-01.lax.example.com";

    /// A name of a router that fills more than the Host column.
    ///
    /// The name takes 25 columns, and the address of the left router adds
    /// ` (203.0.113.8)`, which takes 14 more. The two of them therefore take
    /// 25 + 14 = 39 of the 30 columns, and the count wants 5 more.
    const LONG_ROUTER_NAME: &str = "ae-1.core.lax.example.net";

    /// The tail of the row of a TTL that folded two rounds, and that two
    /// routers answered at.
    ///
    /// The TTL answered both rounds, so the loss is 0 / 2 = 0.0 percent. The
    /// samples are 10.0 and 30.0, so the last is 30.0, the smallest is 10.0,
    /// the mean is 40 / 2 = 20.0, and the largest is 30.0. Each sample stands
    /// 10.0 from the mean, so the deviation is 10.0. The window of the
    /// sparkline runs from 10.0 to 30.0, so the first sample takes the lowest
    /// bar and the second takes the highest.
    const TWO_ROUND_TAIL: &str = "  0.0%      2   30.0   10.0   20.0   30.0   10.0  ▁▇";

    #[test]
    fn a_cut_host_keeps_the_star_of_the_destination() {
        // One round probes one TTL, and the destination answers it at 1.0. The
        // host wants 39 columns for the name and the address, and 2 more for
        // the star, of the 30 that the Host column holds.
        //
        // The star stands behind the cut, so the name gives up the columns
        // that the star takes: 30 - 2 = 28 columns of the name and the address
        // stay, which is the 23 of the name and the ` (93.` behind it.
        let table = table_of(&[round(1, 1, &[(1, TARGET, 1.0)])]);
        let names = names_of(&[(TARGET, LONG_DESTINATION_NAME)]);
        let lines = lines_of(&table, &names, Some(address(TARGET)), NOMINAL_WIDTH);
        let row = line(&lines, COLUMN_HEADER_LINE + 1);
        let cut = "edge-01.lax.example.com (93. ★";
        assert_eq!(
            display_width(cut),
            HOST_WIDTH,
            "the cut host of the destination fills the Host column and no more"
        );
        assert_eq!(
            row,
            format!("   1  {cut}{COLUMN_SPACES}{ONE_ROUND_TAIL}"),
            "the star of the destination stands behind the cut name"
        );
        assert_eq!(
            display_width(&host_column(row, HOST_WIDTH)),
            HOST_WIDTH,
            "the printed cell of the host fills the Host column and no more"
        );
    }

    #[test]
    fn a_cut_host_keeps_the_count_of_the_routers_behind_it() {
        // Two rounds probe one TTL, and a different router answers each of
        // them, so the row names the first of the two and counts the other
        // one. The host wants 39 columns for the name and the address, and 5
        // more for the count, of the 30 that the Host column holds.
        //
        // The count stands behind the cut, so the name gives up the columns
        // that the count takes: 30 - 5 = 25 columns of the name and the
        // address stay, which is the name alone.
        let table = table_of(&[
            round(1, 1, &[(1, LEFT_ROUTER, 10.0)]),
            round(1, 1, &[(1, RIGHT_ROUTER, 30.0)]),
        ]);
        let names = names_of(&[(LEFT_ROUTER, LONG_ROUTER_NAME)]);
        let lines = lines_of(&table, &names, None, NOMINAL_WIDTH);
        let row = line(&lines, COLUMN_HEADER_LINE + 1);
        let cut = "ae-1.core.lax.example.net (+1)";
        assert_eq!(
            display_width(cut),
            HOST_WIDTH,
            "the cut host of the TTL fills the Host column and no more"
        );
        assert_eq!(
            row,
            format!("   1  {cut}{COLUMN_SPACES}{TWO_ROUND_TAIL}"),
            "the count of the other routers stands behind the cut name"
        );
        assert_eq!(
            display_width(&host_column(row, HOST_WIDTH)),
            HOST_WIDTH,
            "the printed cell of the host fills the Host column and no more"
        );
    }

    #[test]
    fn a_lost_probe_draws_a_mark_of_its_own_and_no_address_row_draws_it() {
        // Four rounds probe TTL 1. The left router answers the first round and
        // the fourth, the right router answers the second, and the third round
        // gets nothing back. The Recent column of the TTL therefore draws four
        // marks, and the third of them is the loss. A column of three bars
        // would read as the column of a TTL that lost nothing, and the Loss%
        // beside it would contradict the one picture of the recent behavior of
        // that TTL.
        //
        // The times of the window are 10, 30, and 20, so its scale runs from 10
        // to 30: 10 takes the lowest bar, 30 takes the highest, and 20 stands
        // at half of the span, which is the fourth bar of the seven. The loss
        // takes no part in the scale, because a lost probe measures no time.
        //
        // Neither address row draws a loss. The round that the right router
        // answered is no loss of the left one, and the round that no router
        // answered is a loss of the TTL and of neither of them. A mark on an
        // address row would report a loss that the Share% beside it
        // contradicts.
        let table = table_of(&[
            round(1, 1, &[(1, LEFT_ROUTER, 10.0)]),
            round(1, 1, &[(1, RIGHT_ROUTER, 30.0)]),
            round(1, 1, &[]),
            round(1, 1, &[(1, LEFT_ROUTER, 20.0)]),
        ]);
        let lines = lines_of(&table, &BTreeMap::new(), None, NOMINAL_WIDTH);
        assert_eq!(
            recent_of(line(&lines, 3)),
            "▁▇╳▄",
            "the ttl draws one mark for each probe, and the lost probe takes a mark of its own"
        );
        assert_eq!(
            recent_of(line(&lines, 4)),
            "▁▇",
            "the left router draws its two answers and no loss"
        );
        assert_eq!(
            recent_of(line(&lines, 5)),
            "▁",
            "the right router draws its one answer and no loss"
        );
    }

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

    /// A terminal far too narrow for the last set of columns.
    const NARROW_TERMINAL: u16 = 20;

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
        // The Host column takes what the rest of the frame leaves it, and a
        // terminal that leaves it less than the floor gets the floor. No width
        // cuts it below that: a host of three characters names no router, and
        // the columns of the table go away instead.
        for width in 0..=NOMINAL_WIDTH {
            assert!(
                host_columns(width) >= HOST_MIN,
                "a terminal of {width} columns cut the Host column to {} columns",
                host_columns(width)
            );
        }
        assert_eq!(
            host_columns(NARROW_TERMINAL),
            HOST_MIN,
            "a terminal too narrow for the last set of columns gets the frame at the floor"
        );
        let narrow = golden_lines(NARROW_TERMINAL);
        assert!(
            display_width(line(&narrow, COLUMN_HEADER_LINE)) > usize::from(NARROW_TERMINAL),
            "the frame at the floor is wider than the terminal, and the terminal clips it"
        );
    }

    /// The least terminal width that holds each set of columns, and the
    /// headings that stand there.
    ///
    /// The sets run from the widest to the narrowest one, and the width of each
    /// row is the least width that holds its set: one column less drops the
    /// next column of the order, which is the set of the row below.
    ///
    /// The test spells the widths, and the module derives them from the widths
    /// of its columns. The two are on purpose, as they are for the nominal
    /// width above: a test that read the constants of the module would agree
    /// with every set of widths the module ever holds. The frame without the
    /// Host column takes 67, 56, 49, 42, 35, 28, 22, and 13 columns, and the
    /// floor of the Host column adds 12 to each of them.
    const COLUMN_SETS: [(u16, &[&str]); 8] = [
        (
            79,
            &[
                "TTL", "Host", "Loss%", "Sent", "Last", "Min", "Avg", "Max", "StDev", "Recent",
            ],
        ),
        (
            68,
            &[
                "TTL", "Host", "Loss%", "Sent", "Last", "Min", "Avg", "Max", "StDev",
            ],
        ),
        (
            61,
            &["TTL", "Host", "Loss%", "Sent", "Last", "Min", "Avg", "Max"],
        ),
        (54, &["TTL", "Host", "Loss%", "Sent", "Last", "Min", "Avg"]),
        (47, &["TTL", "Host", "Loss%", "Sent", "Last", "Avg"]),
        (40, &["TTL", "Host", "Loss%", "Sent", "Avg"]),
        (34, &["TTL", "Host", "Loss%", "Avg"]),
        (25, &["TTL", "Host", "Avg"]),
    ];

    /// A terminal one column too narrow for the last set of columns.
    const BELOW_THE_LAST_SET: u16 = 24;

    /// The narrowest terminal that a test below reads.
    const ONE_COLUMN: u16 = 1;

    /// A terminal that holds every column but the sparkline and the deviation.
    const TWO_DROPPED: u16 = 67;

    /// A terminal that holds every column but the sparkline, the deviation, and
    /// the largest time.
    const THREE_DROPPED: u16 = 60;

    /// A host name longer than the Host column of every width the tests below
    /// read, and one that holds no space.
    ///
    /// The cut of such a name fills the column exactly, so the field that
    /// starts in the first column of the Host column is as wide as the column
    /// is. A name that held a space would read as two fields, and a name
    /// shorter than the column would measure itself and not the column.
    const LONG_NAME: &str = "core1.router.example.net.example.org";

    /// The heading of every column that a column header holds.
    fn headings_of(line: &str) -> Vec<String> {
        fields_with_columns(line)
            .into_iter()
            .map(|(_, text)| text)
            .collect()
    }

    /// The heading of every column of the golden frame at a terminal width.
    fn headings(width: u16) -> Vec<String> {
        headings_of(line(&golden_lines(width), COLUMN_HEADER_LINE))
    }

    /// The number of columns that the Host column takes at a terminal width.
    ///
    /// The one row of the frame names a host longer than the column, so the cut
    /// host fills the column and the field measures the column itself.
    fn host_columns(width: u16) -> usize {
        let table = table_of(&[round(1, 1, &[(1, FIRST_HOP, 1.0)])]);
        let names = names_of(&[(FIRST_HOP, LONG_NAME)]);
        let lines = lines_of(&table, &names, None, width);
        let row = line(&lines, COLUMN_HEADER_LINE + 1);
        let host = fields_with_columns(row)
            .into_iter()
            .find(|(column, _)| *column == HOST_START)
            .map_or(0, |(_, text)| display_width(&text));
        assert!(
            host < display_width(LONG_NAME),
            "the cut host must stop inside the name, or the field measures the name: {row}"
        );
        host
    }

    /// The terminal column that each field behind the percentage of a line ends
    /// in.
    ///
    /// Every column behind the percentage holds its text to the right, so two
    /// lines whose columns stand in the same place end their fields in the same
    /// columns however long the numbers of either line are.
    fn ends_behind_the_percent(line: &str) -> Vec<usize> {
        let end = percent_end(line);
        fields_with_columns(line)
            .into_iter()
            .filter(|(column, _)| *column >= end)
            .map(|(column, text)| column + display_width(&text))
            .collect()
    }

    /// The value that stands under one heading of the column header.
    ///
    /// The lookup reads the columns of the two lines, and not the order of
    /// their fields. A row that read its cells from a second list would shift
    /// the cells of every column behind a dropped one, and a lookup by order
    /// would read the shifted row as the right one. A column of numbers holds
    /// its text to the right, so its heading and its cell end in the same
    /// column, and the Host column holds its text to the left, so the two start
    /// in the same column.
    fn value_under(header: &str, row: &str, heading: &str) -> Option<String> {
        let (start, width) = fields_with_columns(header)
            .into_iter()
            .find(|(_, text)| text == heading)
            .map(|(column, text)| (column, display_width(&text)))?;
        let end = start + width;
        fields_with_columns(row)
            .into_iter()
            .find(|(column, text)| *column == start || column + display_width(text) == end)
            .map(|(_, text)| text)
    }

    #[test]
    fn the_columns_drop_in_the_order_of_the_module_documentation() {
        for (index, &(width, standing)) in COLUMN_SETS.iter().enumerate() {
            assert_eq!(
                headings(width),
                standing,
                "a terminal of {width} columns holds every column of its set"
            );
            // One column less drops the next column of the order. The last set
            // drops nothing: the TTL and the host name the hop, and the average
            // says how slow it is.
            let narrower = COLUMN_SETS
                .get(index + 1)
                .map_or(standing, |&(_, next)| next);
            assert_eq!(
                headings(width - 1),
                narrower,
                "a terminal of {} columns drops the next column of the order",
                width - 1
            );
        }
    }

    #[test]
    fn no_column_drops_while_the_terminal_holds_the_frame() {
        let (least, whole) = COLUMN_SETS[0];
        for width in [least, NOMINAL_WIDTH, WIDE_TERMINAL] {
            assert_eq!(
                headings(width),
                whole,
                "a terminal of {width} columns holds every column of the table"
            );
        }
        assert_eq!(
            host_columns(least),
            HOST_MIN,
            "the least width that holds every column stands the Host column on its floor"
        );
    }

    #[test]
    fn the_host_column_takes_the_columns_that_a_dropped_column_released() {
        // The last set drops no column, so the width below it releases nothing.
        for &(width, _) in COLUMN_SETS.iter().take(COLUMN_SETS.len() - 1) {
            let floor = host_columns(width);
            let after_the_drop = host_columns(width - 1);
            assert_eq!(
                floor, HOST_MIN,
                "the least width of a set stands the Host column on its floor"
            );
            assert!(
                after_the_drop > floor,
                "a terminal of {} columns dropped a column, so the Host column takes what it released: {after_the_drop} columns against {floor}",
                width - 1
            );
        }
    }

    #[test]
    fn a_terminal_below_the_last_set_renders_at_the_floor() {
        let (least, last_set) = COLUMN_SETS[COLUMN_SETS.len() - 1];
        for width in [BELOW_THE_LAST_SET, ONE_COLUMN] {
            assert_eq!(
                headings(width),
                last_set,
                "a terminal of {width} columns keeps the TTL, the host, and the average"
            );
            assert_eq!(
                host_columns(width),
                HOST_MIN,
                "a terminal of {width} columns stands the Host column on its floor"
            );
        }
        let lines = golden_lines(BELOW_THE_LAST_SET);
        let header = line(&lines, COLUMN_HEADER_LINE);
        assert_eq!(
            display_width(header),
            usize::from(least),
            "the frame at the floor takes the least width of the last set"
        );
        assert!(
            display_width(header) > usize::from(BELOW_THE_LAST_SET),
            "the terminal clips the frame, and a frame of no columns says nothing"
        );
    }

    #[test]
    fn the_columns_that_stand_line_up_under_the_column_header() {
        let lines = golden_lines(THREE_DROPPED);
        let header = line(&lines, COLUMN_HEADER_LINE);
        let ttl_row = line(&lines, SHARED_TTL_LINE);
        assert_eq!(
            headings_of(header),
            ["TTL", "Host", "Loss%", "Sent", "Last", "Min", "Avg"],
            "a terminal of {THREE_DROPPED} columns drops the sparkline, the deviation, and the largest time"
        );
        for other in [
            header,
            line(&lines, SHARED_TTL_LINE + 1),
            line(&lines, SHARED_TTL_LINE + 2),
        ] {
            assert_eq!(
                percent_end(other),
                percent_end(ttl_row),
                "the percentage of {other} must end where the percentage of {ttl_row} ends"
            );
            assert_eq!(
                ends_behind_the_percent(other),
                ends_behind_the_percent(ttl_row),
                "the columns behind the percentage of {other} must stand under the columns of {ttl_row}"
            );
        }
    }

    #[test]
    fn a_dropped_column_takes_its_cells_with_it() {
        // Three rounds probe TTL 1, and the router answers each of them, at
        // 10.0, 40.0, and 20.0. The loss is 0 / 3 = 0.0 percent. The last
        // answer is 20.0, the smallest is 10.0, and the largest is 40.0. The
        // mean is 70 / 3 = 23.33, which one decimal place writes as 23.3. The
        // distances from the mean are -13.33, 16.67, and -3.33, whose squares
        // sum to 466.67, so the variance is 466.67 / 3 = 155.56 and the
        // deviation is 12.47, which prints as 12.5. No two of those numbers are
        // the same, so a cell that landed in the column beside its own fails
        // the test.
        let table = table_of(&[
            round(1, 1, &[(1, BARE_HOP, 10.0)]),
            round(1, 1, &[(1, BARE_HOP, 40.0)]),
            round(1, 1, &[(1, BARE_HOP, 20.0)]),
        ]);
        let names = BTreeMap::new();
        let lines = lines_of(&table, &names, None, TWO_DROPPED);
        let header = line(&lines, COLUMN_HEADER_LINE);
        let row = line(&lines, COLUMN_HEADER_LINE + 1);
        assert_eq!(
            headings_of(header),
            ["TTL", "Host", "Loss%", "Sent", "Last", "Min", "Avg", "Max"],
            "a terminal of {TWO_DROPPED} columns drops the sparkline and the deviation"
        );
        for (heading, value) in [
            ("TTL", "1"),
            ("Host", BARE_HOP),
            ("Loss%", "0.0%"),
            ("Sent", "3"),
            ("Last", "20.0"),
            ("Min", "10.0"),
            ("Avg", "23.3"),
            ("Max", "40.0"),
        ] {
            assert_eq!(
                value_under(header, row, heading).as_deref(),
                Some(value),
                "the cell under {heading} holds the number that {heading} names: {row}"
            );
        }
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
