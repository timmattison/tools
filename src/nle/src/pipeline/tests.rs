//! Tests for the filter pipeline and the render.

use super::*;
use crate::types::{Clip, ClipId, EventType, FilterRegion, Seconds, TimelineTrack, TrackId};

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

fn kind(name: &str) -> FilterKind {
    FilterKind::new(name)
}

fn applied(name: &str, region: FilterRegion) -> AppliedFilter {
    AppliedFilter {
        kind: kind(name),
        region,
        options: FilterOptions::new(),
    }
}

fn all(name: &str) -> AppliedFilter {
    applied(name, FilterRegion::All)
}

fn range(name: &str, start: f64, end: f64) -> AppliedFilter {
    applied(
        name,
        FilterRegion::Range {
            start: Seconds::new(start).unwrap(),
            end: Seconds::new(end).unwrap(),
        },
    )
}

/// Moves each event to `map(time)`.
fn mapper(map: fn(f64) -> f64) -> impl Fn(&[Event], &FilterOptions) -> Vec<Event> + Send + Sync {
    move |events, _| {
        events
            .iter()
            .map(|e| Event {
                time: map(e.time),
                ..e.clone()
            })
            .collect()
    }
}

fn double() -> impl Fn(&[Event], &FilterOptions) -> Vec<Event> + Send + Sync {
    mapper(|t| t * 2.0)
}

fn add_one() -> impl Fn(&[Event], &FilterOptions) -> Vec<Event> + Send + Sync {
    mapper(|t| t + 1.0)
}

// Tests of the registry.

#[test]
fn contains_is_false_before_and_true_after_register() {
    let mut registry = FilterRegistry::new();
    assert!(!registry.contains(&kind("double")));

    registry.register(kind("double"), double());

    assert!(registry.contains(&kind("double")));
    assert!(!registry.contains(&kind("other")));
}

#[test]
fn a_second_register_under_one_kind_replaces_the_first() {
    let mut registry = FilterRegistry::new();
    registry.register(kind("f"), double());
    registry.register(kind("f"), add_one());

    let out = registry.run(events(&[1.0, 2.0]), &[all("f")]).unwrap();

    assert_eq!(times(&out), vec![2.0, 3.0]);
}

#[test]
fn run_with_an_unregistered_kind_fails_and_names_the_kind() {
    let registry = FilterRegistry::new();

    let result = registry.run(events(&[1.0]), &[all("wobble")]);

    match result {
        Err(NleError::RenderError(message)) => assert!(message.contains("wobble"), "{message}"),
        other => panic!("expected a RenderError, got {other:?}"),
    }
}

// Tests of the region All.

#[test]
fn one_filter_on_all_changes_every_event() {
    let mut registry = FilterRegistry::new();
    registry.register(kind("double"), double());

    let out = registry
        .run(events(&[0.5, 1.0, 2.0]), &[all("double")])
        .unwrap();

    assert_eq!(times(&out), vec![1.0, 2.0, 4.0]);
}

#[test]
fn two_filters_chain_in_order() {
    let mut registry = FilterRegistry::new();
    registry.register(kind("double"), double());
    registry.register(kind("add"), add_one());

    let add_then_double = registry
        .run(events(&[1.0, 2.0]), &[all("add"), all("double")])
        .unwrap();
    let double_then_add = registry
        .run(events(&[1.0, 2.0]), &[all("double"), all("add")])
        .unwrap();

    assert_eq!(times(&add_then_double), vec![4.0, 6.0]);
    assert_eq!(times(&double_then_add), vec![3.0, 5.0]);
}

// Tests of a range.

use std::sync::{Arc as TestArc, Mutex};

/// Registers `double` and a spy that records the events it gets and then
/// answers with `answer(events)`.
fn spy_registry(
    answer: fn(&[Event]) -> Vec<Event>,
) -> (FilterRegistry, TestArc<Mutex<Vec<Vec<Event>>>>) {
    let seen = TestArc::new(Mutex::new(Vec::new()));
    let captured = TestArc::clone(&seen);
    let mut registry = FilterRegistry::new();
    registry.register(kind("spy"), move |events, _| {
        captured.lock().unwrap().push(events.to_vec());
        answer(events)
    });
    (registry, seen)
}

