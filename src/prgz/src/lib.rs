//! Core logic for `prgz` (Progress Gzip): gzip-compress one file and report
//! what the run cost.
//!
//! The binary owns the progress bar and the command line. This library owns the
//! two parts that a test can drive without a terminal: the compression itself,
//! and the closing report.

#![cfg_attr(not(test), warn(clippy::unwrap_used))]
#![cfg_attr(not(test), warn(clippy::expect_used))]

use std::fs;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use colored::Colorize;
use flate2::write::GzEncoder;
use flate2::Compression;
use num_format::ToFormattedString;
use thiserror::Error;

pub use num_format::Locale;

/// The characters that end the locale name inside a POSIX locale value. A
/// full locale value has the shape `language_REGION.codeset@modifier`.
const CODESET_MARKS: [char; 2] = ['.', '@'];

/// The characters that separate the language from the region.
const REGION_MARKS: [char; 2] = ['_', '-'];

/// The count of fraction digits that a formatted number shows. The Go tool
/// shows the same count through `%.2f`.
const FRACTION_DIGITS: usize = 2;

/// The count of seconds in one second. A duration of this length or more reads
/// in seconds.
const SECONDS_PER_SECOND: f64 = 1.0;

/// The count of seconds in one millisecond. A shorter duration of this length
/// or more reads in milliseconds.
const SECONDS_PER_MILLISECOND: f64 = 0.001;

/// The count of seconds in one microsecond. A still shorter duration reads in
/// microseconds.
const SECONDS_PER_MICROSECOND: f64 = 0.000_001;

/// The suffix that marks a duration in seconds.
const SECOND_SUFFIX: &str = "s";

/// The suffix that marks a duration in milliseconds.
const MILLISECOND_SUFFIX: &str = "ms";

/// The suffix that marks a duration in microseconds.
const MICROSECOND_SUFFIX: &str = "\u{b5}s";

/// Resolve the number formatting locale from a POSIX locale value, such as
/// the value of `LANG`, `LC_ALL`, or `LC_NUMERIC`.
///
/// The function first removes the codeset and the modifier from the value, so
/// `de_DE.UTF-8@euro` becomes `de_DE`. It then looks for the remainder in the
/// locale table. If the table does not hold the remainder, the function looks
/// for the language part alone, so `de_DE` becomes `de`. If the table holds
/// neither name, the function answers `Locale::en`. The values `C`, `POSIX`,
/// and an empty value name no locale, thus they all answer `Locale::en`.
pub fn locale_from_lang(lang: &str) -> Locale {
    let name = lang
        .split_once(CODESET_MARKS)
        .map_or(lang, |(head, _)| head);
    if let Ok(locale) = Locale::from_name(name) {
        return locale;
    }
    let language = name.split_once(REGION_MARKS).map_or(name, |(head, _)| head);
    Locale::from_name(language).unwrap_or(Locale::en)
}

/// Format an integer with the thousands separator of the locale.
///
/// The locale also sets the size of each group of digits. Most locales put the
/// digits in groups of three. Some locales, such as the locales of India, use
/// other group sizes.
pub fn format_int(value: u64, locale: &Locale) -> String {
    value.to_formatted_string(locale)
}

/// Format a number with two fraction digits, the thousands separator of the
/// locale, and the decimal separator of the locale.
///
/// The two fraction digits match the `%.2f` of the Go tool, and the rounding
/// matches it as well.
///
/// A value that is not finite has no digits to group. Such a value gets the
/// word of the locale instead: `nan` for a value that is not a number, and
/// `infinity` after the sign for an infinite value.
///
/// A finite value with an integer part of more than 39 digits is too large to
/// group. Such a value keeps its two fraction digits, but shows the digits of
/// the integer part without a separator.
pub fn format_float(value: f64, locale: &Locale) -> String {
    if !value.is_finite() {
        return format_not_finite(value, locale);
    }
    let sign = sign_of(value, locale);
    let digits = format!("{:.*}", FRACTION_DIGITS, value.abs());
    let (integer, fraction) = digits.split_once('.').unwrap_or((digits.as_str(), ""));
    let grouped = match integer.parse::<u128>() {
        Ok(number) => number.to_formatted_string(locale),
        Err(_) => integer.to_string(),
    };
    let decimal = locale.decimal();
    format!("{sign}{grouped}{decimal}{fraction}")
}

