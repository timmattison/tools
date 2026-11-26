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
            assert!(
                message.contains("skip-to-end, accelerate, summary"),
                "{message}"
            );
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

fn summary_options(
    head: Option<serde_json::Value>,
    tail: Option<serde_json::Value>,
) -> FilterOptions {
    let mut options = FilterOptions::new().with(MODE_OPTION, SUMMARY);
    if let Some(head) = head {
        options = options.with(HEAD_OPTION, head);
    }
    if let Some(tail) = tail {
        options = options.with(TAIL_OPTION, tail);
    }
    options
}

#[test]
fn bad_summary_options_fail_the_run() {
    use serde_json::json;
    let cases = [
        ("no head", summary_options(None, Some(json!(1)))),
        (
            "negative head",
            summary_options(Some(json!(-1)), Some(json!(1))),
        ),
        (
            "fraction head",
            summary_options(Some(json!(1.5)), Some(json!(1))),
        ),
        (
            "string head",
            summary_options(Some(json!("3")), Some(json!(1))),
        ),
        ("no tail", summary_options(Some(json!(1)), None)),
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
fn mode_of_reads_back_a_summary_and_keeps_its_json_form() {
    let mode = Mode::Summary { head: 3, tail: 2 };
    let filter = applied(range(1.0, 2.0), mode);
    assert_eq!(mode_of(&filter), Some(mode));
    assert_eq!(
        serde_json::to_value(&filter.options).unwrap(),
        serde_json::json!({"mode": "summary", "head": 3, "tail": 2})
    );
}

#[test]
fn mode_of_is_none_for_bad_summary_options() {
    use serde_json::json;
    assert_eq!(
        mode_of(&with_options(summary_options(None, Some(json!(1))))),
        None
    );
    assert_eq!(
        mode_of(&with_options(summary_options(
            Some(json!(-1)),
            Some(json!(1))
        ))),
        None
    );
}

type Triple = (f64, EventType, String);

fn triples(events: &[Event]) -> Vec<Triple> {
    events
        .iter()
        .map(|e| (e.time, e.event_type.clone(), e.data.clone()))
        .collect()
}

fn out(time: f64, data: &str) -> Triple {
    (time, EventType::Output, data.to_string())
}

fn event_of(triple: &Triple) -> Event {
    Event {
        time: triple.0,
        event_type: triple.1.clone(),
        data: triple.2.clone(),
    }
}

fn summary(start: f64, end: f64, head: usize, tail: usize) -> AppliedFilter {
    applied(range(start, end), Mode::Summary { head, tail })
}

fn run_summary(input: &[Triple], filter: &AppliedFilter) -> Vec<Triple> {
    let events: Vec<Event> = input.iter().map(event_of).collect();
    triples(
        &filters::registry()
            .run(events, std::slice::from_ref(filter))
            .unwrap(),
    )
}

#[test]
fn summary_of_one_event_for_each_line_drops_the_middle_and_the_time() {
    let mut input: Vec<Triple> = (1..=10)
        .map(|n| out(10.0 + f64::from(n), &format!("l{n}\r\n")))
        .collect();
    input.push(out(35.0, "late"));
    let got = run_summary(&input, &summary(10.0, 30.0, 2, 2));
    assert_eq!(
        got,
        vec![
            out(11.0, "l1\r\n"),
            out(12.0, "l2\r\n"),
            out(13.0, "...\r\n"),
            out(14.0, "l9\r\n"),
            out(15.0, "l10\r\n"),
            out(30.0, "late"),
        ]
    );
}

#[test]
fn summary_of_short_output_leaves_the_span_unchanged() {
    let input = vec![
        out(11.0, "a\r\nb\r\n"),
        out(12.0, "c\r\n"),
        out(13.0, "d\r\n"),
        out(35.0, "late"),
    ];
    let got = run_summary(&input, &summary(10.0, 30.0, 2, 2));
    assert_eq!(got, input);
}

#[test]
fn summary_of_all_lines_in_one_event_splits_the_event() {
    let input = vec![out(12.0, "a\r\nb\r\nc\r\nd\r\ne\r\n"), out(35.0, "late")];
    let got = run_summary(&input, &summary(10.0, 30.0, 1, 1));
    assert_eq!(
        got,
        vec![
            out(12.0, "a\r\n"),
            out(12.0, "...\r\n"),
            out(12.0, "e\r\n"),
            out(35.0, "late"),
        ]
    );
}

#[test]
fn summary_of_lines_that_cross_events_joins_their_pieces() {
    let input = vec![
        out(11.0, "a\r\nb"),
        out(12.0, "\r\nc\r\nd\r\n"),
        out(13.0, "e\r\nf"),
    ];
    let got = run_summary(&input, &summary(10.0, 30.0, 1, 2));
    assert_eq!(
        got,
        vec![
            out(11.0, "a\r\n"),
            out(11.0, "...\r\n"),
            out(12.0, "e\r\nf")
        ]
    );
}

#[test]
fn summary_keeps_multibyte_text_whole() {
    let input = vec![
        out(11.0, "日本語\r\n"),
        out(12.0, "🎉🎊\r\n"),
        out(13.0, "café\r\n"),
        out(14.0, "x\r\n"),
        out(15.0, "y\r\n"),
    ];
    let got = run_summary(&input, &summary(10.0, 30.0, 1, 1));
    assert_eq!(
        got,
        vec![
            out(11.0, "日本語\r\n"),
            out(12.0, "...\r\n"),
            out(13.0, "y\r\n")
        ]
    );

    let one = vec![out(12.0, "日本語\r\n🎉\r\ncafé\r\n")];
    let got = run_summary(&one, &summary(10.0, 30.0, 1, 1));
    assert_eq!(
        got,
        vec![
            out(12.0, "日本語\r\n"),
            out(12.0, "...\r\n"),
            out(12.0, "café\r\n")
        ]
    );
}

#[test]
fn summary_shortens_the_length_by_the_dropped_time() {
    let recording = Recording {
        version: 2,
        width: 80,
        height: 24,
        timestamp: 0.0,
        duration: 40.0,
        command: String::new(),
        title: String::new(),
        env: BTreeMap::new(),
        events: (1..=10)
            .map(|n| Event {
                time: 10.0 + f64::from(n),
                event_type: EventType::Output,
                data: format!("l{n}\r\n"),
            })
            .chain([Event {
                time: 35.0,
                event_type: EventType::Output,
                data: "late".to_string(),
            }])
            .collect(),
    };
    let mut project = EditProject::from_recording(Arc::new(recording)).unwrap();
    project.filters.push(summary(10.0, 30.0, 2, 2));
    let rendered = filters::registry().render(&project).unwrap();
    assert_eq!(rendered.duration, 35.0);
    assert_eq!(rendered.events.last().map(|e| e.time), Some(30.0));
}

fn inp(time: f64, data: &str) -> Triple {
    (time, EventType::Input, data.to_string())
}

#[test]
fn summary_drops_inputs_between_dropped_output_and_moves_later_inputs() {
    let input = vec![
        inp(11.0, "i1"),
        out(12.0, "a\r\n"),
        out(13.0, "b\r\n"),
        inp(14.0, "i2"),
        out(15.0, "c\r\n"),
        out(16.0, "d\r\n"),
        inp(18.0, "i3"),
    ];
    let got = run_summary(&input, &summary(10.0, 30.0, 1, 1));
    assert_eq!(
        got,
        vec![
            inp(11.0, "i1"),
            out(12.0, "a\r\n"),
            out(13.0, "...\r\n"),
            out(14.0, "d\r\n"),
            inp(16.0, "i3"),
        ]
    );
}

#[test]
fn summary_with_no_tail_keeps_only_the_head() {
    let mut input: Vec<Triple> = (1..=5)
        .map(|n| out(10.0 + f64::from(n), &format!("l{n}\r\n")))
        .collect();
    input.push(inp(17.0, "i"));
    let got = run_summary(&input, &summary(10.0, 30.0, 1, 0));
    assert_eq!(
        got,
        vec![out(11.0, "l1\r\n"), out(12.0, "...\r\n"), inp(14.0, "i")]
    );
}

#[test]
fn summary_with_no_head_puts_the_marker_at_the_first_output() {
    let input: Vec<Triple> = (1..=4)
        .map(|n| out(10.0 + f64::from(n), &format!("l{n}\r\n")))
        .collect();
    let got = run_summary(&input, &summary(10.0, 30.0, 0, 1));
    assert_eq!(got, vec![out(11.0, "...\r\n"), out(12.0, "l4\r\n")]);
}

#[test]
fn summary_of_a_region_without_output_is_unchanged() {
    let only_inputs = vec![inp(11.0, "a\n"), inp(12.0, "b\n"), inp(13.0, "c\n")];
    assert_eq!(
        run_summary(&only_inputs, &summary(10.0, 30.0, 0, 0)),
        only_inputs
    );
    assert_eq!(run_summary(&[], &summary(10.0, 30.0, 0, 0)), vec![]);
}

fn render_summary(items: &[(f64, &str)], duration: f64, head: usize, tail: usize) -> Recording {
    let recording = Recording {
        version: 2,
        width: 80,
        height: 24,
        timestamp: 0.0,
        duration,
        command: String::new(),
        title: String::new(),
        env: BTreeMap::new(),
        events: items
            .iter()
            .map(|(time, data)| Event {
                time: *time,
                event_type: EventType::Output,
                data: (*data).to_string(),
            })
            .collect(),
    };
    let mut project = EditProject::from_recording(Arc::new(recording)).unwrap();
    project
        .filters
        .push(applied(FilterRegion::All, Mode::Summary { head, tail }));
    filters::registry().render(&project).unwrap()
}

#[test]
fn summary_length_never_falls_below_the_largest_time_after_rounding() {
    let items = [(0.0, "a\r\n"), (0.1, "b\r\n"), (0.4, "c\r\n")];
    let rendered = render_summary(&items, 0.4, 1, 0);
    assert_eq!(
        triples(&rendered.events),
        vec![out(0.0, "a\r\n"), out(0.1, "...\r\n")]
    );
    assert_eq!(rendered.duration, 0.1);
}

#[test]
fn summary_tail_never_moves_before_the_marker_after_rounding() {
    let items = [
        (0.0, "a\r\n"),
        (0.1, "b\r\n"),
        (0.4, "c\r\n"),
        (0.4, "d\r\n"),
    ];
    let rendered = render_summary(&items, 0.4, 1, 1);
    assert_eq!(
        triples(&rendered.events),
        vec![out(0.0, "a\r\n"), out(0.1, "...\r\n"), out(0.1, "d\r\n")]
    );
}
