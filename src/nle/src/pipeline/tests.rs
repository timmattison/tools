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

// Tests of the span filter: the length and the shift.

/// A span filter that multiplies each time and the length by `factor`.
fn scale_span(factor: f64) -> impl Fn(Span, &FilterOptions) -> Result<Span, String> + Send + Sync {
    move |span, _| {
        Ok(Span {
            events: span
                .events
                .into_iter()
                .map(|e| Event {
                    time: e.time * factor,
                    ..e
                })
                .collect(),
            length: span.length * factor,
            marks: span.marks,
        })
    }
}

/// Registers a span filter under "spy" that records the length that it gets
/// and answers with the span that it got.
fn span_spy_registry() -> (FilterRegistry, TestArc<Mutex<Vec<f64>>>) {
    let seen = TestArc::new(Mutex::new(Vec::new()));
    let captured = TestArc::clone(&seen);
    let mut registry = FilterRegistry::new();
    registry.register_span(kind("spy"), move |span, _| {
        captured.lock().unwrap().push(span.length);
        Ok(span)
    });
    (registry, seen)
}

#[test]
fn a_span_filter_on_a_range_sees_the_length_of_the_range() {
    let (registry, seen) = span_spy_registry();

    registry
        .run(events(&[0.5, 1.5, 5.0]), &[range("spy", 1.0, 3.0)])
        .unwrap();

    assert_eq!(*seen.lock().unwrap(), vec![2.0]);
}

#[test]
fn a_span_filter_on_all_sees_the_last_event_time_as_the_length_in_run() {
    let (registry, seen) = span_spy_registry();

    registry.run(events(&[1.0, 4.0]), &[all("spy")]).unwrap();

    assert_eq!(*seen.lock().unwrap(), vec![4.0]);
}

#[test]
fn a_span_filter_on_all_sees_the_duration_of_the_project_in_render() {
    let (registry, seen) = span_spy_registry();

    registry
        .render(&one_clip_project(vec![all("spy")]))
        .unwrap();

    // The events end at 8 s, and the project lasts 10 s.
    assert_eq!(*seen.lock().unwrap(), vec![10.0]);
}

#[test]
fn a_span_filter_length_stops_at_the_end_of_the_timeline() {
    let (registry, seen) = span_spy_registry();

    registry
        .run(events(&[1.0, 4.0]), &[range("spy", 2.0, 100.0)])
        .unwrap();

    assert_eq!(*seen.lock().unwrap(), vec![2.0]);
}

#[test]
fn a_span_filter_length_is_zero_for_a_range_after_the_end_of_the_timeline() {
    let (registry, seen) = span_spy_registry();

    registry
        .run(events(&[1.0, 4.0]), &[range("spy", 10.0, 20.0)])
        .unwrap();

    assert_eq!(*seen.lock().unwrap(), vec![0.0]);
}

#[test]
fn a_span_filter_moves_the_later_events_by_the_change_of_the_length() {
    let mut registry = FilterRegistry::new();
    registry.register_span(kind("halve"), scale_span(0.5));

    let out = registry
        .run(events(&[0.5, 1.5, 5.0]), &[range("halve", 1.0, 3.0)])
        .unwrap();

    // The length 2 becomes 1, so the event at 5 moves by -1.
    assert_eq!(times(&out), vec![0.5, 1.25, 4.0]);
}

#[test]
fn a_span_filter_on_a_region_with_no_event_still_moves_the_later_events() {
    let mut registry = FilterRegistry::new();
    registry.register_span(kind("halve"), scale_span(0.5));

    let out = registry
        .run(events(&[0.5, 5.0]), &[range("halve", 1.0, 3.0)])
        .unwrap();

    assert_eq!(times(&out), vec![0.5, 4.0]);
}

#[test]
fn render_moves_the_duration_by_the_change_of_the_length_of_a_span_filter() {
    let mut registry = FilterRegistry::new();
    registry.register_span(kind("halve"), scale_span(0.5));

    let whole = registry
        .render(&one_clip_project(vec![all("halve")]))
        .unwrap();
    // The project lasts 10 s, and the length 10 becomes 5.
    assert_eq!(whole.duration, 5.0);

    let part = registry
        .render(&one_clip_project(vec![range("halve", 1.0, 3.0)]))
        .unwrap();
    assert_eq!(part.duration, 9.0);
}

// Tests of the span filter: errors and registration.