/// Give the word of the locale for a value that is not finite.
fn format_not_finite(value: f64, locale: &Locale) -> String {
    if value.is_nan() {
        return locale.nan().to_string();
    }
    let sign = sign_of(value, locale);
    let infinity = locale.infinity();
    format!("{sign}{infinity}")
}

/// Give the minus sign of the locale for a negative value, and nothing for a
/// value that is not negative.
fn sign_of(value: f64, locale: &Locale) -> &'static str {
    if value.is_sign_negative() {
        locale.minus_sign()
    } else {
        ""
    }
}

/// Format a duration the way the closing report shows it.
///
/// A duration of one second or more gets seconds and the suffix `s`. A shorter
/// duration of one millisecond or more gets milliseconds and the suffix `ms`.
/// A still shorter duration gets microseconds and the suffix `µs`. Each of the
/// three goes through [`format_float`], thus each one shows two fraction
/// digits in the separators of the locale.
pub fn format_duration(duration: Duration, locale: &Locale) -> String {
    let seconds = duration.as_secs_f64();
    let (value, suffix) = if seconds >= SECONDS_PER_SECOND {
        (seconds, SECOND_SUFFIX)
    } else if seconds >= SECONDS_PER_MILLISECOND {
        (seconds / SECONDS_PER_MILLISECOND, MILLISECOND_SUFFIX)
    } else {
        (seconds / SECONDS_PER_MICROSECOND, MICROSECOND_SUFFIX)
    };
    let number = format_float(value, locale);
    format!("{number}{suffix}")
}

/// Why a compression run stopped short.
#[derive(Debug, Error)]
pub enum CompressError {
    /// The run could not open the input file.
    #[error("could not open the input file {}", path.display())]
    OpenInput {
        /// The path of the input file.
        path: PathBuf,
        /// The error that the file system gave.
        #[source]
        source: io::Error,
    },
    /// The run could not read the bytes of the input.
    #[error("could not read the input file")]
    ReadInput {
        /// The error that the file system gave.
        #[source]
        source: io::Error,
    },
    /// The run could not make the output file.
    #[error("could not create the output file {}", path.display())]
    CreateOutput {
        /// The path of the output file.
        path: PathBuf,
        /// The error that the file system gave.
        #[source]
        source: io::Error,
    },
    /// The run could not write the bytes of the gzip stream.
    #[error("could not write the output file")]
    WriteOutput {
        /// The error that the file system gave.
        #[source]
        source: io::Error,
    },
    /// The user stopped the run before it was complete.
    #[error("the user stopped the run")]
    Cancelled,
    /// The output file is the input file, thus a run would destroy the input.
    #[error("the output file {} is the input file", path.display())]
    SameFile {
        /// The path of the output file.
        path: PathBuf,
    },
}

/// The count of bytes in the buffer that holds one block of the input. A
/// larger buffer makes fewer read calls, and a smaller buffer makes the
/// progress report more frequent. 64 kibibytes is a common size for a read of
/// a disk, thus this size keeps the count of read calls low and it still
/// reports the progress many times for a file of a few megabytes.
const BUFFER_SIZE: usize = 64 * 1024;

/// The message of the error that a reader gets when it answers a count of
/// bytes that is larger than the buffer. Such a reader breaks the contract of
/// [`Read::read`].
const TOO_MANY_BYTES: &str = "the reader answered more bytes than the buffer holds";

