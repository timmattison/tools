//! Tests for the typing speed filter.
//!
//! Most tests use 4 characters per second, so each interval is 0.25 s, which
//! a float holds exactly.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::*;
use crate::filters;
use crate::types::{EditProject, Event, EventType, Recording};

fn key(time: f64, data: &str) -> Event {
    Event {
        time,
        event_type: EventType::Input,
        data: data.to_owned(),
    }
}

fn out(time: f64, data: &str) -> Event {
    Event {
        time,
        event_type: EventType::Output,
        data: data.to_owned(),
    }
}

fn times(events: &[Event]) -> Vec<f64> {
    events.iter().map(|event| event.time).collect()
}

fn range(start: f64, end: f64) -> FilterRegion {
    FilterRegion::Range {
        start: Seconds::new(start).unwrap(),
        end: Seconds::new(end).unwrap(),
    }
}

/// A normalization at `rate` characters per second, with a pause threshold
/// of 2 s and no jitter.
fn typing(rate: f64) -> Normalization {
    Normalization {
        rate: TypingRate::new(rate).unwrap(),
        pause: Seconds::new(2.0).unwrap(),
        jitter: Jitter::NONE,
    }
}

fn run(events: Vec<Event>, filters_list: &[AppliedFilter]) -> Vec<Event> {
    filters::registry().run(events, filters_list).unwrap()
}

#[test]
fn each_key_of_a_burst_comes_at_the_target_rate() {
    let input = vec![
        key(1.0, "a"),
        key(1.1, "é"),
        key(1.7, "🎉"),
        key(1.8, "日本"),
    ];

    let got = run(input, &[applied(FilterRegion::All, typing(4.0))]);

    assert_eq!(times(&got), vec![1.0, 1.25, 1.5, 1.75]);
    assert_eq!(data(&got), vec!["a", "é", "🎉", "日本"]);
    assert!(got.iter().all(|event| event.event_type == EventType::Input));
}

#[test]
fn the_rate_sets_the_interval() {
    // 2 and 8 characters per second give intervals of 0.5 s and 0.125 s,
    // which a float holds exactly.
    for (rate, expected) in [
        (2.0, vec![0.0, 0.5, 1.0, 1.5]),
        (8.0, vec![0.0, 0.125, 0.25, 0.375]),
    ] {
        let input = || vec![key(0.0, "a"), key(0.1, "b"), key(0.2, "c"), key(0.3, "d")];
        let from_options = with_options(FilterOptions::new().with(RATE_OPTION, rate));

        let through_applied = run(input(), &[applied(FilterRegion::All, typing(rate))]);
        let through_options = run(input(), &[from_options]);

        assert_eq!(times(&through_applied), expected, "applied at {rate}");
        assert_eq!(times(&through_options), expected, "options at {rate}");
    }
}

#[test]
fn the_events_after_the_last_key_move_by_its_change() {
    let input = vec![
        key(10.0, "l"),
        key(11.0, "s"),
        out(12.0, "Cargo.toml\r\n"),
        out(25.0, "after the region"),
    ];

    let got = run(input, &[applied(range(10.0, 20.0), typing(4.0))]);

    // The second key moves from 11 s to 10.25 s, so each later event moves
    // 0.75 s earlier, also the event after the region.
    assert_eq!(times(&got), vec![10.0, 10.25, 11.25, 24.25]);
}

#[test]
fn a_render_moves_the_duration_by_the_change_of_the_last_key() {
    let recording = Recording {
        version: 2,
        width: 80,
        height: 24,
        timestamp: 0.0,
        duration: 10.0,
        command: String::new(),
        title: String::new(),
        env: BTreeMap::new(),
        events: vec![key(1.0, "a"), key(2.0, "b"), out(5.0, "x")],
    };
    let mut project = EditProject::from_recording(Arc::new(recording)).unwrap();
    project
        .filters
        .push(applied(FilterRegion::All, typing(4.0)));

    let rendered = filters::registry().render(&project).unwrap();

    assert_eq!(times(&rendered.events), vec![1.0, 1.25, 4.25]);
    assert_eq!(rendered.duration, 9.25);
}

fn data(events: &[Event]) -> Vec<&str> {
    events.iter().map(|event| event.data.as_str()).collect()
}

