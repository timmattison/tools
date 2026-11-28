//! Tests of the registry of the filters that ship with the app. The options
//! are written with the literal keys of the project file, because those keys
//! are a file format.

use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::error::NleError;
use crate::pipeline::Mark;
use crate::types::{
    AppliedFilter, EditProject, Event, EventType, FilterKind, FilterOptions, FilterRegion,
    Recording, Seconds,
};

fn output(time: f64) -> Event {
    Event {
        time,
        event_type: EventType::Output,
        data: format!("o{time}"),
    }
}

fn input(time: f64) -> Event {
    Event {
        time,
        event_type: EventType::Input,
        data: format!("i{time}"),
    }
}

fn outputs(times: &[f64]) -> Vec<Event> {
    times.iter().copied().map(output).collect()
}

fn times(events: &[Event]) -> Vec<f64> {
    events.iter().map(|e| e.time).collect()
}

fn all(kind: &str, options: FilterOptions) -> AppliedFilter {
    AppliedFilter {
        kind: FilterKind::new(kind),
        region: FilterRegion::All,
        options,
    }
}

fn range(kind: &str, start: f64, end: f64, options: FilterOptions) -> AppliedFilter {
    AppliedFilter {
        kind: FilterKind::new(kind),
        region: FilterRegion::Range {
            start: Seconds::new(start).unwrap(),
            end: Seconds::new(end).unwrap(),
        },
        options,
    }
}

fn run(events: Vec<Event>, filter: AppliedFilter) -> Result<Vec<Event>, NleError> {
    registry().run(events, &[filter])
}

/// Renders a project of one recording that holds `events` and ends at
/// `duration`, with `filter` applied.
fn render(events: Vec<Event>, duration: f64, filter: AppliedFilter) -> Recording {
    try_render(events, duration, filter).unwrap()
}

/// The result of `render`, which can be a render error.
fn try_render(
    events: Vec<Event>,
    duration: f64,
    filter: AppliedFilter,
) -> Result<Recording, NleError> {
    registry().render(&project(events, duration, vec![filter]))
}

/// A project of one recording that holds `events` and ends at `duration`,
/// with `filters` applied in order.
fn project(events: Vec<Event>, duration: f64, filters: Vec<AppliedFilter>) -> EditProject {
    let source = Recording {
        version: 2,
        width: 80,
        height: 24,
        timestamp: 1_700_000_000.0,
        duration,
        command: "zsh".to_string(),
        title: "filters".to_string(),
        env: std::collections::BTreeMap::new(),
        events,
    };
    let mut project = EditProject::from_recording(Arc::new(source)).unwrap();
    project.filters = filters;
    project
}

/// Asserts that `result` is a render error that names the kind of `filter`,
/// says that the options are invalid, and holds `reason`.
fn assert_invalid_options(result: Result<Vec<Event>, NleError>, kind: &str, reason: &str) {
    match result {
        Err(NleError::RenderError(message)) => {
            assert!(message.contains(&format!("\"{kind}\"")), "{message}");
            assert!(message.contains("invalid options"), "{message}");
            assert!(message.contains(reason), "{reason}: {message}");
        }
        other => panic!("expected a RenderError, got {other:?}"),
    }
}

// Tests of pause removal.

fn removal(threshold: f64, max_pause: f64) -> FilterOptions {
    FilterOptions::new()
        .with("threshold", threshold)
        .with("max_pause", max_pause)
}

#[test]
fn pause_removal_shortens_each_gap_above_the_threshold_to_the_max_pause() {
    let events = vec![
        output(0.0),
        input(1.0),
        output(5.0),
        input(5.5),
        output(10.0),
    ];

    let out = run(events.clone(), all(PAUSE_REMOVAL, removal(2.0, 1.0))).unwrap();

    // The gaps 1 and 0.5 stay. The gaps 4 and 4.5 become 1.
    assert_eq!(times(&out), vec![0.0, 1.0, 2.0, 2.5, 3.5]);
    for (before, after) in events.iter().zip(&out) {
        assert_eq!(before.event_type, after.event_type);
        assert_eq!(before.data, after.data);
    }
}

#[test]
fn pause_removal_keeps_a_gap_equal_to_the_threshold() {
    let out = run(outputs(&[0.0, 2.0]), all(PAUSE_REMOVAL, removal(2.0, 1.0))).unwrap();

    assert_eq!(times(&out), vec![0.0, 2.0]);
}