/// Compress the bytes of `reader` into `writer` as a gzip stream.
///
/// The function answers the count of the uncompressed bytes that it read. It
/// reads one buffer at a time. After each buffer it gives the running count of
/// the read bytes to `on_progress`. Before each buffer it asks `cancelled`
/// whether the user stopped the run.
///
/// The function finishes the gzip stream before it answers, thus the writer
/// holds every byte of the stream when the function is complete. The function
/// owns no file, thus a caller that made a file must remove that file when
/// this function fails.
///
/// # Errors
///
/// Answers [`CompressError::ReadInput`] when a read of the input fails, and
/// [`CompressError::WriteOutput`] when a write of the gzip stream fails. A
/// write that only fails at the end of the stream gives the same error.
/// Answers [`CompressError::Cancelled`] when `cancelled` answers true.
pub fn compress_stream<R: Read, W: Write>(
    mut reader: R,
    writer: W,
    cancelled: &dyn Fn() -> bool,
    on_progress: &mut dyn FnMut(u64),
) -> Result<u64, CompressError> {
    let mut encoder = GzEncoder::new(writer, Compression::default());
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    let mut read_bytes = 0_u64;
    loop {
        if cancelled() {
            return Err(CompressError::Cancelled);
        }
        let count = reader
            .read(&mut buffer)
            .map_err(|source| CompressError::ReadInput { source })?;
        if count == 0 {
            break;
        }
        let block = buffer
            .get(..count)
            .ok_or_else(|| CompressError::ReadInput {
                source: io::Error::other(TOO_MANY_BYTES),
            })?;
        encoder
            .write_all(block)
            .map_err(|source| CompressError::WriteOutput { source })?;
        read_bytes += count as u64;
        on_progress(read_bytes);
    }
    encoder
        .finish()
        .map_err(|source| CompressError::WriteOutput { source })?;
    Ok(read_bytes)
}

/// A writer that counts the bytes that it gives to another writer.
///
/// The count of the compressed bytes must come from the stream itself. A read
/// of the size of the output file can happen before the last bytes of the
/// stream reach that file. The Go tool that this crate replaces had that
/// fault, thus it reported a size that was too small.
struct CountingWriter<'a, W: Write> {
    /// The writer that gets the bytes.
    inner: W,
    /// The count of the bytes that went to the writer.
    count: &'a mut u64,
}

impl<W: Write> Write for CountingWriter<'_, W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buffer)?;
        *self.count += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Answers whether `output` names the file that the open handle `source`
/// already holds.
///
/// On unix the function compares the device and the inode of the open handle
/// against a stat of `output`. It reads the handle rather than the input
/// path, thus it also catches a hard link to the input under a different
/// name, which a comparison of the two paths would miss. On every other
/// platform the function canonicalizes `input` and `output` and compares the
/// results, because this crate has no portable way to read the identity of
/// an open handle. On both platforms an `output` that does not yet exist
/// cannot be the input, thus the function then answers false: `fs::metadata`
/// and [`fs::canonicalize`] both fail on a path with nothing there, and
/// either failure answers false.
#[cfg(unix)]
fn output_is_input(source: &File, _input: &Path, output: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    let Ok(source_metadata) = source.metadata() else {
        return false;
    };
    let Ok(output_metadata) = fs::metadata(output) else {
        return false;
    };
    source_metadata.dev() == output_metadata.dev() && source_metadata.ino() == output_metadata.ino()
}

/// Answers whether `output` names the file at `input`. See the unix version
/// of this function for the full rustdoc; the two share one call site.
#[cfg(not(unix))]
fn output_is_input(_source: &File, input: &Path, output: &Path) -> bool {
    let Ok(input_path) = fs::canonicalize(input) else {
        return false;
    };
    let Ok(output_path) = fs::canonicalize(output) else {
        return false;
    };
    input_path == output_path
}

