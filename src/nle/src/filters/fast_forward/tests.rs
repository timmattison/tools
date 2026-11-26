//! Tests for the fast-forward filter.

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

fn run_times(input: &[f64], filters_list: &[AppliedFilter]) -> Vec<f64> {
    let out = filters::registry()
        .run(events(input), filters_list)
        .unwrap();
    times(&out)
}

fn skip(start: f64, end: f64) -> AppliedFilter {
    applied(range(start, end), Mode::SkipToEnd)
}

#[test]
fn acceleration_accepts_finite_values_above_one() {
    for good in [1.5, 20.0, 50.0] {
        assert_eq!(Acceleration::new(good).unwrap().get(), good);
    }
}

#[test]
fn acceleration_refuses_bad_values() {
    for bad in [
        1.0,
        0.5,
        0.0,
        -1.0,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ] {
        assert!(
            matches!(
                Acceleration::new(bad),
                Err(NleError::InvalidAcceleration(_))
            ),
            "{bad} must be refused"
        );
    }
}

#[test]
fn skip_to_end_moves_region_events_to_the_start_and_removes_the_length() {
    let got = run_times(&[5.0, 12.0, 15.0, 25.0], &[skip(10.0, 20.0)]);
    assert_eq!(got, vec![5.0, 10.0, 10.0, 15.0]);
}

#[test]
fn skip_to_end_of_a_region_with_no_event_removes_the_length() {
    let got = run_times(&[5.0, 25.0], &[skip(10.0, 20.0)]);
    assert_eq!(got, vec![5.0, 15.0]);
}

#[test]
fn skip_to_end_keeps_type_data_and_order_of_events() {
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

    let out = filters::registry().run(input, &[skip(10.0, 20.0)]).unwrap();

    let got: Vec<(f64, EventType, &str)> = out
        .iter()
        .map(|e| (e.time, e.event_type.clone(), e.data.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            (10.0, EventType::Input, "first"),
            (10.0, EventType::Output, "second"),
            (10.0, EventType::Output, "third"),
        ]
    );
}

#[test]
fn skip_to_end_with_the_whole_region_puts_every_event_at_zero() {
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
    project
        .filters
        .push(applied(FilterRegion::All, Mode::SkipToEnd));

    let rendered = filters::registry().render(&project).unwrap();

    assert_eq!(times(&rendered.events), vec![0.0, 0.0, 0.0, 0.0]);
    assert_eq!(rendered.duration, 0.0);
}

#[test]
fn the_registry_holds_the_fast_forward_filter() {
    assert!(filters::registry().contains(&kind()));
}

fn accelerate(start: f64, end: f64, value: f64) -> AppliedFilter {
    applied(
        range(start, end),
        Mode::Accelerate(Acceleration::new(value).unwrap()),
    )
}

#[test]
fn accelerate_divides_times_and_length_and_moves_later_events() {
    let got = run_times(&[4.0, 12.0, 24.0], &[accelerate(8.0, 24.0, 16.0)]);
    assert_eq!(got, vec![4.0, 8.25, 9.0]);
}

fn with_options(options: FilterOptions) -> AppliedFilter {
    AppliedFilter {
        kind: kind(),
        region: range(10.0, 20.0),
        options,
    }
}

#[test]
fn bad_options_fail_the_run() {
    let cases = [
        ("no mode", FilterOptions::new()),
        ("number mode", FilterOptions::new().with(MODE_OPTION, 3)),
        (
            "unknown mode",
            FilterOptions::new().with(MODE_OPTION, "rewind"),
        ),
        (
            "no speed",
            FilterOptions::new().with(MODE_OPTION, ACCELERATE),
        ),
        (
            "string speed",
            FilterOptions::new()
                .with(MODE_OPTION, ACCELERATE)
                .with(SPEED_OPTION, "fast"),
        ),
        (
            "speed one",
            FilterOptions::new()
                .with(MODE_OPTION, ACCELERATE)
                .with(SPEED_OPTION, 1.0),
        ),
        (
            "speed below one",
            FilterOptions::new()
                .with(MODE_OPTION, ACCELERATE)
                .with(SPEED_OPTION, 0.5),
        ),
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
fn an_unknown_mode_lists_the_known_modes() {
    let options = FilterOptions::new().with(MODE_OPTION, "rewind");
    match filters::registry().run(events(&[5.0]), &[with_options(options)]) {
        Err(NleError::RenderError(message)) => {
            assert!(message.contains("skip-to-end, accelerate"), "{message}");
        }
        other => panic!("expected a render error, got {other:?}"),
    }
}

#[test]
fn mode_of_reads_back_an_applied_filter() {
    let accelerate = Mode::Accelerate(Acceleration::new(20.0).unwrap());
    for mode in [Mode::SkipToEnd, accelerate] {
        assert_eq!(mode_of(&applied(range(1.0, 2.0), mode)), Some(mode));
    }
}

#[test]
fn mode_of_is_none_for_other_kinds_and_bad_options() {
    let other_kind = AppliedFilter {
        kind: FilterKind::new("other"),
        region: FilterRegion::All,
        options: FilterOptions::new().with(MODE_OPTION, SKIP_TO_END),
    };
    assert_eq!(mode_of(&other_kind), None);
    for options in [
        FilterOptions::new(),
        FilterOptions::new().with(MODE_OPTION, 3),
        FilterOptions::new().with(MODE_OPTION, "rewind"),
        FilterOptions::new().with(MODE_OPTION, ACCELERATE),
        FilterOptions::new()
            .with(MODE_OPTION, ACCELERATE)
            .with(SPEED_OPTION, 1.0),
    ] {
        assert_eq!(mode_of(&with_options(options)), None);
    }
}

#[test]
fn options_keep_their_json_form() {
    let accelerate = Mode::Accelerate(Acceleration::new(20.0).unwrap());
    let json = serde_json::to_value(&applied(FilterRegion::All, accelerate).options).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"mode": "accelerate", "speed": 20.0})
    );
    let json = serde_json::to_value(&applied(FilterRegion::All, Mode::SkipToEnd).options).unwrap();
    assert_eq!(json, serde_json::json!({"mode": "skip-to-end"}));
}