#[test]
fn a_range_filter_sees_the_events_inside_relative_to_the_region_start() {
    let (registry, seen) = spy_registry(|events| {
        events
            .iter()
            .map(|e| Event {
                time: e.time * 2.0,
                ..e.clone()
            })
            .collect()
    });
    let input = events(&[0.5, 1.0, 1.5, 2.5, 3.5]);

    let out = registry
        .run(input.clone(), &[range("spy", 1.0, 2.0)])
        .unwrap();

    let calls = seen.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(times(&calls[0]), vec![0.0, 0.5]);
    assert_eq!(times(&out), vec![0.5, 1.0, 2.0, 3.0, 4.0]);
    // The data and the type pass through.
    assert_eq!(out[0], input[0]);
    assert_eq!(out[1].data, input[1].data);
    assert_eq!(out[1].event_type, input[1].event_type);
    assert_eq!(out[4].data, input[4].data);
}

#[test]
fn a_filter_that_returns_nothing_moves_the_later_events_left() {
    let (registry, _seen) = spy_registry(|_| Vec::new());

    let out = registry
        .run(
            events(&[0.5, 1.0, 1.5, 2.5, 3.5]),
            &[range("spy", 1.0, 2.0)],
        )
        .unwrap();

    assert_eq!(times(&out), vec![0.5, 2.0, 3.0]);
}

#[test]
fn an_empty_region_still_calls_the_filter_with_an_empty_slice() {
    let (registry, seen) = spy_registry(|events| events.to_vec());

    let out = registry
        .run(events(&[0.5, 3.5]), &[range("spy", 1.0, 2.0)])
        .unwrap();

    let calls = seen.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].is_empty());
    assert_eq!(times(&out), vec![0.5, 3.5]);
}

#[test]
fn an_event_at_the_end_of_a_range_is_after_the_range_and_moves_by_the_shift() {
    let (registry, seen) = spy_registry(|events| {
        events
            .iter()
            .map(|e| Event {
                time: e.time * 2.0,
                ..e.clone()
            })
            .collect()
    });

    let out = registry
        .run(
            events(&[0.5, 1.0, 1.5, 2.0, 3.0]),
            &[range("spy", 1.0, 2.0)],
        )
        .unwrap();

    // The filter sees 1.0 and 1.5, and not the event at the end 2.0.
    let calls = seen.lock().unwrap();
    assert_eq!(times(&calls[0]), vec![0.0, 0.5]);
    // The shift is 1.0 - 0.5, so 2.0 and 3.0 both move to 2.5 and 3.5.
    assert_eq!(times(&out), vec![0.5, 1.0, 2.0, 2.5, 3.5]);
}

// Tests of the checks on the answer of a filter.

fn registry_answering(time: f64) -> FilterRegistry {
    let mut registry = FilterRegistry::new();
    registry.register(kind("bad"), move |events, _| {
        events.iter().map(|e| Event { time, ..e.clone() }).collect()
    });
    registry
}

fn assert_render_error_names(result: Result<Vec<Event>, NleError>, name: &str) {
    match result {
        Err(NleError::RenderError(message)) => assert!(message.contains(name), "{message}"),
        other => panic!("expected a RenderError, got {other:?}"),
    }
}

#[test]
fn a_filter_that_answers_nan_or_infinity_fails_and_names_the_kind() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let registry = registry_answering(bad);

        let result = registry.run(events(&[1.0, 2.0]), &[all("bad")]);

        assert_render_error_names(result, "bad");
    }
}

#[test]
fn a_filter_that_answers_a_time_below_zero_on_the_timeline_fails() {
    let registry = registry_answering(-0.5);
    assert_render_error_names(registry.run(events(&[1.0]), &[all("bad")]), "bad");

    // Relative -1.5 on a range that starts at 1.0 is -0.5 on the timeline.
    let registry = registry_answering(-1.5);
    assert_render_error_names(
        registry.run(events(&[1.0]), &[range("bad", 1.0, 2.0)]),
        "bad",
    );
}