/// Compress the file at `input` into a gzip file at `output`.
///
/// The function makes the output file and writes the gzip stream into it. It
/// then answers the sizes of the two files and the time that the run took. The
/// size of the output is the count of the bytes of the stream, thus it holds
/// the last bytes of the stream as well.
///
/// The function refuses to run when `output` names the file at `input`,
/// checked before the function makes the output file. [`File::create`]
/// truncates its target, thus a run that skipped this check would truncate
/// the input file while the open input handle still read it, and it would
/// write a small gzip stream of nothing over the bytes that the user meant
/// to keep.
///
/// The function removes a part of an output file when the run fails, and also
/// when the user stops the run. A part of a gzip stream looks like a complete
/// one, thus a run that leaves such a file gives a broken file to the user.
/// The refusal above returns before the function makes any output file, thus
/// no removal follows it and the input file stays whole.
///
/// # Errors
///
/// Answers [`CompressError::OpenInput`] when the input file does not open.
/// Answers [`CompressError::SameFile`] when the output path names the input
/// file. Answers [`CompressError::CreateOutput`] when the output file does
/// not open. Answers [`CompressError::ReadInput`], [`CompressError::WriteOutput`],
/// or [`CompressError::Cancelled`] when the stream stops short.
pub fn compress_file(
    input: &Path,
    output: &Path,
    cancelled: &dyn Fn() -> bool,
    on_progress: &mut dyn FnMut(u64),
) -> Result<Stats, CompressError> {
    let source = File::open(input).map_err(|source| CompressError::OpenInput {
        path: input.to_path_buf(),
        source,
    })?;
    if output_is_input(&source, input, output) {
        return Err(CompressError::SameFile {
            path: output.to_path_buf(),
        });
    }
    let target = File::create(output).map_err(|source| CompressError::CreateOutput {
        path: output.to_path_buf(),
        source,
    })?;
    let start = Instant::now();
    let mut new_size = 0_u64;
    let counter = CountingWriter {
        inner: target,
        count: &mut new_size,
    };
    let result = compress_stream(source, counter, cancelled, on_progress);
    let duration = start.elapsed();
    match result {
        Ok(original_size) => Ok(Stats {
            original_size,
            new_size,
            duration,
        }),
        Err(error) => {
            // A failure to remove the file does not change the error that the
            // caller gets. The caller must know why the run stopped.
            let _ = fs::remove_file(output);
            Err(error)
        }
    }
}

/// The suffix that marks a gzip file. A run that gets no output name adds
/// this suffix to the input name.
const GZIP_SUFFIX: &str = ".gz";

/// The count of percent in the whole. A fraction becomes a percentage when it
/// is multiplied by this number.
const PERCENT_SCALE: f64 = 100.0;

/// The rate that a run of no length gets. A rate needs a length of time, thus
/// a run that took no time has no measured rate.
const NO_RATE: f64 = 0.0;

/// What one compression run produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    /// The count of bytes that the run read from the input.
    pub original_size: u64,
    /// The count of bytes that the run wrote to the output.
    pub new_size: u64,
    /// The time that the run took.
    pub duration: Duration,
}

impl Stats {
    /// Answers whether the output is not smaller than the input.
    ///
    /// An output of the same size as the input counts as larger, because the
    /// run gave no reduction. An empty input always gives a larger output,
    /// because a gzip stream of no bytes still carries a header.
    #[must_use]
    pub fn grew(&self) -> bool {
        self.new_size >= self.original_size
    }

    /// The change in size, as a percentage of the original size.
    ///
    /// The value is positive when the file became smaller. It is negative when
    /// the file became larger. The function answers `None` when the original
    /// size is zero, because no percentage of zero exists.
    #[must_use]
    pub fn size_change_percent(&self) -> Option<f64> {
        if self.original_size == 0 {
            return None;
        }
        let original = self.original_size as f64;
        let new = self.new_size as f64;
        Some((1.0 - new / original) * PERCENT_SCALE)
    }

