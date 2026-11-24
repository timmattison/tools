//! Tests for the compression core of `prgz`.
//!
//! Each test that touches the file system makes its own temporary directory,
//! thus two copies of this test binary can run at the same moment.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "every unwrap and expect in this file is an assertion, not an unhandled error"
)]

use std::cell::Cell;
use std::fs;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use flate2::read::GzDecoder;
use prgz::{compress_file, compress_stream, default_output_path, CompressError, Stats};
use tempfile::TempDir;

/// The largest difference that two fractions can have and still count as equal.
const TOLERANCE: f64 = 1e-9;

/// Answers whether two fractions are equal to the tolerance of the tests.
fn close(left: f64, right: f64) -> bool {
    (left - right).abs() < TOLERANCE
}

/// Makes one set of statistics for a test.
fn stats(original_size: u64, new_size: u64, duration: Duration) -> Stats {
    Stats {
        original_size,
        new_size,
        duration,
    }
}

#[test]
fn the_default_output_path_appends_gz_to_the_whole_name() {
    assert_eq!(
        default_output_path(Path::new("notes.txt")),
        PathBuf::from("notes.txt.gz")
    );
    assert_eq!(
        default_output_path(Path::new("notes.tar")),
        PathBuf::from("notes.tar.gz")
    );
    assert_eq!(
        default_output_path(Path::new("archive")),
        PathBuf::from("archive.gz")
    );
    assert_eq!(
        default_output_path(Path::new("/one/two/notes.tar")),
        PathBuf::from("/one/two/notes.tar.gz")
    );
}

#[test]
fn the_default_output_path_keeps_every_character_of_a_multi_byte_name() {
    assert_eq!(
        default_output_path(Path::new("\u{65e5}\u{672c}\u{8a9e}.txt")),
        PathBuf::from("\u{65e5}\u{672c}\u{8a9e}.txt.gz")
    );
    assert_eq!(
        default_output_path(Path::new("caf\u{e9}.txt")),
        PathBuf::from("caf\u{e9}.txt.gz")
    );
    assert_eq!(
        default_output_path(Path::new("\u{1f389}.txt")),
        PathBuf::from("\u{1f389}.txt.gz")
    );
}

#[cfg(unix)]
#[test]
fn the_default_output_path_keeps_a_name_that_is_not_utf_8() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let name = OsStr::from_bytes(b"bad\xffname.bin");
    let expected = OsStr::from_bytes(b"bad\xffname.bin.gz");
    assert_eq!(
        default_output_path(Path::new(name)),
        PathBuf::from(expected)
    );
}

#[test]
fn a_smaller_output_did_not_grow() {
    let smaller = stats(1_000, 400, Duration::from_secs(2));
    assert!(!smaller.grew());
    let percent = smaller.size_change_percent().unwrap();
    assert!(close(percent, 60.0), "the percent is {percent}");
}

#[test]
fn an_output_of_the_same_size_grew() {
    let same = stats(1_000, 1_000, Duration::from_secs(1));
    assert!(same.grew());
    let percent = same.size_change_percent().unwrap();
    assert!(close(percent, 0.0), "the percent is {percent}");
}

#[test]
fn a_larger_output_grew_and_gives_a_negative_percent() {
    let larger = stats(1_000, 1_100, Duration::from_secs(1));
    assert!(larger.grew());
    let percent = larger.size_change_percent().unwrap();
    assert!(close(percent, -10.0), "the percent is {percent}");
}

#[test]
fn an_original_size_of_zero_gives_no_percent() {
    let empty = stats(0, 20, Duration::from_secs(1));
    assert!(empty.grew());
    assert_eq!(empty.size_change_percent(), None);
}

#[test]
fn the_rates_divide_the_sizes_by_the_seconds() {
    let measured = stats(1_000, 400, Duration::from_secs(2));
    let read_rate = measured.bytes_read_per_second();
    let written_rate = measured.bytes_written_per_second();
    assert!(close(read_rate, 500.0), "the read rate is {read_rate}");
    assert!(
        close(written_rate, 200.0),
        "the written rate is {written_rate}"
    );
}

#[test]
fn a_duration_of_zero_gives_a_rate_of_zero() {
    let instant = stats(1_000, 400, Duration::ZERO);
    let read_rate = instant.bytes_read_per_second();
    let written_rate = instant.bytes_written_per_second();
    assert!(close(read_rate, 0.0), "the read rate is {read_rate}");
    assert!(
        close(written_rate, 0.0),
        "the written rate is {written_rate}"
    );
}

/// The count of bytes in an input that covers more than one read buffer.
const MANY_BUFFERS: usize = 500_000;

/// A reader that answers a set count of bytes and then fails.
struct FailingReader {
    /// The count of bytes that the reader gives before it fails.
    remaining: usize,
}