#[test]
fn an_output_between_two_keys_shrinks_with_their_gap() {
    let input = vec![
        key(0.0, "a"),
        out(0.25, "first"),
        out(0.5, "second"),
        key(1.0, "b"),
    ];

    let got = run(input, &[applied(FilterRegion::All, typing(4.0))]);

    // The gap of 1 s becomes 0.25 s, so each delay after "a" is a quarter.
    assert_eq!(times(&got), vec![0.0, 0.0625, 0.125, 0.25]);
    assert_eq!(data(&got), vec!["a", "first", "second", "b"]);
}

#[test]
fn an_output_between_two_keys_keeps_its_delay_when_their_gap_grows() {
    let input = vec![key(0.0, "a"), out(0.0625, "a"), key(0.125, "b")];

    let got = run(input, &[applied(FilterRegion::All, typing(4.0))]);

    // The echo of "a" comes 0.0625 s after it, as in the recording.
    assert_eq!(times(&got), vec![0.0, 0.0625, 0.25]);
}

#[test]
fn a_gap_longer_than_the_pause_threshold_stays() {
    let input = vec![
        key(0.0, "a"),
        key(0.5, "b"),
        out(1.0, "thinking"),
        key(3.0, "c"),
        key(3.5, "d"),
    ];

    let got = run(input, &[applied(FilterRegion::All, typing(4.0))]);

    // The gap of 2.5 s from "b" to "c" is a pause, so it stays 2.5 s long.
    // Only the two bursts around it become faster.
    assert_eq!(times(&got), vec![0.0, 0.25, 0.75, 2.75, 3.0]);
}

#[test]
fn a_gap_equal_to_the_pause_threshold_is_typing() {
    let input = vec![key(0.0, "a"), key(2.0, "b")];

    let got = run(input, &[applied(FilterRegion::All, typing(4.0))]);

    assert_eq!(times(&got), vec![0.0, 0.25]);
}

#[test]
fn the_gap_after_a_line_end_stays() {
    for enter in ["\r", "\n", "s\r"] {
        let input = vec![
            key(0.0, "l"),
            key(0.5, "s"),
            key(1.0, enter),
            out(1.0625, "Cargo.toml\r\n"),
            key(1.5, "c"),
            key(1.75, "d"),
        ];

        let got = run(input, &[applied(FilterRegion::All, typing(4.0))]);

        // The gap of 0.5 s from the line end to "c" is shorter than the
        // pause threshold, but it is the gap between two commands.
        assert_eq!(
            times(&got),
            vec![0.0, 0.25, 0.5, 0.5625, 1.0, 1.25],
            "line end {enter:?}"
        );
    }
}

#[test]
fn a_jitter_changes_each_interval_inside_its_bound_and_the_same_way_each_time() {
    let input: Vec<Event> = (0..21_u32)
        .map(|index| key(f64::from(index) * 0.5, "x"))
        .collect();
    let jittery = Normalization {
        jitter: Jitter::new(0.5).unwrap(),
        ..typing(4.0)
    };
    let filter = [applied(FilterRegion::All, jittery)];

    let first = times(&run(input.clone(), &filter));
    let second = times(&run(input, &filter));

    assert_eq!(first, second, "a render must give the same times each time");
    let intervals: Vec<f64> = first.windows(2).map(|pair| pair[1] - pair[0]).collect();
    // A jitter of 0.5 at 4 characters per second gives intervals from
    // 0.125 s up to, and not including, 0.375 s. The difference of two times
    // can round, so each bound has a small tolerance.
    for interval in &intervals {
        assert!(
            (0.125 - 1e-12..0.375 + 1e-12).contains(interval),
            "interval {interval} is outside the bound: {intervals:?}"
        );
    }
    let smallest = intervals.iter().copied().fold(f64::INFINITY, f64::min);
    let largest = intervals.iter().copied().fold(0.0, f64::max);
    assert!(
        largest - smallest > 0.05,
        "the intervals do not vary: {intervals:?}"
    );
}

#[test]
fn a_typing_rate_must_be_finite_and_above_zero() {
    for good in [0.5, 4.0, 12.0] {
        assert_eq!(TypingRate::new(good).unwrap().get(), good);
    }
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let error = TypingRate::new(bad).expect_err("a bad rate must be refused");
        assert!(error.to_string().contains("typing rate"), "{bad}: {error}");
    }
}

#[test]
fn a_jitter_must_be_from_zero_up_to_one() {
    for good in [0.0, 0.25, 0.99] {
        assert_eq!(Jitter::new(good).unwrap().get(), good);
    }
    for bad in [-0.1, 1.0, 1.5, f64::NAN, f64::INFINITY] {
        let error = Jitter::new(bad).expect_err("a bad jitter must be refused");
        assert!(error.to_string().contains("jitter"), "{bad}: {error}");
    }
}