#[test]
fn pause_removal_accepts_a_max_pause_equal_to_the_threshold_and_caps_each_pause_at_it() {
    let out = run(outputs(&[0.0, 5.0]), all(PAUSE_REMOVAL, removal(2.0, 2.0))).unwrap();

    assert_eq!(times(&out), vec![0.0, 2.0]);
}

#[test]
fn pause_removal_keeps_the_order_of_events_at_one_time() {
    let events = vec![output(0.0), output(5.0), input(5.0), output(5.0)];

    let out = run(events.clone(), all(PAUSE_REMOVAL, removal(2.0, 1.0))).unwrap();

    assert_eq!(times(&out), vec![0.0, 1.0, 1.0, 1.0]);
    assert_eq!(out, {
        let mut moved = events;
        for event in moved.iter_mut().skip(1) {
            event.time = 1.0;
        }
        moved
    });
}

#[test]
fn pause_removal_measures_the_gaps_in_time_order() {
    // The pipeline gives a filter the events in the order of its input.
    let out = run(
        outputs(&[5.0, 0.0, 1.0]),
        all(PAUSE_REMOVAL, removal(2.0, 1.0)),
    )
    .unwrap();

    assert_eq!(times(&out), vec![0.0, 1.0, 2.0]);
}

#[test]
fn pause_removal_moves_the_end_by_the_removed_time_and_keeps_the_time_after_the_last_event() {
    // The time from the last event at 5 to the end at 7 is not a gap between
    // two events, so it stays.
    let rendered = render(
        outputs(&[0.0, 1.0, 5.0]),
        7.0,
        all(PAUSE_REMOVAL, removal(2.0, 1.0)),
    );

    assert_eq!(times(&rendered.events), vec![0.0, 1.0, 2.0]);
    assert_eq!(rendered.duration, 4.0);
}

#[test]
fn pause_removal_on_a_range_changes_only_the_gaps_inside_the_range() {
    let out = run(
        outputs(&[0.0, 4.0, 8.0, 12.0, 20.0]),
        range(PAUSE_REMOVAL, 3.0, 13.0, removal(2.0, 1.0)),
    )
    .unwrap();

    // Inside the range, 8 and 12 each lose 3. The gap from 0 to 4 crosses
    // the start of the range, and the gap from 12 to 20 crosses its end, so
    // both stay.
    assert_eq!(times(&out), vec![0.0, 4.0, 5.0, 6.0, 14.0]);
}

#[test]
fn pause_removal_refuses_invalid_options() {
    let cases = [
        (FilterOptions::new().with("max_pause", 1.0), "threshold"),
        (FilterOptions::new().with("threshold", 2.0), "max_pause"),
        (removal(-1.0, 0.0), "below zero"),
        (
            FilterOptions::new()
                .with("threshold", 2.0)
                .with("max_pause", "soon"),
            "invalid type",
        ),
        (removal(1.0, 2.0), "greater than"),
        (removal(2.0, 1.0).with("treshold", 3.0), "treshold"),
    ];
    for (options, reason) in cases {
        let result = run(outputs(&[0.0, 5.0]), all(PAUSE_REMOVAL, options));

        assert_invalid_options(result, PAUSE_REMOVAL, reason);
    }
}

// Tests of pause insertion.

fn insertion(at: f64, duration: f64) -> FilterOptions {
    FilterOptions::new()
        .with("at", at)
        .with("duration", duration)
}

#[test]
fn pause_insertion_moves_each_event_at_or_after_the_point_later_by_the_duration() {
    let events = vec![output(0.0), input(1.0), input(2.0), output(3.0)];

    let out = run(events.clone(), all(PAUSE_INSERTION, insertion(2.0, 1.5))).unwrap();

    // The event at the point moves too, so a pause can come before a key.
    assert_eq!(times(&out), vec![0.0, 1.0, 3.5, 4.5]);
    for (before, after) in events.iter().zip(&out) {
        assert_eq!(before.event_type, after.event_type);
        assert_eq!(before.data, after.data);
    }
}

#[test]
fn pause_insertion_makes_the_timeline_longer_by_the_duration() {
    let rendered = render(
        outputs(&[0.0, 2.0]),
        3.0,
        all(PAUSE_INSERTION, insertion(1.0, 2.0)),
    );

    assert_eq!(times(&rendered.events), vec![0.0, 4.0]);
    assert_eq!(rendered.duration, 5.0);
}

