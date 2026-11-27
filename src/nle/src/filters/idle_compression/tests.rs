//! Tests for the idle time compression filter.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::*;
use crate::filters;
use crate::pipeline::{Mark, Rendered};
use crate::types::{EditProject, Event, EventType, Recording, Seconds};

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

// Tests of the compression.

fn event(event_type: EventType, time: f64, data: &str) -> Event {
    Event {
        time,
        event_type,
        data: data.to_string(),
    }
}

fn output(time: f64, data: &str) -> Event {
    event(EventType::Output, time, data)
}

fn input(time: f64, data: &str) -> Event {
    event(EventType::Input, time, data)
}

fn times(events: &[Event]) -> Vec<f64> {
    events.iter().map(|e| e.time).collect()
}

fn mark_times(marks: &[Mark]) -> Vec<f64> {
    marks.iter().map(|mark| mark.time).collect()
}

/// A project of one recording of `duration` seconds on the 20x5 screen of
/// `settings()`, with `events` and `filters`.
fn project_of(events: Vec<Event>, duration: f64, filters: Vec<AppliedFilter>) -> EditProject {
    let recording = Recording {
        version: 2,
        width: 20,
        height: 5,
        timestamp: 0.0,
        duration,
        command: String::new(),
        title: String::new(),
        env: BTreeMap::new(),
        events,
    };
    let mut project = EditProject::from_recording(Arc::new(recording)).unwrap();
    project.filters = filters;
    project
}

/// The render of `events` in a recording of `duration` seconds, with one
/// idle compression of the whole timeline by `settings`.
fn compressed(events: Vec<Event>, duration: f64, settings: IdleCompression) -> Rendered {
    let filter = applied(FilterRegion::All, settings);
    filters::registry()
        .render_with_marks(&project_of(events, duration, vec![filter]))
        .unwrap()
}

#[test]
fn an_idle_gap_of_at_least_the_threshold_shrinks_to_the_compressed_length() {
    let rendered = compressed(vec![output(0.0, "a"), output(10.0, "b")], 11.0, settings());

    assert_eq!(times(&rendered.recording.events), vec![0.0, 1.0]);
    // The gap from 10 s to the end is 1 s, below the threshold, so it stays.
    assert_eq!(rendered.recording.duration, 2.0);
}

#[test]
fn a_gap_below_the_threshold_stays() {
    let events = vec![output(0.0, "a"), output(2.5, "b"), output(5.0, "c")];

    let rendered = compressed(events, 5.0, settings());

    assert_eq!(times(&rendered.recording.events), vec![0.0, 2.5, 5.0]);
    assert_eq!(rendered.recording.duration, 5.0);
}

#[test]
fn a_gap_of_exactly_the_threshold_shrinks() {
    let rendered = compressed(vec![output(0.0, "a"), output(3.0, "b")], 3.0, settings());

    assert_eq!(times(&rendered.recording.events), vec![0.0, 1.0]);
}

#[test]
fn a_compressed_length_of_zero_removes_the_gap() {
    let remove = IdleCompression::new(3.0, 0.0, 20, 5).unwrap();

    let rendered = compressed(vec![output(0.0, "a"), output(10.0, "b")], 10.0, remove);

    assert_eq!(times(&rendered.recording.events), vec![0.0, 0.0]);
    assert_eq!(rendered.recording.duration, 0.0);
}

#[test]
fn a_key_ends_an_idle_period_also_when_the_screen_does_not_change() {
    let events = vec![output(0.0, "a"), input(5.0, "x"), output(10.0, "b")];

    let rendered = compressed(events, 10.0, settings());

    assert_eq!(times(&rendered.recording.events), vec![0.0, 1.0, 2.0]);
    assert_eq!(rendered.recording.duration, 2.0);
}

#[test]
fn output_that_changes_no_cell_does_not_end_an_idle_period() {
    let events = vec![
        output(0.0, "a"),
        // A cursor move, a window title, and a redraw of the same "a".
        output(4.0, "\x1b[H"),
        output(6.0, "\x1b]0;title\x07"),
        output(8.0, "\x1b[1;1Ha"),
        output(10.0, "b"),
    ];

    let rendered = compressed(events.clone(), 10.0, settings());

    // One period of 10 s becomes 1 s, and the times inside it shrink to a tenth.
    assert_eq!(
        times(&rendered.recording.events),
        vec![0.0, 0.4, 0.6, 0.8, 1.0]
    );
    // Each event keeps its type, its data, and its place.
    let kept: Vec<(&EventType, &str)> = rendered
        .recording
        .events
        .iter()
        .map(|e| (&e.event_type, e.data.as_str()))
        .collect();
    let given: Vec<(&EventType, &str)> = events
        .iter()
        .map(|e| (&e.event_type, e.data.as_str()))
        .collect();
    assert_eq!(kept, given);
}

