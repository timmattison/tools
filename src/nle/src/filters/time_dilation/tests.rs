//! Tests for the time dilation filter.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::*;
use crate::filters;
use crate::types::{EditProject, Event, EventType, Recording, Seconds};

fn ev(time: f64) -> Event {
    Event {
        time,
        event_type: EventType::Output,
        data: format!("e{time}"),
    }
}

fn events(times: &[f64]) -> Vec<Event> {
    times.iter().copied().map(ev).collect()
}

fn times(events: &[Event]) -> Vec<f64> {
    events.iter().map(|e| e.time).collect()
}

fn range(start: f64, end: f64) -> FilterRegion {
    FilterRegion::Range {
        start: Seconds::new(start).unwrap(),
        end: Seconds::new(end).unwrap(),
    }
}

fn speed(value: f64) -> Speed {
    Speed::new(value).unwrap()
}

fn dilation(start: f64, end: f64, value: f64) -> AppliedFilter {
    applied(range(start, end), speed(value))
}

fn run_times(input: &[f64], filters_list: &[AppliedFilter]) -> Vec<f64> {
    let out = filters::registry()
        .run(events(input), filters_list)
        .unwrap();
    times(&out)
}

#[test]
fn speed_accepts_positive_finite_values() {
    for good in [0.5, 1.0, 2.0] {
        assert_eq!(Speed::new(good).unwrap().get(), good);
    }
}

#[test]
fn speed_refuses_bad_values() {
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(
            matches!(Speed::new(bad), Err(NleError::InvalidSpeed(_))),
            "{bad} must be refused"
        );
    }
}

#[test]
fn speed_up_shrinks_the_region_and_moves_later_events() {
    let got = run_times(&[5.0, 12.0, 15.0, 25.0], &[dilation(10.0, 20.0, 2.0)]);
    assert_eq!(got, vec![5.0, 11.0, 12.5, 20.0]);
}

#[test]
fn slow_down_stretches_the_region_and_moves_later_events() {
    let got = run_times(&[5.0, 12.0, 25.0], &[dilation(10.0, 20.0, 0.5)]);
    assert_eq!(got, vec![5.0, 14.0, 35.0]);
}

#[test]
fn a_region_with_no_event_still_changes_the_length() {
    let got = run_times(&[5.0, 25.0], &[dilation(10.0, 20.0, 2.0)]);
    assert_eq!(got, vec![5.0, 20.0]);
}

#[test]
fn render_with_the_whole_region_scales_events_and_duration() {
    let recording = Recording {
        version: 2,
        width: 80,
        height: 24,
        timestamp: 0.0,
        duration: 10.0,
        command: String::new(),
        title: String::new(),
        env: BTreeMap::new(),
        events: events(&[0.0, 2.0, 4.0, 8.0]),
    };
    let mut project = EditProject::from_recording(Arc::new(recording)).unwrap();
    project.filters.push(applied(FilterRegion::All, speed(2.0)));

    let rendered = filters::registry().render(&project).unwrap();

    assert_eq!(times(&rendered.events), vec![0.0, 1.0, 2.0, 4.0]);
    assert_eq!(rendered.duration, 5.0);
}

#[test]
fn two_regions_can_have_different_speeds() {
    let got = run_times(
        &[2.0, 8.0, 12.0, 22.0, 28.0, 40.0],
        &[dilation(0.0, 10.0, 2.0), dilation(20.0, 30.0, 0.5)],
    );
    assert_eq!(got, vec![1.0, 4.0, 7.0, 17.0, 26.0, 45.0]);
}

#[test]
fn speed_one_changes_nothing() {
    let got = run_times(&[5.0, 12.0, 25.0], &[dilation(10.0, 20.0, 1.0)]);
    assert_eq!(got, vec![5.0, 12.0, 25.0]);
}

#[test]
fn data_type_and_order_of_events_stay() {
    let input = vec![
        Event {
            time: 12.0,
            event_type: EventType::Input,
            data: "first".to_string(),
        },
        Event {
            time: 12.0,
            event_type: EventType::Output,
            data: "second".to_string(),
        },
        Event {
            time: 14.0,
            event_type: EventType::Output,
            data: "third".to_string(),
        },
    ];

    let out = filters::registry()
        .run(input, &[dilation(10.0, 20.0, 2.0)])
        .unwrap();

    let got: Vec<(f64, EventType, &str)> = out
        .iter()
        .map(|e| (e.time, e.event_type.clone(), e.data.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            (11.0, EventType::Input, "first"),
            (11.0, EventType::Output, "second"),
            (12.0, EventType::Output, "third"),
        ]
    );
}

fn with_options(options: FilterOptions) -> AppliedFilter {
    AppliedFilter {
        kind: kind(),
        region: range(10.0, 20.0),
        options,
    }
}

#[test]
fn a_bad_speed_option_fails_the_run() {
    let cases = [
        ("missing", FilterOptions::new()),
        ("string", FilterOptions::new().with(SPEED_OPTION, "fast")),
        ("zero", FilterOptions::new().with(SPEED_OPTION, 0.0)),
        ("negative", FilterOptions::new().with(SPEED_OPTION, -2.0)),
    ];
    for (name, options) in cases {
        let result = filters::registry().run(events(&[5.0, 12.0]), &[with_options(options)]);
        match result {
            Err(NleError::RenderError(message)) => {
                assert!(message.contains(KIND), "{name}: {message}");
            }
            other => panic!("{name}: expected a render error, got {other:?}"),
        }
    }
}

#[test]
fn speed_of_reads_back_an_applied_filter() {
    let filter = applied(range(1.0, 2.0), speed(0.25));
    assert_eq!(speed_of(&filter), Some(speed(0.25)));
}

#[test]
fn speed_of_is_none_for_other_kinds_and_bad_options() {
    let other_kind = AppliedFilter {
        kind: FilterKind::new("other"),
        region: FilterRegion::All,
        options: FilterOptions::new().with(SPEED_OPTION, 2.0),
    };
    assert_eq!(speed_of(&other_kind), None);
    for options in [
        FilterOptions::new().with(SPEED_OPTION, 0.0),
        FilterOptions::new().with(SPEED_OPTION, "fast"),
        FilterOptions::new(),
    ] {
        assert_eq!(speed_of(&with_options(options)), None);
    }
}

#[test]
fn the_shipped_registry_holds_time_dilation() {
    assert!(filters::registry().contains(&kind()));
}

#[test]
fn a_mark_inside_the_region_moves_with_the_speed() {
    use crate::pipeline::{Mark, Span};
    let mut registry = filters::registry();
    registry.register_span(
        FilterKind::new("mark"),
        |mut span: Span, _: &FilterOptions| {
            span.marks.push(Mark {
                time: 6.0,
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
        duration: 10.0,
        command: String::new(),
        title: String::new(),
        env: BTreeMap::new(),
        events: events(&[0.0, 9.0]),
    };
    let mut project = EditProject::from_recording(Arc::new(recording)).unwrap();
    project.filters = vec![
        AppliedFilter {
            kind: FilterKind::new("mark"),
            region: FilterRegion::All,
            options: FilterOptions::new(),
        },
        dilation(2.0, 10.0, 2.0),
    ];

    let rendered = registry.render_with_marks(&project).unwrap();

    // The mark is 4 s into the region, and a speed of 2 makes that 2 s.
    let times: Vec<f64> = rendered.marks.iter().map(|mark| mark.time).collect();
    assert_eq!(times, vec![4.0]);
}