#[test]
fn pause_insertion_after_the_last_event_still_makes_the_timeline_longer() {
    // No event moves, but the end at 3 comes after the point at 2.5.
    let rendered = render(
        outputs(&[0.0, 2.0]),
        3.0,
        all(PAUSE_INSERTION, insertion(2.5, 1.0)),
    );

    assert_eq!(times(&rendered.events), vec![0.0, 2.0]);
    assert_eq!(rendered.duration, 4.0);
}

#[test]
fn pause_insertion_refuses_a_point_past_the_end_of_the_timeline() {
    // The region `All` ends at the end of the timeline, 3. A pause there
    // would make the timeline longer, but the end at 3 is before the point.
    let result = try_render(
        outputs(&[0.0, 2.0]),
        3.0,
        all(PAUSE_INSERTION, insertion(5.0, 1.0)),
    );

    assert_invalid_options(
        result.map(|rendered| rendered.events),
        PAUSE_INSERTION,
        "past the end of the region",
    );

    // The point 3 is the end of the timeline, so the timeline gets longer.
    let rendered = render(
        outputs(&[0.0, 2.0]),
        3.0,
        all(PAUSE_INSERTION, insertion(3.0, 1.0)),
    );

    assert_eq!(times(&rendered.events), vec![0.0, 2.0]);
    assert_eq!(rendered.duration, 4.0);
}

#[test]
fn pause_insertion_on_a_range_takes_the_point_from_the_start_of_the_range() {
    // The point 2 in the range from 3 is 5 on the timeline. The event at 6
    // moves to 7, and the event after the range moves by the same shift.
    let out = run(
        outputs(&[0.0, 4.0, 6.0, 10.0]),
        range(PAUSE_INSERTION, 3.0, 8.0, insertion(2.0, 1.0)),
    )
    .unwrap();

    assert_eq!(times(&out), vec![0.0, 4.0, 7.0, 11.0]);
}

#[test]
fn pause_insertion_on_a_range_moves_what_is_after_the_range_also_when_no_event_of_the_range_moves()
{
    // The point 2 in the range from 3 is 5 on the timeline, after the event at
    // 4, which is the last event of the range. The event at 10 and the end of
    // the timeline at 10 are after the range, and each moves later by 1.
    let filter = || range(PAUSE_INSERTION, 3.0, 8.0, insertion(2.0, 1.0));
    let rendered = render(outputs(&[0.0, 4.0, 10.0]), 10.0, filter());

    assert_eq!(times(&rendered.events), vec![0.0, 4.0, 11.0]);
    assert_eq!(rendered.duration, 11.0);

    // With no event after the range, the end of the timeline still moves.
    let rendered = render(outputs(&[0.0, 4.0]), 10.0, filter());

    assert_eq!(times(&rendered.events), vec![0.0, 4.0]);
    assert_eq!(rendered.duration, 11.0);
}

#[test]
fn pause_insertion_refuses_a_point_past_the_end_of_its_range() {
    // The range from 3 to 8 is 5 long, so the point 20 is 23 on the timeline.
    // A pause there must move the event at 25 and not the event at 9. The
    // events after the range all move by one shift, so the point is an error.
    let events = || outputs(&[0.0, 4.0, 9.0, 25.0]);
    let result = run(
        events(),
        range(PAUSE_INSERTION, 3.0, 8.0, insertion(20.0, 1.0)),
    );

    assert_invalid_options(result, PAUSE_INSERTION, "past the end of the region");

    // The point 5 is the end of the range, 8 on the timeline, and each event
    // after the range is after it.
    let out = run(
        events(),
        range(PAUSE_INSERTION, 3.0, 8.0, insertion(5.0, 1.0)),
    )
    .unwrap();

    assert_eq!(times(&out), vec![0.0, 4.0, 10.0, 26.0]);
}

#[test]
fn pause_insertion_accepts_the_end_of_a_range_with_decimal_bounds() {
    // The range from 1.1 to 1.3 has the length 1.3 - 1.1, which is
    // 0.19999999999999996 and not 0.2. The point 0.2 that a person types is
    // the end of the range, so the pause goes there, and the event at 2 after
    // the range moves later by the duration.
    let events = || outputs(&[0.0, 1.1, 2.0]);
    let out = run(
        events(),
        range(PAUSE_INSERTION, 1.1, 1.3, insertion(0.2, 1.0)),
    )
    .unwrap();

    assert_eq!(times(&out), vec![0.0, 1.1, 3.0]);

    // A point that is past the end by more than a rounding error is still
    // an error.
    let result = run(
        events(),
        range(PAUSE_INSERTION, 1.1, 1.3, insertion(0.21, 1.0)),
    );

    assert_invalid_options(result, PAUSE_INSERTION, "past the end of the region");
}