#[test]
fn multi_byte_output_that_changes_a_cell_is_activity() {
    let events = vec![
        output(0.0, "日本語"),
        output(2.0, "🎉"),
        output(4.0, "café"),
    ];

    let rendered = compressed(events, 4.0, IdleCompression::new(1.5, 0.5, 20, 5).unwrap());

    assert_eq!(times(&rendered.recording.events), vec![0.0, 0.5, 1.0]);
}

/// A clock that ticks on the bottom row of the 20x5 screen at 4 s and at 8 s,
/// between a change of the top row at 0 s and at 10 s.
fn bottom_clock() -> Vec<Event> {
    vec![
        output(0.0, "a"),
        output(4.0, "\x1b[5;1H12:00"),
        output(8.0, "\x1b[5;1H12:01"),
        output(10.0, "\x1b[1;2Hb"),
    ]
}

#[test]
fn a_change_of_an_ignored_bottom_row_is_not_activity() {
    let ignoring = settings().ignoring_rows(0, 1).unwrap();

    let rendered = compressed(bottom_clock(), 10.0, ignoring);

    assert_eq!(times(&rendered.recording.events), vec![0.0, 0.4, 0.8, 1.0]);
}

#[test]
fn a_change_of_a_watched_bottom_row_is_activity() {
    let rendered = compressed(bottom_clock(), 10.0, settings());

    // The gaps of 4 s shrink to 1 s, and the gap of 2 s stays.
    assert_eq!(times(&rendered.recording.events), vec![0.0, 1.0, 2.0, 4.0]);
}

#[test]
fn a_change_of_an_ignored_top_row_is_not_activity() {
    let ignoring = settings().ignoring_rows(1, 0).unwrap();
    let events = vec![
        output(0.0, "\x1b[2;1Ha"),
        output(4.0, "\x1b[1;1H12:00"),
        output(10.0, "\x1b[2;2Hb"),
    ];

    let rendered = compressed(events, 10.0, ignoring);

    assert_eq!(times(&rendered.recording.events), vec![0.0, 0.4, 1.0]);
}

#[test]
fn with_each_row_ignored_only_a_key_is_activity() {
    let ignoring = settings().ignoring_rows(2, 3).unwrap();
    let events = vec![output(0.0, "a"), input(5.0, "x"), output(10.0, "b")];

    let rendered = compressed(events, 10.0, ignoring);

    // The periods are 0 s to 5 s and 5 s to 10 s. The output at 10 s is not
    // activity, so it is the end of the timeline that ends the second one.
    assert_eq!(times(&rendered.recording.events), vec![0.0, 1.0, 2.0]);
}

#[test]
fn idle_time_at_the_start_and_at_the_end_shrinks() {
    let rendered = compressed(vec![output(5.0, "a")], 20.0, settings());

    assert_eq!(times(&rendered.recording.events), vec![1.0]);
    assert_eq!(rendered.recording.duration, 2.0);
}

#[test]
fn a_range_with_no_event_shrinks_and_the_later_events_move() {
    let region = FilterRegion::Range {
        start: Seconds::new(2.0).unwrap(),
        end: Seconds::new(12.0).unwrap(),
    };
    let filter = applied(region, settings());
    let project = project_of(
        vec![output(0.0, "a"), output(15.0, "b")],
        15.0,
        vec![filter],
    );

    let rendered = filters::registry().render_with_marks(&project).unwrap();

    // The range of 10 s becomes 1 s, so the event at 15 s moves by -9 s.
    assert_eq!(times(&rendered.recording.events), vec![0.0, 6.0]);
    assert_eq!(rendered.recording.duration, 6.0);
}

#[test]
fn a_range_replays_from_a_blank_screen_at_its_start_and_marks_its_periods() {
    let region = FilterRegion::Range {
        start: Seconds::new(10.0).unwrap(),
        end: Seconds::new(30.0).unwrap(),
    };
    let events = vec![
        output(0.0, "a"),
        // A redraw of the "a" from before the range. The replay starts from a
        // blank screen at 10 s, so this redraw is activity.
        output(12.0, "\x1b[1;1Ha"),
        output(22.0, "b"),
        // A cursor move inside the idle period from 22 s to the end at 30 s.
        output(26.0, "\x1b[H"),
        output(35.0, "c"),
    ];
    let project = project_of(events, 40.0, vec![applied(region, settings())]);

    let rendered = filters::registry().render_with_marks(&project).unwrap();

    // Relative to 10 s, the activity is at 2 and 12, and the range ends at
    // 20. The gap from 0 to 2 stays. The periods from 2 to 12 and from 12 to
    // 20 each become 1 s, so the range of 20 s becomes 4 s. The cursor move
    // at 16 is half way into its period of 8 s, and the event at 35 s moves
    // by -16 s.
    assert_eq!(
        times(&rendered.recording.events),
        vec![0.0, 12.0, 13.0, 13.5, 19.0]
    );
    assert_eq!(rendered.recording.duration, 24.0);
    assert_eq!(
        rendered.marks,
        vec![
            Mark {
                time: 12.0,
                label: "Idle time: 0m 10.0s compressed to 0m 1.0s".to_string()
            },
            Mark {
                time: 13.0,
                label: "Idle time: 0m 8.0s compressed to 0m 1.0s".to_string()
            },
        ]
    );
}