#[test]
fn two_shifts_that_add_up_to_infinity_fail_the_run() {
    let mut registry = FilterRegistry::new();
    registry.register(kind("far"), |_, _| vec![ev(f64::MAX)]);

    // The first filter moves 1.0 to about f64::MAX. The second sees nothing
    // in its range, answers f64::MAX again, and its shift moves the later
    // events to infinity.
    let result = registry.run(
        events(&[0.0, 1.0]),
        &[range("far", 0.0, 0.5), range("far", 0.0, 0.5)],
    );

    assert!(
        matches!(result, Err(NleError::RenderError(_))),
        "{result:?}"
    );
}

#[test]
fn an_event_time_that_is_not_finite_fails_the_run_when_the_end_is_finite() {
    // The end starts at the largest time, and `f64::max` skips NaN and is
    // never -inf here. So the end stays 0.5, and only the check of each event
    // time can refuse the run.
    for bad in [f64::NAN, f64::NEG_INFINITY] {
        let result = FilterRegistry::new().run(events(&[0.5, bad]), &[]);

        assert!(
            matches!(result, Err(NleError::RenderError(_))),
            "{bad}: {result:?}"
        );
    }
}

#[test]
fn a_relative_time_below_zero_is_fine_when_the_timeline_time_is_not() {
    // Relative -0.5 on a range that starts at 1.0 is 0.5 on the timeline.
    let registry = registry_answering(-0.5);

    let out = registry
        .run(events(&[1.0]), &[range("bad", 1.0, 2.0)])
        .unwrap();

    assert_eq!(times(&out), vec![0.5]);
}

// Tests of the order of the result.

fn data_event(time: f64, data: &str) -> Event {
    Event {
        time,
        event_type: EventType::Output,
        data: data.to_string(),
    }
}

#[test]
fn a_filter_that_answers_out_of_order_gives_a_result_in_time_order() {
    let mut registry = FilterRegistry::new();
    registry.register(kind("reverse"), |events, _| {
        events.iter().rev().cloned().collect()
    });

    let out = registry
        .run(events(&[0.5, 1.0, 1.5, 2.0]), &[all("reverse")])
        .unwrap();

    assert_eq!(times(&out), vec![0.5, 1.0, 1.5, 2.0]);
}

#[test]
fn events_at_one_time_keep_the_order_the_filter_gave() {
    let mut registry = FilterRegistry::new();
    registry.register(kind("tie"), |_, _| {
        vec![
            data_event(2.0, "late"),
            data_event(1.0, "first"),
            data_event(1.0, "second"),
            data_event(1.0, "third"),
        ]
    });

    let out = registry.run(events(&[0.0]), &[all("tie")]).unwrap();

    let data: Vec<&str> = out.iter().map(|e| e.data.as_str()).collect();
    assert_eq!(data, vec!["first", "second", "third", "late"]);
}

// Tests of a filter with typed options.

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ShiftBy {
    seconds: f64,
}

/// Moves each event later by `options.seconds`, and leaves the end to the
/// shift.
fn shift_by(span: Span, options: &ShiftBy) -> Span {
    Span {
        events: span
            .events
            .into_iter()
            .map(|e| Event {
                time: e.time + options.seconds,
                ..e
            })
            .collect(),
        end: None,
        shift: None,
    }
}

fn typed_registry() -> FilterRegistry {
    let mut registry = FilterRegistry::new();
    registry.register_typed(kind("shift"), shift_by);
    registry
}

fn all_with(name: &str, options: FilterOptions) -> AppliedFilter {
    AppliedFilter {
        kind: kind(name),
        region: FilterRegion::All,
        options,
    }
}

#[test]
fn a_typed_filter_gets_its_options_parsed() {
    let out = typed_registry()
        .run(
            events(&[1.0, 2.0]),
            &[all_with("shift", FilterOptions::new().with("seconds", 0.5))],
        )
        .unwrap();

    assert_eq!(times(&out), vec![1.5, 2.5]);
}