impl Read for FailingReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Err(io::Error::other("the reader stopped"));
        }
        let count = self.remaining.min(buffer.len());
        buffer[..count].fill(b'a');
        self.remaining -= count;
        Ok(count)
    }
}

/// A writer that fails every write and every flush.
struct FailingWriter;

impl Write for FailingWriter {
    fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("the writer stopped"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::other("the writer stopped"))
    }
}

#[test]
fn compress_stream_makes_a_gzip_stream_of_the_input() {
    let input = b"the same line, over and over, compresses well. ".repeat(100);
    let mut output = Vec::new();
    let count = compress_stream(input.as_slice(), &mut output, &|| false, &mut |_| {}).unwrap();
    assert_eq!(count, input.len() as u64);
    assert!(!output.is_empty(), "the output holds no bytes");
    let mut decoded = Vec::new();
    GzDecoder::new(output.as_slice())
        .read_to_end(&mut decoded)
        .unwrap();
    assert_eq!(decoded, input);
}

#[test]
fn compress_stream_reports_the_count_after_each_buffer() {
    let input = vec![b'x'; MANY_BUFFERS];
    let mut output = Vec::new();
    let mut counts: Vec<u64> = Vec::new();
    let count = compress_stream(input.as_slice(), &mut output, &|| false, &mut |read| {
        counts.push(read);
    })
    .unwrap();
    assert_eq!(count, MANY_BUFFERS as u64);
    assert!(counts.len() > 1, "the count of reports is {}", counts.len());
    assert!(
        counts.windows(2).all(|pair| pair[0] < pair[1]),
        "the reports do not rise: {counts:?}"
    );
    assert_eq!(counts.last().copied(), Some(MANY_BUFFERS as u64));
}

#[test]
fn a_stream_that_the_user_stops_says_the_user_stopped_it() {
    let input = vec![b'x'; MANY_BUFFERS];
    let mut output = Vec::new();
    let calls = Cell::new(0_u32);
    let stopped = || {
        let seen = calls.get();
        calls.set(seen + 1);
        seen > 0
    };
    let error = compress_stream(input.as_slice(), &mut output, &stopped, &mut |_| {}).unwrap_err();
    assert!(
        matches!(error, CompressError::Cancelled),
        "the error is {error}"
    );
}

#[test]
fn a_read_that_fails_after_progress_gives_a_read_error() {
    let reader = FailingReader {
        remaining: MANY_BUFFERS,
    };
    let mut output = Vec::new();
    let mut reports = 0_u32;
    let error = compress_stream(reader, &mut output, &|| false, &mut |_| reports += 1).unwrap_err();
    assert!(
        matches!(error, CompressError::ReadInput { .. }),
        "the error is {error}"
    );
    assert!(reports > 0, "the run made no progress before it failed");
}

#[test]
fn a_write_that_fails_gives_a_write_error() {
    let input = vec![b'x'; MANY_BUFFERS];
    let error =
        compress_stream(input.as_slice(), FailingWriter, &|| false, &mut |_| {}).unwrap_err();
    assert!(
        matches!(error, CompressError::WriteOutput { .. }),
        "the error is {error}"
    );
}

#[test]
fn a_write_that_only_fails_at_the_finish_gives_a_write_error() {
    let empty: &[u8] = &[];
    let error = compress_stream(empty, FailingWriter, &|| false, &mut |_| {}).unwrap_err();
    assert!(
        matches!(error, CompressError::WriteOutput { .. }),
        "the error is {error}"
    );
}

/// The start value of the generator of bytes that gzip cannot compress.
const RANDOM_SEED: u64 = 0x2545_f491_4f6c_dd1d;

/// The count of bytes in the block that gzip cannot compress.
const RANDOM_BYTES: usize = 64 * 1024;

/// Makes a temporary directory for one test. Each directory has its own name,
/// thus two copies of this test binary do not touch the same files.
fn temp_dir() -> TempDir {
    TempDir::new().unwrap()
}

/// Makes a block of bytes that gzip cannot make smaller. The generator is a
/// xorshift, thus the same seed always gives the same bytes.
fn incompressible_bytes(count: usize) -> Vec<u8> {
    let mut state = RANDOM_SEED;
    let mut bytes = Vec::with_capacity(count);
    while bytes.len() < count {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        bytes.extend_from_slice(&state.to_le_bytes());
    }
    bytes.truncate(count);
    bytes
}

/// Reads the bytes that a gzip file holds.
fn decompress(path: &Path) -> Vec<u8> {
    let mut decoded = Vec::new();
    GzDecoder::new(File::open(path).unwrap())
        .read_to_end(&mut decoded)
        .unwrap();
    decoded
}