#[test]
fn a_span_filter_that_answers_an_error_fails_the_run_with_the_kind_and_the_message() {
    let mut registry = FilterRegistry::new();
    registry.register_span(kind("speed"), |_, _| Err("bad speed".to_string()));

    let result = registry.run(events(&[1.0]), &[all("speed")]);

    match result {
        Err(NleError::RenderError(message)) => {
            assert!(message.contains("speed"), "{message}");
            assert!(message.contains("bad speed"), "{message}");
        }
        other => panic!("expected a RenderError, got {other:?}"),
    }
}

#[test]
fn a_span_filter_that_answers_a_bad_length_fails_and_names_the_kind() {
    // The events end at 3 s, so a length below 3 is below the largest time.
    let bad_lengths = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, 2.5];
    for length in bad_lengths {
        let mut registry = FilterRegistry::new();
        registry.register_span(kind("len"), move |span, _| {
            Ok(Span {
                events: span.events,
                length,
                marks: span.marks,
            })
        });

        let result = registry.run(events(&[1.0, 3.0]), &[all("len")]);

        match result {
            Err(NleError::RenderError(message)) => {
                assert!(message.contains("len"), "length {length}: {message}");
            }
            other => panic!("length {length}: expected a RenderError, got {other:?}"),
        }
    }
}

#[test]
fn a_span_filter_may_answer_a_length_equal_to_its_largest_time() {
    let mut registry = FilterRegistry::new();
    registry.register_span(kind("len"), |span, _| {
        Ok(Span {
            events: span.events,
            length: 3.0,
            marks: span.marks,
        })
    });

    let out = registry.run(events(&[1.0, 3.0]), &[all("len")]).unwrap();

    assert_eq!(times(&out), vec![1.0, 3.0]);
}

#[test]
fn register_span_and_register_replace_each_other_under_one_kind() {
    let mut registry = FilterRegistry::new();
    assert!(!registry.contains(&kind("f")));

    registry.register_span(kind("f"), scale_span(0.5));
    assert!(registry.contains(&kind("f")));
    registry.register(kind("f"), add_one());
    let out = registry.run(events(&[1.0, 2.0]), &[all("f")]).unwrap();
    assert_eq!(times(&out), vec![2.0, 3.0]);

    registry.register_span(kind("f"), scale_span(2.0));
    let out = registry.run(events(&[1.0, 2.0]), &[all("f")]).unwrap();
    assert_eq!(times(&out), vec![2.0, 4.0]);
}

// Tests of the marks.

/// A span filter that keeps its span and adds a mark at `at`, relative to
/// the start of its region.
fn marker(
    at: f64,
    label: &'static str,
) -> impl Fn(Span, &FilterOptions) -> Result<Span, String> + Send + Sync {
    move |mut span, _| {
        span.marks.push(Mark {
            time: at,
            label: label.to_string(),
        });
        Ok(span)
    }
}

fn mark_times(marks: &[Mark]) -> Vec<f64> {
    marks.iter().map(|mark| mark.time).collect()
}

/// The marks of the render of `one_clip_project(filters)` through `registry`.
fn rendered_marks(registry: &FilterRegistry, filters: Vec<AppliedFilter>) -> Vec<Mark> {
    registry
        .render_with_marks(&one_clip_project(filters))
        .unwrap()
        .marks
}

#[test]
fn a_mark_of_a_span_filter_goes_onto_the_timeline_from_the_start_of_its_region() {
    let mut registry = FilterRegistry::new();
    registry.register_span(kind("mark"), marker(1.5, "here"));

    let marks = rendered_marks(&registry, vec![range("mark", 2.0, 6.0)]);

    assert_eq!(
        marks,
        vec![Mark {
            time: 3.5,
            label: "here".to_string()
        }]
    );
}

#[test]
fn a_later_span_filter_gets_the_marks_inside_its_region_relative_to_its_start() {
    let seen = TestArc::new(Mutex::new(Vec::new()));
    let captured = TestArc::clone(&seen);
    let mut registry = FilterRegistry::new();
    registry.register_span(kind("mark"), marker(1.5, "inside"));
    registry.register_span(kind("early"), marker(0.5, "before"));
    registry.register_span(kind("spy"), move |span, _| {
        captured.lock().unwrap().push(mark_times(&span.marks));
        Ok(span)
    });

    // The marks go to 3.5 and 0.5. The spy starts at 3, so it sees 0.5 only.
    rendered_marks(
        &registry,
        vec![
            all("early"),
            range("mark", 2.0, 6.0),
            range("spy", 3.0, 8.0),
        ],
    );

    assert_eq!(*seen.lock().unwrap(), vec![vec![0.5]]);
}