    /// The count of input bytes that the run read in one second.
    ///
    /// A run of no length answers a rate of zero, because a rate needs a
    /// length of time.
    #[must_use]
    pub fn bytes_read_per_second(&self) -> f64 {
        rate(self.original_size, self.duration)
    }

    /// The count of output bytes that the run wrote in one second.
    ///
    /// A run of no length answers a rate of zero, because a rate needs a
    /// length of time.
    #[must_use]
    pub fn bytes_written_per_second(&self) -> f64 {
        rate(self.new_size, self.duration)
    }
}

/// Divides a count of bytes by the seconds of a duration.
fn rate(bytes: u64, duration: Duration) -> f64 {
    let seconds = duration.as_secs_f64();
    if seconds <= 0.0 {
        return NO_RATE;
    }
    bytes as f64 / seconds
}

/// The output path a run takes when the user names none: `<input>.gz`.
///
/// The function adds the suffix to the whole name, thus `notes.tar` gives
/// `notes.tar.gz`. It works on the bytes of the name, thus a name that is not
/// valid UTF-8 keeps every byte.
#[must_use]
pub fn default_output_path(input: &Path) -> PathBuf {
    let mut name = input.as_os_str().to_os_string();
    name.push(GZIP_SUFFIX);
    PathBuf::from(name)
}

/// The first line of a report of a run that made the file smaller.
const SHRANK_HEADER: &str = "Compression complete";

/// The first line of a report of a run that did not make the file smaller. The
/// Go tool that this crate replaces logs the same sense at the warning level,
/// thus the line carries the word `Warning` and the report paints it yellow.
const GREW_HEADER: &str = "Warning: compression complete, but the file size increased";

/// The label of the count of bytes that the run read.
const ORIGINAL_SIZE_LABEL: &str = "Original size:";

/// The label of the count of bytes that the run wrote.
const NEW_SIZE_LABEL: &str = "New size:";

/// The label of the change in size.
const SIZE_CHANGE_LABEL: &str = "Size change:";

/// The label of the time that the run took.
const DURATION_LABEL: &str = "Duration:";

/// The label of the rate at which the run read the input.
const BYTES_READ_LABEL: &str = "Bytes read per second:";

/// The label of the rate at which the run wrote the output.
const BYTES_WRITTEN_LABEL: &str = "Bytes written per second:";

/// The count of columns that holds every label. It is the length of
/// [`BYTES_WRITTEN_LABEL`], which is the longest of the six labels, thus every
/// value starts in the same column and a reader can read the values down the
/// page.
const LABEL_WIDTH: usize = BYTES_WRITTEN_LABEL.len();

/// The text that starts each value line. The indent puts the values under the
/// header.
const VALUE_INDENT: &str = "  ";

/// The text that takes the place of the size change when the input holds no
/// bytes. No percentage of zero exists, thus the report gives the reason in
/// place of a number.
const NO_SIZE_CHANGE: &str = "not available (the input holds no bytes)";

/// The sign that marks the size change as a percentage.
const PERCENT_SIGN: &str = "%";

/// The unit that follows a count of bytes.
const BYTES_UNIT: &str = " bytes";