fn with_options(options: FilterOptions) -> AppliedFilter {
    AppliedFilter {
        kind: kind(),
        region: FilterRegion::All,
        options,
    }
}

#[test]
fn a_filter_with_only_a_rate_has_a_pause_of_two_seconds_and_no_jitter() {
    let input = vec![key(0.0, "a"), key(1.9, "b"), key(4.5, "c")];
    let filter = with_options(FilterOptions::new().with(RATE_OPTION, 4.0));

    let got = run(input, &[filter]);

    // 1.9 s is typing, and 2.6 s is a pause.
    assert_eq!(times(&got), vec![0.0, 0.25, 2.85]);
}

#[test]
fn the_pause_threshold_sets_where_a_burst_ends() {
    // At the default threshold of 2 s, the gap of 1.5 s is typing and the gap
    // of 2.5 s is a pause, which gives [0.0, 0.25, 2.75].
    let input = || vec![key(0.0, "a"), key(1.5, "b"), key(4.0, "c")];
    for (pause, expected) in [(1.0, vec![0.0, 1.5, 4.0]), (3.0, vec![0.0, 0.25, 0.5])] {
        let normalization = Normalization {
            pause: Seconds::new(pause).unwrap(),
            ..typing(4.0)
        };
        let options = FilterOptions::new()
            .with(RATE_OPTION, 4.0)
            .with(PAUSE_OPTION, pause);

        let through_applied = run(input(), &[applied(FilterRegion::All, normalization)]);
        let through_options = run(input(), &[with_options(options)]);

        assert_eq!(times(&through_applied), expected, "applied at {pause}");
        assert_eq!(times(&through_options), expected, "options at {pause}");
    }
}

#[test]
fn a_bad_option_fails_the_render_with_a_message_that_names_it() {
    let rate = || FilterOptions::new().with(RATE_OPTION, 4.0);
    let cases = [
        (
            FilterOptions::new(),
            "the option \"chars_per_second\" is missing or is not a number",
        ),
        (
            FilterOptions::new().with(RATE_OPTION, "fast"),
            "the option \"chars_per_second\" is missing or is not a number",
        ),
        (
            FilterOptions::new().with(RATE_OPTION, 0.0),
            "the option \"chars_per_second\" is not valid: Invalid typing rate: 0",
        ),
        (
            rate().with(PAUSE_OPTION, -1.0),
            "the option \"pause\" is not valid: Invalid time: -1",
        ),
        (
            rate().with(PAUSE_OPTION, "long"),
            "the option \"pause\" is not a number",
        ),
        (
            rate().with(JITTER_OPTION, 1.5),
            "the option \"jitter\" is not valid: Invalid jitter: 1.5",
        ),
        (
            rate().with(JITTER_OPTION, "some"),
            "the option \"jitter\" is not a number",
        ),
    ];
    for (options, expected) in cases {
        let error = filters::registry()
            .run(vec![key(0.0, "a"), key(1.0, "b")], &[with_options(options)])
            .expect_err("a bad option must fail the render");
        let message = error.to_string();
        assert!(
            message.contains("filter \"typing-speed\" failed") && message.contains(expected),
            "expected {expected:?} in {message:?}"
        );
    }
}

#[test]
fn a_burst_typed_faster_than_the_rate_becomes_longer() {
    let input = vec![
        key(10.0, "a"),
        key(10.0625, "b"),
        key(10.125, "c"),
        out(10.25, "abc"),
        out(15.0, "after the region"),
    ];

    let got = run(input, &[applied(range(10.0, 12.0), typing(4.0))]);

    // "c" moves 0.375 s later, so each later event moves 0.375 s later too.
    assert_eq!(times(&got), vec![10.0, 10.25, 10.5, 10.625, 15.375]);
}

#[test]
fn a_region_with_no_key_changes_nothing() {
    let input = vec![out(1.0, "build"), out(2.0, "done")];

    let got = run(input, &[applied(FilterRegion::All, typing(4.0))]);

    assert_eq!(times(&got), vec![1.0, 2.0]);
}

#[test]
fn an_event_before_the_first_key_keeps_its_time() {
    let input = vec![out(0.5, "$ "), key(1.0, "a"), key(1.5, "b")];

    let got = run(input, &[applied(FilterRegion::All, typing(4.0))]);

    assert_eq!(times(&got), vec![0.5, 1.0, 1.25]);
}