#[test]
fn a_compressed_file_gives_the_input_bytes_back() {
    let dir = temp_dir();
    let input = dir.path().join("input.txt");
    let output = default_output_path(&input);
    let bytes = b"one line that repeats many times. ".repeat(500);
    fs::write(&input, &bytes).unwrap();
    let stats = compress_file(&input, &output, &|_| true, &|| false, &mut |_| {}).unwrap();
    assert_eq!(stats.original_size, bytes.len() as u64);
    assert_eq!(stats.new_size, fs::metadata(&output).unwrap().len());
    assert!(!stats.grew());
    assert_eq!(decompress(&output), bytes);
}

#[test]
fn an_empty_input_gives_an_empty_gzip_stream_that_grew() {
    let dir = temp_dir();
    let input = dir.path().join("empty.bin");
    let output = default_output_path(&input);
    fs::write(&input, b"").unwrap();
    let stats = compress_file(&input, &output, &|_| true, &|| false, &mut |_| {}).unwrap();
    assert_eq!(stats.original_size, 0);
    assert!(stats.new_size > 0, "the gzip stream holds no bytes");
    assert_eq!(stats.new_size, fs::metadata(&output).unwrap().len());
    assert!(stats.grew());
    assert_eq!(stats.size_change_percent(), None);
    assert!(decompress(&output).is_empty());
}

#[test]
fn bytes_that_gzip_cannot_compress_give_a_larger_file() {
    let dir = temp_dir();
    let input = dir.path().join("random.bin");
    let output = default_output_path(&input);
    let bytes = incompressible_bytes(RANDOM_BYTES);
    fs::write(&input, &bytes).unwrap();
    let stats = compress_file(&input, &output, &|_| true, &|| false, &mut |_| {}).unwrap();
    assert!(
        stats.new_size > stats.original_size,
        "the output holds {} bytes and the input holds {} bytes",
        stats.new_size,
        stats.original_size
    );
    assert!(stats.grew());
    let percent = stats.size_change_percent().unwrap();
    assert!(percent < 0.0, "the percent is {percent}");
    assert_eq!(decompress(&output), bytes);
}

#[test]
fn a_file_of_many_buffers_reports_the_progress_of_the_read() {
    let dir = temp_dir();
    let input = dir.path().join("many.bin");
    let output = default_output_path(&input);
    fs::write(&input, vec![b'x'; MANY_BUFFERS]).unwrap();
    let mut counts: Vec<u64> = Vec::new();
    let stats = compress_file(&input, &output, &|_| true, &|| false, &mut |read| {
        counts.push(read);
    })
    .unwrap();
    assert_eq!(stats.original_size, MANY_BUFFERS as u64);
    assert!(counts.len() > 1, "the count of reports is {}", counts.len());
    assert!(
        counts.windows(2).all(|pair| pair[0] < pair[1]),
        "the reports do not rise: {counts:?}"
    );
    assert_eq!(counts.last().copied(), Some(MANY_BUFFERS as u64));
}

#[test]
fn a_missing_input_file_says_it_could_not_open_the_input() {
    let dir = temp_dir();
    let input = dir.path().join("missing.txt");
    let output = default_output_path(&input);
    let error = compress_file(&input, &output, &|_| true, &|| false, &mut |_| {}).unwrap_err();
    assert!(
        matches!(error, CompressError::OpenInput { .. }),
        "the error is {error}"
    );
    assert!(!output.exists(), "the run made an output file");
}

#[test]
fn an_output_in_a_missing_directory_says_it_could_not_create_the_output() {
    let dir = temp_dir();
    let input = dir.path().join("input.txt");
    fs::write(&input, b"some bytes").unwrap();
    let output = dir.path().join("no-such-directory").join("input.txt.gz");
    let error = compress_file(&input, &output, &|_| true, &|| false, &mut |_| {}).unwrap_err();
    assert!(
        matches!(error, CompressError::CreateOutput { .. }),
        "the error is {error}"
    );
    assert!(!output.exists(), "the run made an output file");
}

/// A directory opens as a file, and the first read of that file fails. The run
/// thus makes the output file and then fails on the read.
#[cfg(unix)]
#[test]
fn a_read_that_fails_leaves_no_output_file() {
    let dir = temp_dir();
    let input = dir.path().join("a-directory");
    fs::create_dir(&input).unwrap();
    let output = default_output_path(&input);
    let error = compress_file(&input, &output, &|_| true, &|| false, &mut |_| {}).unwrap_err();
    assert!(
        matches!(error, CompressError::ReadInput { .. }),
        "the error is {error}"
    );
    assert!(!output.exists(), "the run left an output file");
}

