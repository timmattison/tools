//! Tests of the history source: the pure parse of the atuin output, and the
//! read through a stub `atuin` script.

use chrono::NaiveDate;
use wdid::history::{parse, Exit, HistoryEntry, HistoryError, COMMAND_NOT_FOUND};

/// Makes a time in UTC for the expected rows.
fn time(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> chrono::NaiveDateTime {
    NaiveDate::from_ymd_opt(year, month, day)
        .unwrap()
        .and_hms_opt(hour, minute, second)
        .unwrap()
}

/// Makes one record of the output, with its final NUL.
fn record(time: &str, exit: &str, command: &str) -> Vec<u8> {
    format!("{time}\t{exit}\tmac\ttim\t{command}\0").into_bytes()
}

/// Parses `bytes` and gives the only entry.
fn only(bytes: &[u8]) -> HistoryEntry {
    let mut entries = parse(bytes).unwrap();
    assert_eq!(entries.len(), 1, "{entries:?}");
    entries.remove(0)
}

#[test]
fn parse_reads_all_five_fields() {
    let entry = only(&record("2026-10-14 08:30:05", "0", "prcp a b"));
    assert_eq!(
        entry,
        HistoryEntry {
            time: time(2026, 10, 14, 8, 30, 5),
            exit: Exit::new(0),
            host: "mac".to_owned(),
            user: "tim".to_owned(),
            command: "prcp a b".to_owned(),
        }
    );
}

#[test]
fn parse_reads_many_records_in_order() {
    let mut bytes = record("2026-10-14 08:30:05", "0", "first");
    bytes.extend(record("2026-10-14 08:30:06", "1", "second"));
    let entries = parse(&bytes).unwrap();
    let commands: Vec<&str> = entries.iter().map(|e| e.command.as_str()).collect();
    assert_eq!(commands, ["first", "second"]);
    assert_eq!(entries[1].exit.get(), 1);
}

#[test]
fn parse_keeps_a_newline_in_a_command() {
    let entry = only(&record(
        "2026-10-14 08:30:05",
        "0",
        "cat <<'EOF'\nprcp\nEOF",
    ));
    assert_eq!(entry.command, "cat <<'EOF'\nprcp\nEOF");
}

#[test]
fn parse_keeps_a_tab_in_a_command() {
    let entry = only(&record("2026-10-14 08:30:05", "0", "echo a\tb\tc\td"));
    assert_eq!(entry.command, "echo a\tb\tc\td");
}

#[test]
fn parse_reads_the_exit_code_minus_one() {
    let entry = only(&record("2026-10-14 08:30:05", "-1", "exit"));
    assert_eq!(entry.exit.get(), -1);
    assert!(!entry.exit.is_command_not_found());
}

#[test]
fn parse_reads_the_exit_code_127_as_command_not_found() {
    let entry = only(&record("2026-10-14 08:30:05", "127", "prcpp"));
    assert_eq!(entry.exit.get(), COMMAND_NOT_FOUND);
    assert!(entry.exit.is_command_not_found());
}

#[test]
fn parse_reads_a_last_record_without_a_trailing_nul() {
    let mut bytes = record("2026-10-14 08:30:05", "0", "first");
    bytes.extend(record("2026-10-14 08:30:06", "0", "last"));
    bytes.pop();
    let entries = parse(&bytes).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].command, "last");
}

#[test]
fn parse_reads_multibyte_text() {
    let entry = only(&record("2026-10-14 08:30:05", "0", "prcp 日本語.txt 🎉/"));
    assert_eq!(entry.command, "prcp 日本語.txt 🎉/");
}

#[test]
fn parse_reads_an_empty_command() {
    let entry = only(&record("2026-10-14 08:30:05", "0", ""));
    assert_eq!(entry.command, "");
}

#[test]
fn parse_reads_invalid_utf8_in_a_command_lossily() {
    let entry = only(b"2026-10-14 08:30:05\t0\tmac\ttim\techo \xff\0");
    assert_eq!(entry.command, "echo \u{fffd}");
}

#[test]
fn parse_gives_an_empty_list_for_empty_output() {
    assert!(parse(b"").unwrap().is_empty());
}

/// Checks that `bytes` give `Malformed` for `record`, and that the reason holds `needle`.
fn assert_malformed(bytes: &[u8], expected_record: usize, needle: &str) {
    match parse(bytes) {
        Err(HistoryError::Malformed { record, reason }) => {
            assert_eq!(record, expected_record, "{reason}");
            assert!(reason.contains(needle), "{reason:?} lacks {needle:?}");
        }
        other => panic!("expected Malformed, got {other:?}"),
    }
}

#[test]
fn parse_refuses_a_record_with_too_few_fields() {
    let mut bytes = record("2026-10-14 08:30:05", "0", "ok");
    bytes.extend(b"2026-10-14 08:30:06\t0\tmac\0");
    assert_malformed(&bytes, 2, "fields");
}

#[test]
fn parse_refuses_a_bad_time() {
    let bytes = record("yesterday", "0", "ok");
    assert_malformed(&bytes, 1, "time");
}

#[test]
fn parse_refuses_a_bad_exit() {
    let bytes = record("2026-10-14 08:30:05", "zero", "ok");
    assert_malformed(&bytes, 1, "exit");
}

#[test]
fn malformed_message_names_the_record() {
    let error = parse(&record("yesterday", "0", "ok")).unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("atuin search gave a record that wdid cannot read (record 1): "),
        "{error}"
    );
}
