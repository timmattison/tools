//! Tests for the compression core of `prgz`.
//!
//! Each test that touches the file system makes its own temporary directory,
//! thus two copies of this test binary can run at the same moment.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "every unwrap and expect in this file is an assertion, not an unhandled error"
)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use prgz::{default_output_path, Stats};

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
