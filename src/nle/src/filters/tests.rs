//! Tests for the built-in filters. The options are written with the literal
//! keys of the project file, because those keys are a file format.

use std::sync::Arc;

use super::*;
use crate::error::NleError;
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
    builtin().run(events, &[filter])
}

/// Renders a project of one recording that holds `events` and ends at
/// `duration`, with `filter` applied.
fn render(events: Vec<Event>, duration: f64, filter: AppliedFilter) -> Recording {
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
    project.filters = vec![filter];
    builtin().render(&project).unwrap()
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
    let out = run(outputs(&[5.0, 0.0, 1.0]), all(PAUSE_REMOVAL, removal(2.0, 1.0))).unwrap();

    assert_eq!(times(&out), vec![0.0, 1.0, 2.0]);
}

#[test]
fn pause_removal_moves_the_end_by_the_removed_time_and_keeps_the_time_after_the_last_event() {
    // The time from the last event at 5 to the end at 7 is not a gap between
    // two events, so it stays.
    let rendered = render(outputs(&[0.0, 1.0, 5.0]), 7.0, all(PAUSE_REMOVAL, removal(2.0, 1.0)));

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