#[test]
fn compress_file_over_the_same_path_for_input_and_output_leaves_the_file_untouched() {
    let dir = temp_dir();
    let input = dir.path().join("notes.txt");
    let bytes = b"the bytes that a refusal must not lose".to_vec();
    fs::write(&input, &bytes).unwrap();

    let error = compress_file(&input, &input, &|_| true, &|| false, &mut |_| {});

    assert!(error.is_err(), "the run answered a success");
    assert_eq!(
        fs::read(&input).unwrap(),
        bytes,
        "the run changed the bytes of the input file"
    );
}

#[test]
fn a_run_that_the_user_stops_leaves_no_output_file() {
    let dir = temp_dir();
    let input = dir.path().join("many.bin");
    let output = default_output_path(&input);
    fs::write(&input, vec![b'x'; MANY_BUFFERS]).unwrap();
    let calls = Cell::new(0_u32);
    let stopped = || {
        let seen = calls.get();
        calls.set(seen + 1);
        seen > 0
    };
    let error = compress_file(&input, &output, &|_| true, &stopped, &mut |_| {}).unwrap_err();
    assert!(
        matches!(error, CompressError::Cancelled),
        "the error is {error}"
    );
    assert!(!output.exists(), "the run left an output file");
}

#[test]
fn compress_file_refuses_to_replace_an_existing_output_when_may_replace_says_no() {
    let dir = temp_dir();
    let input = dir.path().join("input.txt");
    fs::write(&input, b"new bytes that must never reach the output").unwrap();
    let output = default_output_path(&input);
    let original = b"the bytes that a refusal must not lose".to_vec();
    fs::write(&output, &original).unwrap();

    let error = compress_file(&input, &output, &|_| false, &|| false, &mut |_| {}).unwrap_err();

    assert!(
        matches!(error, CompressError::OutputExists { .. }),
        "the error is {error}"
    );
    assert_eq!(
        fs::read(&output).unwrap(),
        original,
        "the refusal changed the bytes of the output file"
    );
}

#[test]
fn compress_file_replaces_an_existing_output_when_may_replace_says_yes() {
    let dir = temp_dir();
    let input = dir.path().join("input.txt");
    let bytes = b"one line that repeats many times. ".repeat(500);
    fs::write(&input, &bytes).unwrap();
    let output = default_output_path(&input);
    fs::write(&output, b"stale content that the run must lose").unwrap();

    let stats = compress_file(&input, &output, &|_| true, &|| false, &mut |_| {}).unwrap();

    assert_eq!(stats.original_size, bytes.len() as u64);
    assert_eq!(decompress(&output), bytes);
}

#[test]
fn compress_file_asks_may_replace_only_when_the_output_is_already_there() {
    let dir = temp_dir();
    let input = dir.path().join("input.txt");
    fs::write(&input, b"some bytes").unwrap();
    let output = default_output_path(&input);
    let asked = Cell::new(false);

    let stats = compress_file(
        &input,
        &output,
        &|_| {
            asked.set(true);
            true
        },
        &|| false,
        &mut |_| {},
    )
    .unwrap();

    assert!(
        !asked.get(),
        "the run asked whether it may replace an output file that was not there"
    );
    assert_eq!(stats.original_size, 10);
}

/// A directory opens as a file, and the first read of that file fails. The run
/// thus fails after it opened the output, whether that output is new or was
/// already there.
#[cfg(unix)]
#[test]
fn a_failed_run_removes_an_output_file_that_it_made_itself() {
    let dir = temp_dir();
    let input = dir.path().join("a-directory");
    fs::create_dir(&input).unwrap();
    let output = default_output_path(&input);

    let error = compress_file(&input, &output, &|_| true, &|| false, &mut |_| {}).unwrap_err();

    assert!(
        matches!(error, CompressError::ReadInput { .. }),
        "the error is {error}"
    );
    assert!(
        !output.exists(),
        "the run left an output file that it made itself"
    );
}

/// A directory opens as a file, and the first read of that file fails. The
/// output file is already there before the run starts, thus the failure must
/// leave it in place, even though `may_replace` allowed the run to truncate
/// it on the way in.
#[cfg(unix)]
#[test]
fn a_failed_run_leaves_an_output_file_that_was_already_there() {
    let dir = temp_dir();
    let input = dir.path().join("a-directory");
    fs::create_dir(&input).unwrap();
    let output = default_output_path(&input);
    fs::write(&output, b"the file that the run must not delete").unwrap();

    let error = compress_file(&input, &output, &|_| true, &|| false, &mut |_| {}).unwrap_err();

    assert!(
        matches!(error, CompressError::ReadInput { .. }),
        "the error is {error}"
    );
    assert!(
        output.exists(),
        "the run removed an output file that it did not make"
    );
}