/// Render the report that closes a run.
///
/// The first line names the result of the run: `Compression complete` for a run
/// that made the file smaller, and a warning line for a run that did not. That
/// line, and only that line, carries color — green for the first case and
/// yellow for the second. Six lines follow it, one for each value that the Go
/// tool logs: the original size, the new size, the size change, the duration,
/// and the two rates.
///
/// Every number goes through [`format_int`], [`format_float`], or
/// [`format_duration`], thus the whole report follows one locale.
///
/// The size change is positive when the file became smaller and negative when
/// it became larger. An input of no bytes has no size change, because no
/// percentage of zero exists. Such a report shows the words
/// `not available (the input holds no bytes)` in place of the number.
///
/// The string does not end with a newline. The caller adds one.
pub fn format_report(stats: &Stats, locale: &Locale) -> String {
    let header = if stats.grew() {
        GREW_HEADER.yellow()
    } else {
        SHRANK_HEADER.green()
    };
    let size_change = stats.size_change_percent().map_or_else(
        || NO_SIZE_CHANGE.to_string(),
        |percent| {
            let number = format_float(percent, locale);
            format!("{number}{PERCENT_SIGN}")
        },
    );
    [
        header.to_string(),
        value_line(
            ORIGINAL_SIZE_LABEL,
            &format_size(stats.original_size, locale),
        ),
        value_line(NEW_SIZE_LABEL, &format_size(stats.new_size, locale)),
        value_line(SIZE_CHANGE_LABEL, &size_change),
        value_line(DURATION_LABEL, &format_duration(stats.duration, locale)),
        value_line(
            BYTES_READ_LABEL,
            &format_float(stats.bytes_read_per_second(), locale),
        ),
        value_line(
            BYTES_WRITTEN_LABEL,
            &format_float(stats.bytes_written_per_second(), locale),
        ),
    ]
    .join("\n")
}

/// Render one line of the report: the indent, the label in a column of the
/// width of the longest label, and the value.
fn value_line(label: &str, value: &str) -> String {
    format!("{VALUE_INDENT}{label:<LABEL_WIDTH$} {value}")
}

