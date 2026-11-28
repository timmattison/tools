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

// Tests of the in-place form.

/// Registers an in-place spy under "spot". The spy records the region view
/// that it gets, and answers its events with one more event, "mark", at
/// `at(view)`.
fn in_place_spy(at: fn(RegionView) -> f64) -> (FilterRegistry, TestArc<Mutex<Vec<RegionView>>>) {
    let seen = TestArc::new(Mutex::new(Vec::new()));
    let captured = TestArc::clone(&seen);
    let mut registry = FilterRegistry::new();
    registry.register_in_place(kind("spot"), move |events, view, _| {
        captured.lock().unwrap().push(view);
        let mut answer = events.to_vec();
        answer.push(Event {
            time: at(view),
            event_type: EventType::Output,
            data: "mark".to_string(),
        });
        Ok(answer)
    });
    (registry, seen)
}

fn with_data(events: &[Event]) -> Vec<(f64, &str)> {
    events.iter().map(|e| (e.time, e.data.as_str())).collect()
}

#[test]
fn an_in_place_filter_keeps_the_later_events_where_they_are() {
    let (registry, _seen) = in_place_spy(|view| view.length);

    let out = registry
        .run(
            events(&[0.5, 1.0, 1.5, 2.5, 3.5]),
            &[range("spot", 1.0, 2.0)],
        )
        .unwrap();

    // The mark is at the end of the region, before the events after it, and
    // nothing moves.
    assert_eq!(
        with_data(&out),
        vec![
            (0.5, "e0.5"),
            (1.0, "e1"),
            (1.5, "e1.5"),
            (2.0, "mark"),
            (2.5, "e2.5"),
            (3.5, "e3.5"),
        ]
    );
}

#[test]
fn an_in_place_mark_at_the_region_end_comes_before_an_event_at_that_time() {
    let (registry, _seen) = in_place_spy(|view| view.length);

    let out = registry
        .run(events(&[0.0, 1.0, 2.0]), &[range("spot", 1.0, 2.0)])
        .unwrap();

    assert_eq!(
        with_data(&out),
        vec![(0.0, "e0"), (1.0, "e1"), (2.0, "mark"), (2.0, "e2")]
    );
}

#[test]
fn an_in_place_mark_at_the_region_end_lands_on_the_end_when_the_sum_rounds_up() {
    // The length is 5.79 - 1.73, and that length plus 1.73 is
    // 5.790000000000001 in f64, one step after the end of the region.
    let (start, end) = std::hint::black_box((1.73, 5.79));
    assert!((end - start) + start > end);
    let (registry, _seen) = in_place_spy(|view| view.length);

    let out = registry
        .run(events(&[0.0, start, end]), &[range("spot", start, end)])
        .unwrap();

    assert_eq!(
        with_data(&out),
        vec![
            (0.0, "e0"),
            (1.73, "e1.73"),
            (5.79, "mark"),
            (5.79, "e5.79")
        ]
    );
}

#[test]
fn an_in_place_filter_gets_the_length_and_the_openness_of_its_region() {
    let cases = [
        // The region All reaches the end of the timeline at 4.
        (all("spot"), 4.0, true),
        // The event at 4 comes after the range.
        (range("spot", 1.0, 2.0), 1.0, false),
        (range("spot", 1.0, 4.0), 3.0, false),
        // The range holds the last event and reaches past the end.
        (range("spot", 3.0, 10.0), 1.0, true),
        // The range starts after the end of the timeline.
        (range("spot", 5.0, 6.0), 0.0, true),
    ];
    for (filter, length, open) in cases {
        let (registry, seen) = in_place_spy(|_| 0.0);

        registry
            .run(events(&[0.0, 1.0, 4.0]), std::slice::from_ref(&filter))
            .unwrap();

        assert_eq!(
            *seen.lock().unwrap(),
            vec![RegionView { length, open }],
            "{:?}",
            filter.region
        );
    }
}