#[test]
fn a_typed_filter_with_invalid_options_fails_and_names_the_kind_and_the_reason() {
    let cases = [
        (FilterOptions::new(), "seconds"),
        (FilterOptions::new().with("seconds", "soon"), "invalid type"),
        (
            FilterOptions::new().with("seconds", 0.5).with("extra", 1),
            "extra",
        ),
    ];
    for (options, reason) in cases {
        let result = typed_registry().run(events(&[1.0]), &[all_with("shift", options)]);

        match result {
            Err(NleError::RenderError(message)) => {
                assert!(message.contains("\"shift\""), "{message}");
                assert!(message.contains("invalid options"), "{message}");
                assert!(message.contains(reason), "{reason}: {message}");
            }
            other => panic!("expected a RenderError, got {other:?}"),
        }
    }
}

// Tests of the render of one clip.

use std::collections::BTreeMap;

fn source(title: &str, event_times: &[f64], duration: f64) -> Recording {
    let mut env = BTreeMap::new();
    env.insert("SHELL".to_string(), format!("/bin/{title}"));
    Recording {
        version: 2,
        width: 100,
        height: 30,
        timestamp: 1_700_000_000.0,
        duration,
        command: format!("run-{title}"),
        title: title.to_string(),
        env,
        events: event_times.iter().copied().map(ev).collect(),
    }
}

#[test]
fn render_of_an_empty_project_fails() {
    let registry = FilterRegistry::new();

    let result = registry.render(&EditProject::default());

    assert!(
        matches!(result, Err(NleError::RenderError(_))),
        "{result:?}"
    );
}

#[test]
fn render_of_a_whole_recording_with_no_filter_equals_its_source() {
    let mut recording = source("whole", &[0.0, 0.5, 1.25, 3.0], 3.0);
    recording.events[1] = data_event(0.5, "日本語🎉");
    let project = EditProject::from_recording(Arc::new(recording.clone())).unwrap();

    let rendered = FilterRegistry::new().render(&project).unwrap();

    assert_eq!(rendered, recording);
}

// Tests of the render of two clips on two tracks.

fn clip_of(id: u64, source: Recording, offset: f64) -> Clip {
    let duration = source.duration;
    Clip {
        id: ClipId(id),
        source: Arc::new(source),
        in_point: Seconds::ZERO,
        out_point: Seconds::new(duration).unwrap(),
        timeline_offset: Seconds::new(offset).unwrap(),
    }
}

fn track_of(id: u64, clip: u64) -> TimelineTrack {
    TimelineTrack {
        id: TrackId(id),
        clips: vec![ClipId(clip)],
    }
}

/// Clip 1 is on track 1 and starts at 2.0. Clip 2 is on track 2 and starts at
/// 0.0, so it starts first, although it is second in `clips`.
fn two_clip_project() -> EditProject {
    let mut late = source("late", &[], 1.0);
    late.events = vec![data_event(0.0, "late0"), data_event(1.0, "late1")];
    let mut early = source("early", &[], 2.0);
    early.events = vec![
        data_event(0.0, "early0"),
        data_event(1.0, "early1"),
        data_event(2.0, "early2"),
    ];
    EditProject {
        clips: vec![clip_of(1, late, 2.0), clip_of(2, early, 0.0)],
        timeline: vec![track_of(1, 1), track_of(2, 2)],
        filters: Vec::new(),
    }
}

#[test]
fn render_merges_two_clips_in_time_order_and_keeps_track_order_on_a_tie() {
    let rendered = FilterRegistry::new().render(&two_clip_project()).unwrap();

    let got: Vec<(f64, &str)> = rendered
        .events
        .iter()
        .map(|e| (e.time, e.data.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            (0.0, "early0"),
            (1.0, "early1"),
            (2.0, "late0"),
            (2.0, "early2"),
            (3.0, "late1"),
        ]
    );
}