#[test]
fn pause_insertion_at_a_point_inside_the_tolerance_moves_the_event_at_the_end() {
    // The last event, and thus the end of the timeline, is one rounding step
    // before the point 0.2. The point is the end, so the event at the end
    // moves, the same as for a point that is exactly the end.
    let end = 1.3 - 1.1;
    let out = run(
        outputs(&[0.0, end]),
        all(PAUSE_INSERTION, insertion(0.2, 1.0)),
    )
    .unwrap();
    let exact = run(
        outputs(&[0.0, end]),
        all(PAUSE_INSERTION, insertion(end, 1.0)),
    )
    .unwrap();

    assert_eq!(times(&out), vec![0.0, end + 1.0]);
    assert_eq!(times(&out), times(&exact));
}

#[test]
fn pause_insertion_refuses_invalid_options() {
    let cases = [
        (FilterOptions::new().with("duration", 1.0), "at"),
        (FilterOptions::new().with("at", 1.0), "duration"),
        (insertion(1.0, -1.0), "below zero"),
        (insertion(-1.0, 1.0), "below zero"),
        (insertion(1.0, 1.0).with("length", 1.0), "length"),
    ];
    for (options, reason) in cases {
        let result = run(outputs(&[0.0, 5.0]), all(PAUSE_INSERTION, options));

        assert_invalid_options(result, PAUSE_INSERTION, reason);
    }
}

// Tests of auto-trim.

fn trim(leading: bool, trailing: bool) -> FilterOptions {
    FilterOptions::new()
        .with("leading", leading)
        .with("trailing", trailing)
}

fn data(events: &[Event]) -> Vec<&str> {
    events.iter().map(|e| e.data.as_str()).collect()
}

#[test]
fn auto_trim_removes_the_time_before_the_first_input_event() {
    let events = vec![
        output(0.5),
        output(2.0),
        input(5.0),
        output(5.25),
        input(6.0),
        output(6.5),
    ];

    let rendered = render(events, 6.5, all(AUTO_TRIM, trim(true, false)));

    // The output before the first input moves to the start, in its order,
    // so the screen that the user saw before the first key is still there.
    assert_eq!(times(&rendered.events), vec![0.0, 0.0, 0.0, 0.25, 1.0, 1.5]);
    assert_eq!(
        data(&rendered.events),
        vec!["o0.5", "o2", "i5", "o5.25", "i6", "o6.5"]
    );
    assert_eq!(rendered.duration, 1.5);
}

#[test]
fn auto_trim_removes_the_time_after_the_last_output_event() {
    let events = vec![output(0.0), input(1.0), output(2.0), input(3.0), input(4.0)];

    let rendered = render(events, 5.0, all(AUTO_TRIM, trim(false, true)));

    // The input after the last output moves to the last output, in its order.
    assert_eq!(times(&rendered.events), vec![0.0, 1.0, 2.0, 2.0, 2.0]);
    assert_eq!(data(&rendered.events), vec!["o0", "i1", "o2", "i3", "i4"]);
    assert_eq!(rendered.duration, 2.0);
}

#[test]
fn auto_trim_removes_the_time_after_the_last_event_too() {
    // The time from the last event at 2 to the end at 5 is after the last
    // output, so it goes.
    let rendered = render(outputs(&[0.0, 2.0]), 5.0, all(AUTO_TRIM, trim(false, true)));

    assert_eq!(times(&rendered.events), vec![0.0, 2.0]);
    assert_eq!(rendered.duration, 2.0);
}

#[test]
fn auto_trim_trims_both_ends_when_the_options_are_empty() {
    let events = vec![output(0.5), input(2.0), output(3.0), input(4.0)];

    let rendered = render(events, 6.0, all(AUTO_TRIM, FilterOptions::new()));

    assert_eq!(times(&rendered.events), vec![0.0, 0.0, 1.0, 1.0]);
    assert_eq!(rendered.duration, 1.0);
}