#[test]
fn render_gives_an_in_place_filter_the_end_of_the_project_and_keeps_the_duration() {
    // The project ends at 10, after its last event at 8.
    let (registry, seen) = in_place_spy(|view| view.length);

    let rendered = registry
        .render(&one_clip_project(vec![range("spot", 2.0, 9.0)]))
        .unwrap();

    // The silence from 9 to 10 comes after the range, so the range is not
    // open, and the mark is at its end.
    assert_eq!(
        *seen.lock().unwrap(),
        vec![RegionView {
            length: 7.0,
            open: false
        }]
    );
    assert_eq!(times(&rendered.events), vec![0.0, 8.0, 9.0]);
    assert_eq!(rendered.duration, 10.0);

    let (registry, seen) = in_place_spy(|view| view.length);
    let rendered = registry
        .render(&one_clip_project(vec![all("spot")]))
        .unwrap();
    assert_eq!(
        *seen.lock().unwrap(),
        vec![RegionView {
            length: 10.0,
            open: true
        }]
    );
    assert_eq!(times(&rendered.events), vec![0.0, 8.0, 10.0]);
    assert_eq!(rendered.duration, 10.0);
}

#[test]
fn an_in_place_filter_that_fails_fails_the_run_with_its_kind_and_message() {
    let mut registry = FilterRegistry::new();
    registry.register_in_place(kind("grumpy"), |_, _, _| Err("bad options".to_string()));

    let result = registry.run(events(&[1.0]), &[all("grumpy")]);

    match result {
        Err(NleError::RenderError(message)) => {
            assert!(message.contains("grumpy"), "{message}");
            assert!(message.contains("bad options"), "{message}");
        }
        other => panic!("expected a RenderError, got {other:?}"),
    }
}

#[test]
fn an_in_place_filter_cannot_answer_a_time_outside_its_region() {
    let answers: [fn(RegionView) -> f64; 3] = [|view| view.length + 0.5, |_| -0.5, |_| f64::NAN];
    for at in answers {
        let (registry, _seen) = in_place_spy(at);

        let result = registry.run(events(&[0.0, 1.0, 1.5, 4.0]), &[range("spot", 1.0, 2.0)]);

        match result {
            Err(NleError::RenderError(message)) => {
                assert!(message.contains("\"spot\" answered the time"), "{message}");
            }
            other => panic!("expected a RenderError, got {other:?}"),
        }
    }
}

// Tests of the span filter: the length and the shift.

/// A span filter that multiplies each time and the length by `factor`.
fn scale_span(
    factor: f64,
) -> impl Fn(Span, SpanContext<'_>, &FilterOptions) -> Result<Span, String> + Send + Sync {
    move |span, _, _| {
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
        })
    }
}