#[test]
fn render_takes_the_header_from_the_clip_that_starts_first() {
    let project = two_clip_project();
    let early = Arc::clone(&project.clips[1].source);

    let rendered = FilterRegistry::new().render(&project).unwrap();

    assert_eq!(rendered.version, early.version);
    assert_eq!(rendered.width, early.width);
    assert_eq!(rendered.height, early.height);
    assert_eq!(rendered.timestamp, early.timestamp);
    assert_eq!(rendered.command, "run-early");
    assert_eq!(rendered.title, "early");
    assert_eq!(rendered.env, early.env);
    assert_eq!(rendered.duration, project.duration());
    assert_eq!(rendered.duration, 3.0);
}

// Tests of the duration through filters.

fn one_clip_project(filters: Vec<AppliedFilter>) -> EditProject {
    let mut project =
        EditProject::from_recording(Arc::new(source("one", &[0.0, 8.0], 10.0))).unwrap();
    project.filters = filters;
    project
}

#[test]
fn render_moves_the_duration_by_the_shift_of_a_filter() {
    let mut registry = FilterRegistry::new();
    registry.register(kind("halve"), mapper(|t| t / 2.0));

    let rendered = registry
        .render(&one_clip_project(vec![all("halve")]))
        .unwrap();

    assert_eq!(times(&rendered.events), vec![0.0, 4.0]);
    // The end 10 moves by the shift 4 - 8.
    assert_eq!(rendered.duration, 6.0);
}

#[test]
fn render_keeps_the_duration_for_a_range_that_starts_after_the_end() {
    let mut registry = FilterRegistry::new();
    registry.register(kind("double"), double());

    let rendered = registry
        .render(&one_clip_project(vec![range("double", 20.0, 30.0)]))
        .unwrap();

    assert_eq!(times(&rendered.events), vec![0.0, 8.0]);
    assert_eq!(rendered.duration, 10.0);
}

#[test]
fn render_never_ends_before_the_last_event() {
    let mut registry = FilterRegistry::new();
    registry.register(kind("double"), double());

    let rendered = registry
        .render(&one_clip_project(vec![all("double")]))
        .unwrap();

    // The end 10 moves by 16 - 8 to 18, and the last event is at 16.
    assert_eq!(rendered.duration, 18.0);

    let mut registry = FilterRegistry::new();
    registry.register(kind("far"), |_, _| vec![ev(50.0)]);
    let rendered = registry
        .render(&one_clip_project(vec![range("far", 20.0, 30.0)]))
        .unwrap();
    // The range starts after the end 10, so the end stays, and then it grows
    // to the new last event at 20 + 50.
    assert_eq!(rendered.duration, 70.0);
}

#[test]
fn render_with_an_unknown_filter_kind_fails() {
    let result = FilterRegistry::new().render(&one_clip_project(vec![all("missing")]));

    match result {
        Err(NleError::RenderError(message)) => assert!(message.contains("missing"), "{message}"),
        other => panic!("expected a RenderError, got {other:?}"),
    }
}

#[test]
fn render_refuses_a_project_that_breaks_a_rule() {
    let mut project = one_clip_project(vec![]);
    if let Some(track) = project.timeline.first_mut() {
        track.clips.push(ClipId(99));
    }

    let result = FilterRegistry::new().render(&project);

    assert!(
        matches!(result, Err(NleError::InvalidProject(_))),
        "{result:?}"
    );
}

#[test]
fn render_of_a_clip_whose_events_overflow_the_timeline_fails() {
    // The event at 1e308 goes to 2e308, which is infinity.
    let project = EditProject {
        clips: vec![clip_of(1, source("big", &[0.0, 1e308], 1e308), 1e308)],
        timeline: vec![track_of(1, 1)],
        filters: Vec::new(),
    };

    let result = FilterRegistry::new().render(&project);

    assert!(
        matches!(result, Err(NleError::RenderError(_))),
        "{result:?}"
    );
}

#[test]
fn render_of_a_clip_whose_end_overflows_the_timeline_fails() {
    // The one event goes to 1e308, which is finite, but the duration of the
    // project is infinity.
    let project = EditProject {
        clips: vec![clip_of(1, source("end", &[0.0], 1e308), 1e308)],
        timeline: vec![track_of(1, 1)],
        filters: Vec::new(),
    };

    let result = FilterRegistry::new().render(&project);

    assert!(
        matches!(result, Err(NleError::RenderError(_))),
        "{result:?}"
    );
}

