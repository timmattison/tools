//! Tests for the idle time compression filter.

use super::*;
use crate::types::Seconds;

/// The settings of most tests: compress each period of 3 s or more to 1 s, on
/// a screen of 20 columns and 5 rows.
fn settings() -> IdleCompression {
    IdleCompression::new(3.0, 1.0, 20, 5).unwrap()
}

fn assert_invalid(result: Result<IdleCompression, NleError>, needle: &str) {
    match result {
        Err(NleError::InvalidIdleCompression(message)) => {
            assert!(message.contains(needle), "{needle} missing in {message}");
        }
        other => panic!("expected InvalidIdleCompression with {needle}, got {other:?}"),
    }
}

// Tests of the settings.

#[test]
fn new_keeps_valid_settings() {
    let settings = IdleCompression::new(3.0, 0.5, 80, 24).unwrap();

    assert_eq!(settings.threshold(), 3.0);
    assert_eq!(settings.compress_to(), 0.5);
    assert_eq!((settings.width(), settings.height()), (80, 24));
    assert_eq!(
        (settings.ignore_top_rows(), settings.ignore_bottom_rows()),
        (0, 0)
    );
}

#[test]
fn new_accepts_a_compressed_length_of_zero_and_one_equal_to_the_threshold() {
    assert!(IdleCompression::new(3.0, 0.0, 80, 24).is_ok());
    assert!(IdleCompression::new(3.0, 3.0, 80, 24).is_ok());
}

#[test]
fn new_refuses_a_bad_threshold() {
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_invalid(IdleCompression::new(bad, 0.0, 80, 24), "threshold");
    }
}

#[test]
fn new_refuses_a_bad_compressed_length() {
    for bad in [-0.5, 3.5, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_invalid(IdleCompression::new(3.0, bad, 80, 24), "compressed length");
    }
}

#[test]
fn new_refuses_a_screen_with_no_cell_or_too_many_cells() {
    for (width, height) in [(0, 24), (80, 0), (0, 0), (u16::MAX, u16::MAX), (4097, 1024)] {
        assert_invalid(IdleCompression::new(3.0, 1.0, width, height), "screen");
    }
    assert!(IdleCompression::new(3.0, 1.0, 4096, 1024).is_ok());
}

#[test]
fn ignoring_rows_keeps_rows_that_fit_on_the_screen() {
    let ignoring = settings().ignoring_rows(1, 2).unwrap();
    assert_eq!(
        (ignoring.ignore_top_rows(), ignoring.ignore_bottom_rows()),
        (1, 2)
    );
    // All 5 rows can be ignored. Then only a key is activity.
    assert!(settings().ignoring_rows(2, 3).is_ok());
}

#[test]
fn ignoring_rows_refuses_more_rows_than_the_screen_has() {
    assert_invalid(settings().ignoring_rows(3, 3), "rows");
    assert_invalid(settings().ignoring_rows(u16::MAX, u16::MAX), "rows");
}

// Tests of the options.

#[test]
fn settings_of_reads_back_an_applied_filter() {
    let settings = settings().ignoring_rows(1, 2).unwrap();
    let range = FilterRegion::Range {
        start: Seconds::new(1.0).unwrap(),
        end: Seconds::new(9.0).unwrap(),
    };

    for region in [FilterRegion::All, range] {
        let filter = applied(region.clone(), settings);
        assert_eq!(filter.kind, kind());
        assert_eq!(filter.region, region);
        assert_eq!(settings_of(&filter), Some(settings));
    }
}

#[test]
fn settings_of_is_none_for_another_kind() {
    let mut filter = applied(FilterRegion::All, settings());
    filter.kind = FilterKind::new("time-dilation");

    assert_eq!(settings_of(&filter), None);
}

/// The options of `settings()`, with `key` set to `value`.
fn options_with(key: &str, value: serde_json::Value) -> AppliedFilter {
    let mut filter = applied(FilterRegion::All, settings());
    filter.options = filter.options.with(key, value);
    filter
}

/// The options of `settings()`, with `key` taken out.
fn options_without(key: &str) -> AppliedFilter {
    let full = applied(FilterRegion::All, settings());
    let mut options = FilterOptions::new();
    for name in [
        THRESHOLD_OPTION,
        COMPRESS_TO_OPTION,
        WIDTH_OPTION,
        HEIGHT_OPTION,
        IGNORE_TOP_ROWS_OPTION,
        IGNORE_BOTTOM_ROWS_OPTION,
    ] {
        if name != key {
            let value = full.options.get(name).cloned().unwrap();
            options = options.with(name, value);
        }
    }
    AppliedFilter { options, ..full }
}

#[test]
fn settings_of_is_none_for_a_missing_or_bad_option() {
    let names = [
        THRESHOLD_OPTION,
        COMPRESS_TO_OPTION,
        WIDTH_OPTION,
        HEIGHT_OPTION,
        IGNORE_TOP_ROWS_OPTION,
        IGNORE_BOTTOM_ROWS_OPTION,
    ];
    for name in names {
        assert_eq!(settings_of(&options_without(name)), None, "{name} missing");
        let text = serde_json::Value::from("3");
        assert_eq!(settings_of(&options_with(name, text)), None, "{name} text");
    }
    for name in [
        WIDTH_OPTION,
        HEIGHT_OPTION,
        IGNORE_TOP_ROWS_OPTION,
        IGNORE_BOTTOM_ROWS_OPTION,
    ] {
        for bad in [
            serde_json::Value::from(1.5),
            serde_json::Value::from(-1),
            serde_json::Value::from(70_000),
        ] {
            assert_eq!(
                settings_of(&options_with(name, bad.clone())),
                None,
                "{name} {bad}"
            );
        }
    }
    // Each value is of the right type, but the settings break a rule.
    let zero = serde_json::Value::from(0.0);
    assert_eq!(settings_of(&options_with(THRESHOLD_OPTION, zero)), None);
    let rows = serde_json::Value::from(9);
    assert_eq!(
        settings_of(&options_with(IGNORE_TOP_ROWS_OPTION, rows)),
        None
    );
}