#[test]
fn events_out_of_time_order_replay_in_time_order() {
    let filter = applied(FilterRegion::All, settings());

    let out = filters::registry()
        .run(vec![output(10.0, "b"), output(0.0, "a")], &[filter])
        .unwrap();

    assert_eq!(times(&out), vec![0.0, 1.0]);
}

#[test]
fn a_bad_option_fails_the_render_and_names_the_kind() {
    let filter = options_without(THRESHOLD_OPTION);
    let project = project_of(vec![output(0.0, "a")], 5.0, vec![filter]);

    let result = filters::registry().render_with_marks(&project);

    match result {
        Err(NleError::RenderError(message)) => {
            assert!(message.contains(KIND), "{message}");
            assert!(message.contains(THRESHOLD_OPTION), "{message}");
        }
        other => panic!("expected a RenderError, got {other:?}"),
    }
}

// Tests of the marks.

#[test]
fn the_filter_marks_the_start_of_each_compressed_period() {
    let events = vec![output(0.0, "a"), output(10.0, "b"), output(20.0, "c")];

    let rendered = compressed(events, 20.0, settings());

    assert_eq!(
        rendered.marks,
        vec![
            Mark {
                time: 0.0,
                label: "Idle time: 0m 10.0s compressed to 0m 1.0s".to_string()
            },
            Mark {
                time: 1.0,
                label: "Idle time: 0m 10.0s compressed to 0m 1.0s".to_string()
            },
        ]
    );
}

#[test]
fn a_removed_period_has_a_mark_that_says_so() {
    let remove = IdleCompression::new(3.0, 0.0, 20, 5).unwrap();

    let rendered = compressed(vec![output(0.0, "a"), output(12.5, "b")], 12.5, remove);

    assert_eq!(
        rendered.marks,
        vec![Mark {
            time: 0.0,
            label: "Idle time: 0m 12.5s removed".to_string()
        }]
    );
}

#[test]
fn a_period_that_keeps_its_length_has_no_mark() {
    let same = IdleCompression::new(3.0, 3.0, 20, 5).unwrap();

    let rendered = compressed(vec![output(0.0, "a"), output(3.0, "b")], 3.0, same);

    assert_eq!(times(&rendered.recording.events), vec![0.0, 3.0]);
    assert!(rendered.marks.is_empty(), "{:?}", rendered.marks);
}

#[test]
fn a_compressed_length_equal_to_the_threshold_shortens_a_longer_period() {
    let same = IdleCompression::new(3.0, 3.0, 20, 5).unwrap();

    let rendered = compressed(vec![output(0.0, "a"), output(5.0, "b")], 5.0, same);

    assert_eq!(times(&rendered.recording.events), vec![0.0, 3.0]);
    assert_eq!(rendered.recording.duration, 3.0);
    assert_eq!(
        rendered.marks,
        vec![Mark {
            time: 0.0,
            label: "Idle time: 0m 5.0s compressed to 0m 3.0s".to_string()
        }]
    );
}

#[test]
fn an_earlier_mark_moves_with_the_time_that_it_is_in() {
    let mut registry = filters::registry();
    registry.register_span(
        FilterKind::new("mark"),
        |mut span: Span, _, _: &FilterOptions| {
            span.marks.push(Mark {
                time: 4.0,
                label: "earlier".to_string(),
            });
            span.marks.push(Mark {
                time: 12.0,
                label: "later".to_string(),
            });
            Ok(span)
        },
    );
    let mark = AppliedFilter {
        kind: FilterKind::new("mark"),
        region: FilterRegion::All,
        options: FilterOptions::new(),
    };
    let events = vec![output(0.0, "a"), output(10.0, "b"), output(12.0, "c")];
    let project = project_of(
        events,
        12.0,
        vec![mark, applied(FilterRegion::All, settings())],
    );

    let rendered = registry.render_with_marks(&project).unwrap();

    // The mark at 4 s is in the period of 10 s, which becomes 1 s. The mark
    // at 12 s comes after it, so it moves back by 9 s.
    assert_eq!(mark_times(&rendered.marks), vec![0.0, 0.4, 3.0]);
}

#[test]
fn the_shipped_registry_holds_idle_compression() {
    assert!(filters::registry().contains(&kind()));
}