// Tests of the end of the timeline in a typed filter.

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct NoOptions {}

/// A typed filter under the kind "set-end" that keeps its events and answers
/// `end`.
fn end_registry(end: Option<f64>) -> FilterRegistry {
    let mut registry = FilterRegistry::new();
    registry.register_typed(kind("set-end"), move |span: Span, _: &NoOptions| Span {
        events: span.events,
        end,
        shift: None,
    });
    registry
}

fn assert_render_names(result: Result<Recording, NleError>, name: &str) {
    assert_render_error_names(result.map(|recording| recording.events), name);
}

#[test]
fn a_typed_filter_sees_the_end_only_when_the_end_is_inside_its_region() {
    let seen = TestArc::new(Mutex::new(Vec::new()));
    let captured = TestArc::clone(&seen);
    let mut registry = FilterRegistry::new();
    registry.register_typed(kind("spy"), move |span: Span, _: &NoOptions| {
        captured.lock().unwrap().push(span.end);
        Span {
            events: span.events,
            end: None,
            shift: None,
        }
    });

    // The end of the timeline starts at the last event, 4.0.
    let cases = [
        (all("spy"), Some(4.0)),
        (range("spy", 1.0, 5.0), Some(3.0)),
        // A range holds its start and not its end, so an end of the timeline
        // at the end of the range is after the range.
        (range("spy", 0.0, 4.0), None),
        (range("spy", 5.0, 6.0), None),
    ];
    for (filter, expected) in cases {
        registry.run(events(&[1.0, 4.0]), &[filter]).unwrap();

        assert_eq!(seen.lock().unwrap().pop(), Some(expected));
    }
}

#[test]
fn a_typed_filter_that_answers_an_end_sets_the_end_of_the_timeline() {
    // The project ends at 10, and its last event is at 8.
    let rendered = end_registry(Some(8.5))
        .render(&one_clip_project(vec![all("set-end")]))
        .unwrap();
    assert_eq!(rendered.duration, 8.5);

    // The answered end is relative to the start of the range.
    let rendered = end_registry(Some(4.0))
        .render(&one_clip_project(vec![range("set-end", 5.0, 20.0)]))
        .unwrap();
    assert_eq!(rendered.duration, 9.0);

    // The end is never before the last event.
    let rendered = end_registry(Some(1.0))
        .render(&one_clip_project(vec![all("set-end")]))
        .unwrap();
    assert_eq!(rendered.duration, 8.0);

    // No answered end leaves the end to the shift: 10 moves by 8.5 - 8.
    let rendered = typed_registry()
        .render(&one_clip_project(vec![all_with(
            "shift",
            FilterOptions::new().with("seconds", 0.5),
        )]))
        .unwrap();
    assert_eq!(rendered.duration, 10.5);
}

#[test]
fn a_typed_filter_that_answers_an_end_outside_its_region_fails_and_names_the_kind() {
    // The range starts after the end of the project at 10, so the filter
    // does not see the end, and it cannot set it.
    let result =
        end_registry(Some(1.0)).render(&one_clip_project(vec![range("set-end", 20.0, 30.0)]));

    assert_render_names(result, "\"set-end\"");
}

#[test]
fn a_typed_filter_that_answers_an_end_that_is_not_finite_or_is_below_zero_fails() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
        let result = end_registry(Some(bad)).render(&one_clip_project(vec![all("set-end")]));

        assert_render_names(result, "\"set-end\"");
    }

    // Relative -6 on a range that starts at 5 is -1 on the timeline.
    let result =
        end_registry(Some(-6.0)).render(&one_clip_project(vec![range("set-end", 5.0, 20.0)]));
    assert_render_names(result, "\"set-end\"");

    // Relative -1 on a range that starts at 5 is 4 on the timeline, which is
    // fine. The last event at 8 then keeps the end at 8.
    let rendered = end_registry(Some(-1.0))
        .render(&one_clip_project(vec![range("set-end", 5.0, 20.0)]))
        .unwrap();
    assert_eq!(rendered.duration, 8.0);
}