/// Render a count of bytes with the separator of the locale and the unit.
fn format_size(size: u64, locale: &Locale) -> String {
    let count = format_int(size, locale);
    format!("{count}{BYTES_UNIT}")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        format_duration, format_float, format_int, format_report, locale_from_lang, Locale, Stats,
        GREW_HEADER, SHRANK_HEADER,
    };

    #[test]
    fn a_lang_with_a_codeset_gives_the_language_of_the_lang() {
        assert_eq!(locale_from_lang("de_DE.UTF-8").name(), "de");
        assert_eq!(locale_from_lang("en_US.UTF-8").name(), "en");
    }

    #[test]
    fn a_lang_with_a_modifier_gives_the_language_of_the_lang() {
        assert_eq!(locale_from_lang("de_DE.UTF-8@euro").name(), "de");
        assert_eq!(locale_from_lang("de_DE@euro").name(), "de");
    }

    #[test]
    fn a_lang_that_the_table_holds_gives_that_entry() {
        assert_eq!(locale_from_lang("de_CH").name(), "de-CH");
        assert_eq!(locale_from_lang("de_CH.UTF-8").name(), "de-CH");
    }

    #[test]
    fn a_lang_that_names_no_locale_gives_english() {
        assert_eq!(locale_from_lang("C").name(), "en");
        assert_eq!(locale_from_lang("POSIX").name(), "en");
        assert_eq!(locale_from_lang("").name(), "en");
        assert_eq!(locale_from_lang("zz_ZZ.UTF-8").name(), "en");
    }

    #[test]
    fn format_int_uses_the_separator_of_the_locale() {
        assert_eq!(format_int(1_234_567, &Locale::en), "1,234,567");
        assert_eq!(format_int(1_234_567, &Locale::de), "1.234.567");
        assert_eq!(format_int(0, &Locale::de), "0");
    }

    #[test]
    fn format_float_uses_both_separators_of_the_locale() {
        assert_eq!(format_float(1_234_567.891, &Locale::en), "1,234,567.89");
        assert_eq!(format_float(1_234_567.891, &Locale::de), "1.234.567,89");
        assert_eq!(format_float(0.5, &Locale::en), "0.50");
        assert_eq!(format_float(0.5, &Locale::de), "0,50");
    }

    #[test]
    fn format_float_keeps_the_sign_of_a_negative_value() {
        assert_eq!(format_float(-1.789, &Locale::en), "-1.79");
        assert_eq!(format_float(-1.789, &Locale::de), "-1,79");
        assert_eq!(format_float(-1_234.5, &Locale::de), "-1.234,50");
    }

    #[test]
    fn format_float_of_a_value_that_is_not_finite_gives_the_word_of_the_locale() {
        assert_eq!(format_float(f64::NAN, &Locale::en), "NaN");
        assert_eq!(format_float(f64::INFINITY, &Locale::en), "\u{221e}");
        assert_eq!(format_float(f64::NEG_INFINITY, &Locale::en), "-\u{221e}");
        assert_eq!(format_float(f64::NAN, &Locale::de), "NaN");
        assert_eq!(format_float(f64::NEG_INFINITY, &Locale::de), "-\u{221e}");
    }

    #[test]
    fn format_duration_of_one_second_or_more_gives_seconds() {
        assert_eq!(
            format_duration(Duration::from_millis(1_500), &Locale::en),
            "1.50s"
        );
        assert_eq!(
            format_duration(Duration::from_millis(1_500), &Locale::de),
            "1,50s"
        );
        assert_eq!(
            format_duration(Duration::from_secs(1), &Locale::en),
            "1.00s"
        );
        assert_eq!(
            format_duration(Duration::from_secs(3_600), &Locale::en),
            "3,600.00s"
        );
    }

    #[test]
    fn format_duration_below_one_second_gives_milliseconds() {
        assert_eq!(
            format_duration(Duration::from_micros(1_500), &Locale::en),
            "1.50ms"
        );
        assert_eq!(
            format_duration(Duration::from_millis(1), &Locale::en),
            "1.00ms"
        );
        assert_eq!(
            format_duration(Duration::from_millis(999), &Locale::de),
            "999,00ms"
        );
    }

    #[test]
    fn format_duration_below_one_millisecond_gives_microseconds() {
        assert_eq!(
            format_duration(Duration::from_nanos(1_500), &Locale::en),
            "1.50\u{b5}s"
        );
        assert_eq!(
            format_duration(Duration::from_nanos(500), &Locale::en),
            "0.50\u{b5}s"
        );
        assert_eq!(format_duration(Duration::ZERO, &Locale::de), "0,00\u{b5}s");
    }

    /// The report of a run that made the file smaller, in English, with the
    /// escape codes taken out.
    const SHRANK_REPORT: &str = concat!(
        "Compression complete\n",
        "  Original size:            1,048,576 bytes\n",
        "  New size:                 524,288 bytes\n",
        "  Size change:              50.00%\n",
        "  Duration:                 1.50s\n",
        "  Bytes read per second:    699,050.67\n",
        "  Bytes written per second: 349,525.33",
    );

    /// The same report in German. The two locales use the opposite pair of
    /// separators, thus every number in this report differs from the number in
    /// [`SHRANK_REPORT`].
    const SHRANK_REPORT_IN_GERMAN: &str = concat!(
        "Compression complete\n",
        "  Original size:            1.048.576 bytes\n",
        "  New size:                 524.288 bytes\n",
        "  Size change:              50,00%\n",
        "  Duration:                 1,50s\n",
        "  Bytes read per second:    699.050,67\n",
        "  Bytes written per second: 349.525,33",
    );

    /// The report of a run that made the file larger, in English, with the
    /// escape codes taken out.
    const GREW_REPORT: &str = concat!(
        "Warning: compression complete, but the file size increased\n",
        "  Original size:            1,000 bytes\n",
        "  New size:                 1,200 bytes\n",
        "  Size change:              -20.00%\n",
        "  Duration:                 2.00s\n",
        "  Bytes read per second:    500.00\n",
        "  Bytes written per second: 600.00",
    );

    /// The report of a run over an input of no bytes, in English, with the
    /// escape codes taken out.
    const EMPTY_INPUT_REPORT: &str = concat!(
        "Warning: compression complete, but the file size increased\n",
        "  Original size:            0 bytes\n",
        "  New size:                 20 bytes\n",
        "  Size change:              not available (the input holds no bytes)\n",
        "  Duration:                 1.50s\n",
        "  Bytes read per second:    0.00\n",
        "  Bytes written per second: 13.33",
    );

    /// The escape sequence that starts green text.
    const GREEN_START: &str = "\u{1b}[32m";

    /// The escape sequence that starts yellow text.
    const YELLOW_START: &str = "\u{1b}[33m";

    /// The character that starts every escape sequence.
    const ESCAPE: char = '\u{1b}';

    /// A run that made the file smaller.
    fn shrank() -> Stats {
        Stats {
            original_size: 1_048_576,
            new_size: 524_288,
            duration: Duration::from_millis(1_500),
        }
    }

    /// A run that made the file larger.
    fn grew() -> Stats {
        Stats {
            original_size: 1_000,
            new_size: 1_200,
            duration: Duration::from_secs(2),
        }
    }

    /// A run over an input of no bytes.
    fn empty_input() -> Stats {
        Stats {
            original_size: 0,
            new_size: 20,
            duration: Duration::from_millis(1_500),
        }
    }

    /// Render the report with the escape codes forced on and then taken out,
    /// so the assertion reads the glyphs that a user reads.
    fn glyphs_of(stats: &Stats, locale: &Locale) -> String {
        testcolor::strip_ansi(&testcolor::with_forced_ansi(|| {
            format_report(stats, locale)
        }))
    }

    /// Render the report with the escape codes forced on and left in place.
    fn painted(stats: &Stats) -> String {
        testcolor::with_forced_ansi(|| format_report(stats, &Locale::en))
    }

    /// Give the first line of a report and the lines under it.
    fn split_header(report: &str) -> (&str, Vec<&str>) {
        let mut lines = report.lines();
        let header = lines.next().unwrap_or_default();
        (header, lines.collect())
    }

    #[test]
    fn a_report_of_a_run_that_made_the_file_smaller_holds_the_six_values() {
        assert_eq!(glyphs_of(&shrank(), &Locale::en), SHRANK_REPORT);
    }

    #[test]
    fn a_report_of_a_run_that_made_the_file_larger_warns_and_shows_a_negative_change() {
        let report = glyphs_of(&grew(), &Locale::en);
        assert_eq!(report, GREW_REPORT);
        assert!(report.contains("-20.00%"), "the report is {report}");
    }

    #[test]
    fn a_report_of_an_input_of_no_bytes_names_the_reason_for_the_missing_change() {
        assert_eq!(glyphs_of(&empty_input(), &Locale::en), EMPTY_INPUT_REPORT);
    }

    #[test]
    fn a_report_follows_the_locale_that_it_gets() {
        let stats = shrank();
        let english = glyphs_of(&stats, &Locale::en);
        let german = glyphs_of(&stats, &Locale::de);
        assert_eq!(english, SHRANK_REPORT);
        assert_eq!(german, SHRANK_REPORT_IN_GERMAN);
        assert_ne!(english, german);
    }

    #[test]
    fn the_header_of_a_run_that_made_the_file_smaller_is_green() {
        let report = painted(&shrank());
        let (header, values) = split_header(&report);
        assert!(header.starts_with(GREEN_START), "the header is {header:?}");
        assert!(header.contains(SHRANK_HEADER), "the header is {header:?}");
        for line in values {
            assert!(!line.contains(ESCAPE), "the line is {line:?}");
        }
    }

    #[test]
    fn the_header_of_a_run_that_made_the_file_larger_is_yellow() {
        let report = painted(&grew());
        let (header, values) = split_header(&report);
        assert!(header.starts_with(YELLOW_START), "the header is {header:?}");
        assert!(header.contains(GREW_HEADER), "the header is {header:?}");
        for line in values {
            assert!(!line.contains(ESCAPE), "the line is {line:?}");
        }
    }
}