#[test]
fn a_jitter_does_not_change_a_pause() {
    let input = vec![key(0.0, "a"), key(0.5, "b"), key(3.0, "c")];
    let jittery = Normalization {
        jitter: Jitter::new(0.5).unwrap(),
        ..typing(4.0)
    };

    let got = run(input, &[applied(FilterRegion::All, jittery)]);

    assert_eq!(got[0].time, 0.0);
    assert_ne!(got[1].time, 0.25, "the jitter must change the interval");
    assert_eq!(got[2].time, got[1].time + 2.5, "the pause must stay 2.5 s");
}

#[test]
fn a_mark_that_idle_compression_puts_inside_the_region_moves_with_the_keys() {
    use crate::filters::idle_compression::{self, IdleCompression};
    // Each output writes a character, so each event is activity. Idle
    // compression shortens each gap of 3 s to 0.5 s, and marks its start.
    let recording = Recording {
        version: 2,
        width: 80,
        height: 24,
        timestamp: 0.0,
        duration: 10.5,
        command: String::new(),
        title: String::new(),
        env: BTreeMap::new(),
        events: vec![
            out(0.5, "$"),
            key(3.5, "a"),
            out(4.0, "a"),
            key(7.0, "b"),
            out(7.5, "b"),
        ],
    };
    let mut project = EditProject::from_recording(Arc::new(recording)).unwrap();
    let compression = idle_compression::applied(
        FilterRegion::All,
        IdleCompression::new(1.0, 0.5, 80, 24).unwrap(),
    );
    project.filters = vec![compression];
    let compressed = filters::registry().render_with_marks(&project).unwrap();
    // After idle compression, the keys are at 1 s and 2 s, and the marks are
    // at 0.5 s, 1.5 s, and 2.5 s.
    assert_eq!(
        times(&compressed.recording.events),
        vec![0.5, 1.0, 1.5, 2.0, 2.5]
    );
    assert_eq!(mark_times(&compressed.marks), vec![0.5, 1.5, 2.5]);
    project
        .filters
        .push(applied(FilterRegion::All, typing(4.0)));

    let rendered = filters::registry()
        .render_with_marks(&project)
        .expect("the render must not fail");

    // The gap of 1 s between the keys becomes 0.25 s. The mark before the
    // first key keeps its time. The mark between the keys shrinks with their
    // gap. The mark after the last key moves by the change of that key, as
    // the events do.
    assert_eq!(mark_times(&rendered.marks), vec![0.5, 1.125, 1.75]);
    assert_eq!(
        times(&rendered.recording.events),
        vec![0.5, 1.0, 1.125, 1.25, 1.75]
    );
    assert_eq!(rendered.recording.duration, 2.25);
    assert_eq!(mark_labels(&rendered.marks), mark_labels(&compressed.marks));
}

fn mark_times(marks: &[crate::pipeline::Mark]) -> Vec<f64> {
    marks.iter().map(|mark| mark.time).collect()
}

fn mark_labels(marks: &[crate::pipeline::Mark]) -> Vec<&str> {
    marks.iter().map(|mark| mark.label.as_str()).collect()
}

#[test]
fn a_mark_at_the_time_of_a_key_moves_with_that_key() {
    use crate::pipeline::{Mark, Span};
    let mut registry = filters::registry();
    registry.register_span(
        FilterKind::new("mark"),
        |mut span: Span, _, _: &FilterOptions| {
            span.marks.push(Mark {
                time: 0.125,
                label: "m".to_string(),
            });
            Ok(span)
        },
    );
    let recording = Recording {
        version: 2,
        width: 80,
        height: 24,
        timestamp: 0.0,
        duration: 1.0,
        command: String::new(),
        title: String::new(),
        env: BTreeMap::new(),
        events: vec![key(0.0, "a"), key(0.125, "b")],
    };
    let mut project = EditProject::from_recording(Arc::new(recording)).unwrap();
    project.filters = vec![
        AppliedFilter {
            kind: FilterKind::new("mark"),
            region: FilterRegion::All,
            options: FilterOptions::new(),
        },
        applied(FilterRegion::All, typing(4.0)),
    ];

    let rendered = registry.render_with_marks(&project).unwrap();

    // "b" was typed faster than the rate, so it moves from 0.125 s to 0.25 s.
    // The mark at its time moves with it, and does not stay between the keys.
    assert_eq!(times(&rendered.recording.events), vec![0.0, 0.25]);
    assert_eq!(mark_times(&rendered.marks), vec![0.25]);
}