#[test]
fn auto_trim_keeps_the_start_with_no_input_and_the_end_with_no_output() {
    let rendered = render(
        outputs(&[1.0, 2.0]),
        4.0,
        all(AUTO_TRIM, FilterOptions::new()),
    );
    assert_eq!(times(&rendered.events), vec![1.0, 2.0]);
    assert_eq!(rendered.duration, 2.0);

    let rendered = render(
        vec![input(1.0), input(2.0)],
        4.0,
        all(AUTO_TRIM, FilterOptions::new()),
    );
    assert_eq!(times(&rendered.events), vec![0.0, 1.0]);
    assert_eq!(rendered.duration, 3.0);
}

#[test]
fn auto_trim_with_both_ends_off_changes_nothing() {
    let events = vec![output(1.0), input(2.0)];

    let rendered = render(events.clone(), 3.0, all(AUTO_TRIM, trim(false, false)));

    assert_eq!(rendered.events, events);
    assert_eq!(rendered.duration, 3.0);
}

#[test]
fn auto_trim_on_a_range_trims_the_ends_of_the_range() {
    let events = vec![
        output(0.0),
        output(4.0),
        input(6.0),
        output(7.0),
        input(9.0),
        output(12.0),
    ];

    let out = run(events, range(AUTO_TRIM, 3.0, 10.0, FilterOptions::new())).unwrap();

    // In the range, the start of the range comes up to the input at 6, and the
    // input at 9 and the end of the range at 10 come back to the output at 7.
    // The event at 12 after the range keeps its gap of 2 from the end of the
    // range, which is now at 4.
    assert_eq!(times(&out), vec![0.0, 3.0, 3.0, 4.0, 4.0, 6.0]);
}

#[test]
fn auto_trim_refuses_invalid_options() {
    let cases = [
        (FilterOptions::new().with("leading", "yes"), "invalid type"),
        (FilterOptions::new().with("trim", true), "trim"),
    ];
    for (options, reason) in cases {
        let result = run(outputs(&[0.0, 5.0]), all(AUTO_TRIM, options));

        assert_invalid_options(result, AUTO_TRIM, reason);
    }
}

// Tests of the marks of an earlier filter.

/// The kind of a test span filter that puts marks into its region.
const MARKER: &str = "marker";

/// The app registry, with a span filter under `MARKER` that puts a mark at
/// each of `times`, relative to the start of its region.
fn marking(times: &[f64]) -> FilterRegistry {
    let times = times.to_vec();
    let mut registry = registry();
    registry.register_span(
        FilterKind::new(MARKER),
        move |mut span: Span, _, _: &FilterOptions| {
            span.marks.extend(times.iter().map(|&time| Mark {
                time,
                label: format!("m{time}"),
            }));
            Ok(span)
        },
    );
    registry
}

/// The mark times of the render of `project(events, duration, ...)`: the
/// marker puts a mark at each of `marks` on the whole timeline, then `filter`
/// runs.
fn moved_marks(
    marks: &[f64],
    events: Vec<Event>,
    duration: f64,
    filter: AppliedFilter,
) -> Vec<f64> {
    let filters = vec![all(MARKER, FilterOptions::new()), filter];
    marking(marks)
        .render_with_marks(&project(events, duration, filters))
        .expect("the render must not fail")
        .marks
        .iter()
        .map(|mark| mark.time)
        .collect()
}

#[test]
fn pause_removal_moves_a_mark_inside_a_shortened_gap_in_proportion() {
    // The gap from 1 s to 5 s becomes 1 s long. A mark in a gap that stays
    // keeps its time, a mark in the shortened gap shrinks with it, and a mark
    // at the time of an event moves with that event.
    let marks = moved_marks(
        &[0.5, 3.0, 5.0],
        outputs(&[0.0, 1.0, 5.0, 6.0]),
        8.0,
        all(PAUSE_REMOVAL, removal(2.0, 1.0)),
    );

    assert_eq!(marks, vec![0.5, 1.5, 2.0]);
}

#[test]
fn pause_removal_moves_a_mark_after_the_last_event_back_by_the_removed_time() {
    // The gap from 1 s to 5 s loses 3 s, so the time after the last event
    // and the mark in it move back by 3 s.
    let marks = moved_marks(
        &[7.0],
        outputs(&[0.0, 1.0, 5.0, 6.0]),
        8.0,
        all(PAUSE_REMOVAL, removal(2.0, 1.0)),
    );

    assert_eq!(marks, vec![4.0]);
}