#[test]
fn a_later_span_filter_moves_the_marks_that_it_answers() {
    let mut registry = FilterRegistry::new();
    registry.register_span(kind("mark"), marker(4.0, "m"));
    registry.register_span(kind("halve"), |span: Span, _: &FilterOptions| {
        Ok(Span {
            marks: span
                .marks
                .into_iter()
                .map(|mark| Mark {
                    time: mark.time / 2.0,
                    ..mark
                })
                .collect(),
            ..span
        })
    });

    let marks = rendered_marks(&registry, vec![all("mark"), range("halve", 2.0, 6.0)]);

    // The mark is 2 s into the region, and the filter halves that.
    assert_eq!(mark_times(&marks), vec![3.0]);
}

#[test]
fn a_mark_after_a_later_region_moves_by_its_shift_and_a_mark_before_it_stays() {
    let mut registry = FilterRegistry::new();
    registry.register_span(kind("early"), marker(1.0, "early"));
    registry.register_span(kind("late"), marker(8.0, "late"));
    registry.register_span(kind("halve"), scale_span(0.5));

    let marks = rendered_marks(
        &registry,
        vec![all("early"), all("late"), range("halve", 2.0, 6.0)],
    );

    // The length 4 becomes 2, so the mark at 8 moves by -2.
    assert_eq!(mark_times(&marks), vec![1.0, 6.0]);
}

#[test]
fn a_mark_inside_the_region_of_an_event_filter_keeps_its_time() {
    let mut registry = FilterRegistry::new();
    registry.register_span(kind("mark"), marker(4.0, "m"));
    registry.register(kind("double"), double());

    let marks = rendered_marks(&registry, vec![all("mark"), range("double", 2.0, 9.0)]);

    assert_eq!(mark_times(&marks), vec![4.0]);
}

#[test]
fn a_mark_after_the_region_of_an_event_filter_moves_by_its_shift() {
    let mut registry = FilterRegistry::new();
    registry.register_span(kind("mark"), marker(9.0, "m"));
    registry.register(kind("add"), add_one());

    // The event at 0 moves to 1, so the shift is 1.
    let marks = rendered_marks(&registry, vec![all("mark"), range("add", 0.0, 5.0)]);

    assert_eq!(mark_times(&marks), vec![10.0]);
}

#[test]
fn a_mark_never_comes_after_the_end_of_the_timeline() {
    let mut registry = FilterRegistry::new();
    registry.register_span(kind("mark"), marker(9.0, "m"));
    registry.register(kind("halve"), mapper(|t| t / 2.0));

    // The halve moves the event at 8 to 4, so the end 10 moves to 6, and the
    // mark inside the region keeps its time 9.
    let rendered = registry
        .render_with_marks(&one_clip_project(vec![all("mark"), all("halve")]))
        .unwrap();

    assert_eq!(rendered.recording.duration, 6.0);
    assert_eq!(mark_times(&rendered.marks), vec![6.0]);
}

#[test]
fn the_marks_of_a_render_are_in_time_order() {
    let mut registry = FilterRegistry::new();
    registry.register_span(kind("late"), marker(5.0, "late"));
    registry.register_span(kind("early"), marker(2.0, "early"));
    registry.register_span(kind("tie"), marker(5.0, "tie"));

    let marks = rendered_marks(&registry, vec![all("late"), all("early"), all("tie")]);

    let labels: Vec<&str> = marks.iter().map(|mark| mark.label.as_str()).collect();
    assert_eq!(labels, vec!["early", "late", "tie"]);
}

#[test]
fn a_span_filter_that_answers_a_bad_mark_fails_and_names_the_kind() {
    // The project lasts 10 s, so a mark above 10 is after the answered length.
    for time in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, 10.5] {
        let mut registry = FilterRegistry::new();
        registry.register_span(kind("badmark"), move |mut span, _| {
            span.marks.push(Mark {
                time,
                label: String::new(),
            });
            Ok(span)
        });

        let result = registry.render_with_marks(&one_clip_project(vec![all("badmark")]));

        match result {
            Err(NleError::RenderError(message)) => {
                assert!(message.contains("badmark"), "mark {time}: {message}");
            }
            other => panic!("mark {time}: expected a RenderError, got {other:?}"),
        }
    }
}

#[test]
fn a_span_filter_may_answer_a_mark_at_its_length() {
    let mut registry = FilterRegistry::new();
    registry.register_span(kind("mark"), marker(10.0, "end"));

    let marks = rendered_marks(&registry, vec![all("mark")]);

    assert_eq!(mark_times(&marks), vec![10.0]);
}