/// Registers a span filter under "spy" that records the length that it gets
/// and answers with the span that it got.
fn span_spy_registry() -> (FilterRegistry, TestArc<Mutex<Vec<f64>>>) {
    let seen = TestArc::new(Mutex::new(Vec::new()));
    let captured = TestArc::clone(&seen);
    let mut registry = FilterRegistry::new();
    registry.register_span(kind("spy"), move |span, _, _| {
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
    registry.register_span(kind("speed"), |_, _, _| Err("bad speed".to_string()));

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
        registry.register_span(kind("len"), move |span, _, _| {
            Ok(Span {
                events: span.events,
                length,
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
    registry.register_span(kind("len"), |span, _, _| {
        Ok(Span {
            events: span.events,
            length: 3.0,
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

// Tests of the screen form.

/// What a screen spy saw: the region view, the size, and the events before the
/// region.
type Seen = (RegionView, TerminalSize, Vec<Event>);

fn size(width: u16, height: u16) -> TerminalSize {
    TerminalSize { width, height }
}

/// Registers a screen filter under `name`. It records what it sees, and it
/// answers its events with one more event, "mark", at the length of its
/// region, and with the size `answer`.
fn register_screen_spy(
    registry: &mut FilterRegistry,
    name: &str,
    answer: TerminalSize,
    seen: &TestArc<Mutex<Vec<Seen>>>,
) {
    let captured = TestArc::clone(seen);
    registry.register_screen(kind(name), move |events, view, _| {
        captured
            .lock()
            .unwrap()
            .push((view.region, view.size, view.before.to_vec()));
        let mut out = events.to_vec();
        out.push(Event {
            time: view.region.length,
            event_type: EventType::Output,
            data: "mark".to_string(),
        });
        Ok(Screen {
            events: out,
            size: answer,
        })
    });
}

fn screen_spy(answer: TerminalSize) -> (FilterRegistry, TestArc<Mutex<Vec<Seen>>>) {
    let seen = TestArc::new(Mutex::new(Vec::new()));
    let mut registry = FilterRegistry::new();
    register_screen_spy(&mut registry, "crop", answer, &seen);
    (registry, seen)
}

fn assert_error_has(result: Result<impl fmt::Debug, NleError>, parts: &[&str]) {
    match result {
        Err(NleError::RenderError(message)) => {
            for part in parts {
                assert!(message.contains(part), "{message}");
            }
        }
        other => panic!("expected a RenderError, got {other:?}"),
    }
}

#[test]
fn a_screen_filter_on_all_sets_the_size_of_the_rendered_recording() {
    let (registry, _seen) = screen_spy(size(40, 10));

    let rendered = registry
        .render(&one_clip_project(vec![all("crop")]))
        .unwrap();

    assert_eq!((rendered.width, rendered.height), (40, 10));
}

#[test]
fn the_first_screen_filter_sees_the_header_size_of_the_first_clip() {
    let (registry, seen) = screen_spy(size(40, 10));

    registry
        .render(&one_clip_project(vec![all("crop")]))
        .unwrap();

    assert_eq!(seen.lock().unwrap()[0].1, size(100, 30));
}

#[test]
fn a_second_screen_filter_sees_the_size_that_the_first_answered() {
    let seen = TestArc::new(Mutex::new(Vec::new()));
    let mut registry = FilterRegistry::new();
    register_screen_spy(&mut registry, "first", size(40, 10), &seen);
    register_screen_spy(&mut registry, "second", size(20, 5), &seen);

    let rendered = registry
        .render(&one_clip_project(vec![all("first"), all("second")]))
        .unwrap();

    let sizes: Vec<TerminalSize> = seen.lock().unwrap().iter().map(|s| s.1).collect();
    assert_eq!(sizes, vec![size(100, 30), size(40, 10)]);
    assert_eq!((rendered.width, rendered.height), (20, 5));
}

#[test]
fn a_screen_filter_on_a_range_sees_the_events_before_and_the_region_view() {
    let (registry, seen) = screen_spy(size(100, 30));

    registry
        .render(&one_clip_project(vec![range("crop", 2.0, 9.0)]))
        .unwrap();

    let seen = seen.lock().unwrap();
    assert_eq!(
        seen[0].0,
        RegionView {
            length: 7.0,
            open: false
        }
    );
    assert_eq!(times(&seen[0].2), vec![0.0]);
}

#[test]
fn a_screen_filter_on_a_range_cannot_change_the_size() {
    let (registry, _seen) = screen_spy(size(40, 10));

    let result = registry.render(&one_clip_project(vec![range("crop", 2.0, 9.0)]));

    assert_error_has(
        result,
        &[
            "\"crop\"",
            "40x10",
            "only a filter on the region All",
            "100x30",
        ],
    );
}

#[test]
fn a_screen_filter_that_answers_no_cells_fails() {
    for answer in [size(0, 10), size(40, 0)] {
        let (registry, _seen) = screen_spy(answer);

        let result = registry.render(&one_clip_project(vec![all("crop")]));

        assert_error_has(result, &["\"crop\"", "which has no cells"]);
    }
}

#[test]
fn a_screen_filter_that_fails_fails_the_render_with_its_kind_and_message() {
    let mut registry = FilterRegistry::new();
    registry.register_screen(kind("grumpy"), |_, _, _| Err("bad options".to_string()));

    let result = registry.render(&one_clip_project(vec![all("grumpy")]));

    assert_error_has(result, &["filter \"grumpy\" failed: bad options"]);
}

#[test]
fn a_screen_filter_cannot_answer_a_time_outside_its_region() {
    let answers: [f64; 3] = [7.5, -0.5, f64::NAN];
    for at in answers {
        let mut registry = FilterRegistry::new();
        registry.register_screen(kind("crop"), move |_, _, _| {
            Ok(Screen {
                events: vec![ev(at)],
                size: size(100, 30),
            })
        });

        let result = registry.render(&one_clip_project(vec![range("crop", 2.0, 9.0)]));

        assert_error_has(result, &["\"crop\" answered the time"]);
    }
}

#[test]
fn a_screen_filter_moves_nothing() {
    let (registry, _seen) = screen_spy(size(40, 10));

    let rendered = registry
        .render(&one_clip_project(vec![all("crop")]))
        .unwrap();

    assert_eq!(times(&rendered.events), vec![0.0, 8.0, 10.0]);
    assert_eq!(rendered.duration, 10.0);
}

#[test]
fn run_with_a_screen_filter_fails_and_names_the_kind() {
    let (registry, _seen) = screen_spy(size(40, 10));

    let result = registry.run(events(&[1.0]), &[all("crop")]);

    assert_error_has(
        result,
        &[
            "\"crop\"",
            "needs the size of the screen",
            "FilterRegistry::render",
        ],
    );
}

#[test]
fn a_project_with_only_the_old_forms_renders_the_header_size() {
    let mut registry = FilterRegistry::new();
    registry.register(kind("double"), double());
    registry.register_in_place(kind("spot"), |events, _, _| Ok(events.to_vec()));

    let rendered = registry
        .render(&one_clip_project(vec![all("double"), all("spot")]))
        .unwrap();

    assert_eq!((rendered.width, rendered.height), (100, 30));
}

#[test]
fn a_screen_view_says_whether_the_region_is_all() {
    let seen = TestArc::new(Mutex::new(Vec::new()));
    let captured = TestArc::clone(&seen);
    let mut registry = FilterRegistry::new();
    registry.register_screen(kind("spy"), move |events, view, _| {
        captured.lock().unwrap().push(view.whole_timeline);
        Ok(Screen {
            events: events.to_vec(),
            size: view.size,
        })
    });

    // The source has events at 0 and 8, and it lasts 10 seconds. The second
    // range starts at 0 and holds each event, thus it is open.
    registry
        .render(&one_clip_project(vec![all("spy")]))
        .unwrap();
    registry
        .render(&one_clip_project(vec![range("spy", 0.0, 50.0)]))
        .unwrap();

    assert_eq!(*seen.lock().unwrap(), vec![true, false]);
}

#[test]
fn register_screen_and_each_other_form_replace_each_other_under_one_kind() {
    // Each row registers one filter of another form under the kind "f".
    type Register = fn(&mut FilterRegistry);
    let forms: [(&str, Register); 3] = [
        ("event", |registry| {
            registry.register(kind("f"), add_one());
        }),
        ("in-place", |registry| {
            registry.register_in_place(kind("f"), |events, _, _| Ok(events.to_vec()));
        }),
        ("span", |registry| {
            registry.register_span(kind("f"), scale_span(2.0));
        }),
    ];

    for (name, register_other) in forms {
        // The screen filter comes first. The other form replaces it, thus the
        // screen filter does not run and the header size stays.
        let seen = TestArc::new(Mutex::new(Vec::new()));
        let mut registry = FilterRegistry::new();
        register_screen_spy(&mut registry, "f", size(40, 10), &seen);
        register_other(&mut registry);
        let rendered = registry.render(&one_clip_project(vec![all("f")])).unwrap();
        assert!(
            seen.lock().unwrap().is_empty(),
            "the {name} form did not replace the screen filter"
        );
        assert_eq!(
            (rendered.width, rendered.height),
            (100, 30),
            "the {name} form did not replace the screen filter"
        );

        // The screen filter comes last. It replaces the other form, thus it
        // runs once and its size is in the header.
        let seen = TestArc::new(Mutex::new(Vec::new()));
        let mut registry = FilterRegistry::new();
        register_other(&mut registry);
        register_screen_spy(&mut registry, "f", size(40, 10), &seen);
        let rendered = registry.render(&one_clip_project(vec![all("f")])).unwrap();
        assert_eq!(
            seen.lock().unwrap().len(),
            1,
            "the screen filter did not replace the {name} form"
        );
        assert_eq!(
            (rendered.width, rendered.height),
            (40, 10),
            "the screen filter did not replace the {name} form"
        );
    }
}

// Tests of the context of a span filter.

/// The contexts that a spy got: the events before its region and the size.
type SeenContexts = TestArc<Mutex<Vec<(Vec<Event>, TerminalSize, bool)>>>;

/// Registers a span filter under "look" that records each context that it
/// gets, as the events before its region, the size, and whether a screen
/// filter rewrote part of the region, and answers with the span that it got.
fn context_spy_registry() -> (FilterRegistry, SeenContexts) {
    let seen = TestArc::new(Mutex::new(Vec::new()));
    let captured = TestArc::clone(&seen);
    let mut registry = FilterRegistry::new();
    registry.register_span(kind("look"), move |span, context, _| {
        captured.lock().unwrap().push((
            context.before.to_vec(),
            context.size,
            context.screen_rewritten,
        ));
        Ok(span)
    });
    (registry, seen)
}

#[test]
fn a_span_filter_on_a_range_gets_the_events_before_the_range_at_their_times() {
    let (registry, seen) = context_spy_registry();

    registry
        .run(events(&[0.5, 1.5, 2.5, 5.0]), &[range("look", 2.0, 3.0)])
        .unwrap();

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, events(&[0.5, 1.5]));
}

#[test]
fn a_span_filter_on_a_range_gets_the_events_after_the_range_at_their_times() {
    let seen = TestArc::new(Mutex::new(Vec::new()));
    let captured = TestArc::clone(&seen);
    let mut registry = FilterRegistry::new();
    registry.register_span(kind("look"), move |span, context, _| {
        captured.lock().unwrap().push(context.after.to_vec());
        Ok(span)
    });

    // The event at the end of the region is after the region.
    registry
        .run(events(&[0.5, 1.5, 2.0, 5.0]), &[range("look", 1.0, 2.0)])
        .unwrap();

    assert_eq!(*seen.lock().unwrap(), vec![events(&[2.0, 5.0])]);
}

#[test]
fn run_gives_a_span_filter_the_size_of_a_vt100() {
    let (registry, seen) = context_spy_registry();

    registry.run(events(&[1.0]), &[all("look")]).unwrap();

    assert_eq!(seen.lock().unwrap()[0].1, TerminalSize::VT100);
}

#[test]
fn render_gives_a_span_filter_the_size_of_the_clip_that_starts_first() {
    let (registry, seen) = context_spy_registry();
    let mut project = two_clip_project();
    // The clip "early" starts first. A size of "late" must not reach the filter.
    let mut late = (*project.clips[0].source).clone();
    late.width = 132;
    late.height = 43;
    project.clips[0].source = Arc::new(late);
    let mut early = (*project.clips[1].source).clone();
    early.width = 120;
    early.height = 40;
    project.clips[1].source = Arc::new(early);
    project.filters.push(all("look"));

    registry.render(&project).unwrap();

    assert_eq!(
        seen.lock().unwrap()[0].1,
        TerminalSize {
            width: 120,
            height: 40
        }
    );
}

#[test]
fn a_span_filter_after_a_screen_filter_gets_the_size_that_the_screen_filter_gave() {
    let (mut registry, seen) = context_spy_registry();
    let screens = TestArc::new(Mutex::new(Vec::new()));
    register_screen_spy(&mut registry, "crop", size(40, 10), &screens);

    registry
        .render(&one_clip_project(vec![all("crop"), all("look")]))
        .unwrap();

    assert_eq!(seen.lock().unwrap()[0].1, size(40, 10));
}

#[test]
fn a_span_filter_after_a_screen_filter_that_rewrote_its_region_is_told_so() {
    let (mut registry, seen) = context_spy_registry();
    let screens = TestArc::new(Mutex::new(Vec::new()));
    register_screen_spy(&mut registry, "crop", size(80, 24), &screens);

    registry
        .render(&one_clip_project(vec![all("crop"), all("look")]))
        .unwrap();

    assert!(seen.lock().unwrap()[0].2);
}

#[test]
fn a_span_filter_after_a_screen_filter_that_changed_nothing_is_not_told_so() {
    let (mut registry, seen) = context_spy_registry();
    registry.register_screen(kind("same"), |events, view, _| {
        Ok(Screen {
            events: events.to_vec(),
            size: view.size,
        })
    });

    registry
        .render(&one_clip_project(vec![all("same"), all("look")]))
        .unwrap();

    assert!(!seen.lock().unwrap()[0].2);
}

#[test]
fn a_span_filter_with_no_screen_filter_before_it_is_not_told_so() {
    let (registry, seen) = context_spy_registry();

    registry
        .render(&one_clip_project(vec![all("look")]))
        .unwrap();

    assert!(!seen.lock().unwrap()[0].2);
}