#[test]
fn pause_removal_keeps_the_time_of_a_mark_before_the_first_event() {
    // The first event at 2 s keeps its time, so the mark at 1 s before it
    // keeps its time too. The gap from 3 s to 8 s becomes 1 s long, so the
    // mark at 5.5 s goes to 3.5 s.
    let marks = moved_marks(
        &[1.0, 5.5],
        outputs(&[2.0, 3.0, 8.0]),
        10.0,
        all(PAUSE_REMOVAL, removal(2.0, 1.0)),
    );

    assert_eq!(marks, vec![1.0, 3.5]);
}

#[test]
fn pause_insertion_moves_each_mark_at_or_after_the_point_later_by_the_duration() {
    let marks = moved_marks(
        &[1.0, 2.0, 4.5],
        outputs(&[0.0, 2.0, 4.0]),
        5.0,
        all(PAUSE_INSERTION, insertion(2.0, 3.0)),
    );

    assert_eq!(marks, vec![1.0, 5.0, 7.5]);
}

#[test]
fn auto_trim_moves_the_marks_back_with_the_cut_at_the_start() {
    // The first input event is at 2 s, so each time moves back by 2 s, and a
    // mark before it moves to the start, as an output event does.
    let events = vec![output(1.0), input(2.0), output(3.0), output(5.0)];

    let marks = moved_marks(&[1.0, 2.5], events, 8.0, all(AUTO_TRIM, trim(true, false)));

    assert_eq!(marks, vec![0.0, 0.5]);
}

#[test]
fn auto_trim_moves_a_mark_after_the_last_output_event_back_to_it() {
    let events = vec![input(0.0), output(3.0)];

    let marks = moved_marks(&[1.0, 6.0], events, 8.0, all(AUTO_TRIM, trim(false, true)));

    assert_eq!(marks, vec![1.0, 3.0]);
}

#[test]
fn pause_insertion_moves_a_mark_at_the_start_of_its_range_after_an_in_place_filter() {
    // The in-place filter on the range from 0.1 s keeps the mark at 0.407 s.
    // The pause at the start of the range from 0.407 s then moves it by 1 s.
    let mut registry = marking(&[0.407]);
    registry.register_in_place(FilterKind::new("same"), |events, _, _| Ok(events.to_vec()));
    let filters = vec![
        all(MARKER, FilterOptions::new()),
        range("same", 0.1, 2.0, FilterOptions::new()),
        range(PAUSE_INSERTION, 0.407, 2.0, insertion(0.0, 1.0)),
    ];

    let rendered = registry
        .render_with_marks(&project(outputs(&[0.0, 1.0]), 2.0, filters))
        .expect("the render must not fail");

    let marks: Vec<f64> = rendered.marks.iter().map(|mark| mark.time).collect();
    assert_eq!(marks, vec![1.0 + 0.407]);
}

#[test]
fn the_viewport_filter_on_all_keeps_the_time_of_each_mark() {
    // The crop changes the screen of 80x24 to 80x10, and moves no time.
    let crop: FilterOptions =
        serde_json::from_value(json!({ "rows": { "start": 0, "end": 10 } })).unwrap();

    let marks = moved_marks(
        &[1.0, 3.0],
        outputs(&[0.0, 2.0]),
        5.0,
        all(viewport::VIEWPORT, crop),
    );

    assert_eq!(marks, vec![1.0, 3.0]);
}

// Tests of the cursor filter in the app registry.

#[test]
fn the_app_registry_holds_the_cursor_filter() {
    assert!(registry().contains(&FilterKind::new(cursor::CURSOR)));
}

#[test]
fn the_app_registry_holds_the_viewport_filter() {
    assert!(registry().contains(&FilterKind::new(viewport::VIEWPORT)));
}

#[test]
fn a_project_with_the_cursor_filter_renders_through_the_app_registry() {
    let mut source = Recording::test_fixture();
    source.events = vec![Event {
        time: 1.0,
        event_type: EventType::Output,
        data: "\x1b[?25hx".to_string(),
    }];
    let mut project = EditProject::from_recording(Arc::new(source)).unwrap();
    project.filters.push(AppliedFilter {
        kind: FilterKind::new(cursor::CURSOR),
        region: FilterRegion::All,
        options: serde_json::from_value(json!({ "visibility": "always-hidden" })).unwrap(),
    });

    let rendered = registry().render(&project).unwrap();

    let data: Vec<(f64, &str)> = rendered
        .events
        .iter()
        .map(|event| (event.time, event.data.as_str()))
        .collect();
    assert_eq!(data, vec![(0.0, "\x1b[?25l"), (1.0, "x")]);
}
