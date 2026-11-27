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
        chapters: Vec::new(),
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

#[test]
fn render_of_a_whole_recording_with_chapters_and_no_filter_equals_its_source() {
    let mut recording = source("chaptered", &[0.0, 1.0, 2.0], 3.0);
    recording.chapters = vec![
        Chapter {
            time: 0.5,
            label: "日本語テスト".to_string(),
        },
        Chapter {
            time: 2.0,
            label: "café 🎉".to_string(),
        },
    ];
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
        chapters: Vec::new(),
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
        chapters: Vec::new(),
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
        chapters: Vec::new(),
    };

    let result = FilterRegistry::new().render(&project);

    assert!(
        matches!(result, Err(NleError::RenderError(_))),
        "{result:?}"
    );
}

// Tests of the chapters in the render.

use crate::types::{Chapter, ChapterId, ChapterMarker};

fn marker(id: u64, time: f64, label: &str) -> ChapterMarker {
    ChapterMarker {
        id: ChapterId(id),
        time: Seconds::new(time).unwrap(),
        label: label.to_string(),
    }
}

/// One clip with events at `event_times`, a duration of `duration`, the
/// `markers`, and the `filters`.
fn chapter_project(
    event_times: &[f64],
    duration: f64,
    markers: Vec<ChapterMarker>,
    filters: Vec<AppliedFilter>,
) -> EditProject {
    EditProject {
        clips: vec![clip_of(1, source("chap", event_times, duration), 0.0)],
        timeline: vec![track_of(1, 1)],
        filters,
        chapters: markers,
    }
}

fn chapter_times(recording: &Recording) -> Vec<f64> {
    recording.chapters.iter().map(|c| c.time).collect()
}

fn assert_close(actual: &[f64], expected: &[f64]) {
    assert_eq!(actual.len(), expected.len(), "{actual:?} vs {expected:?}");
    for (a, e) in actual.iter().zip(expected) {
        assert!((a - e).abs() < 1e-9, "{actual:?} vs {expected:?}");
    }
}

fn doubling_registry() -> FilterRegistry {
    let mut registry = FilterRegistry::new();
    registry.register(kind("double"), mapper(|t| t * 2.0));
    registry
}

#[test]
fn with_no_filter_the_chapters_come_out_in_time_order_with_their_labels() {
    let project = chapter_project(
        &[0.0, 1.0],
        5.0,
        vec![
            marker(1, 4.0, "café 🎉"),
            marker(2, 1.0, "first"),
            marker(3, 2.5, "日本語テスト"),
        ],
        Vec::new(),
    );

    let rendered = FilterRegistry::new().render(&project).unwrap();

    assert_eq!(
        rendered.chapters,
        vec![
            Chapter {
                time: 1.0,
                label: "first".to_string()
            },
            Chapter {
                time: 2.5,
                label: "日本語テスト".to_string()
            },
            Chapter {
                time: 4.0,
                label: "café 🎉".to_string()
            },
        ]
    );
}

#[test]
fn with_no_marker_the_recording_has_no_chapter() {
    let project = chapter_project(&[0.0, 1.0], 5.0, Vec::new(), vec![all("double")]);

    let rendered = doubling_registry().render(&project).unwrap();

    assert!(rendered.chapters.is_empty());
}

#[test]
fn a_marker_after_the_end_is_dropped_and_a_marker_at_the_end_is_kept() {
    let project = chapter_project(
        &[0.0, 1.0],
        5.0,
        vec![marker(1, 5.0, "at end"), marker(2, 5.5, "after end")],
        Vec::new(),
    );

    let rendered = FilterRegistry::new().render(&project).unwrap();

    assert_eq!(chapter_times(&rendered), vec![5.0]);
    assert_eq!(
        rendered.chapters.first().map(|c| c.label.as_str()),
        Some("at end")
    );
}

#[test]
fn a_marker_whose_time_overflows_is_dropped_and_the_render_works() {
    // The range 0..0.5 holds the event 0. It goes to 1e308, so the shift is
    // 1e308. The event 1 and the end go to about 1e308, which is finite. The
    // marker at f64::MAX is after the range and moves to infinity, which is
    // after the end.
    let mut registry = FilterRegistry::new();
    registry.register(kind("far"), |_, _| vec![ev(1e308)]);
    let project = chapter_project(
        &[0.0, 1.0],
        1.0,
        vec![marker(1, 0.0, "start"), marker(2, f64::MAX, "far")],
        vec![range("far", 0.0, 0.5)],
    );

    let rendered = registry.render(&project).unwrap();

    let labels: Vec<&str> = rendered.chapters.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(labels, ["start"]);
}

#[test]
fn a_marker_on_an_event_goes_to_the_new_time_of_that_event() {
    // The events 1, 2, 4 go to 2, 4, 8.
    let project = chapter_project(
        &[1.0, 2.0, 4.0],
        8.0,
        vec![marker(1, 2.0, "on event")],
        vec![all("double")],
    );

    let rendered = doubling_registry().render(&project).unwrap();

    assert_eq!(chapter_times(&rendered), vec![4.0]);
}

#[test]
fn a_marker_between_two_events_keeps_its_fraction_of_the_gap() {
    // 3 is halfway between the events 2 and 4, thus it is halfway between 4 and 8.
    let project = chapter_project(
        &[1.0, 2.0, 4.0],
        8.0,
        vec![marker(1, 3.0, "between")],
        vec![all("double")],
    );

    let rendered = doubling_registry().render(&project).unwrap();

    assert_eq!(chapter_times(&rendered), vec![6.0]);
}

#[test]
fn a_marker_before_the_first_event_scales_from_the_region_start() {
    let project = chapter_project(
        &[1.0, 2.0, 4.0],
        8.0,
        vec![marker(1, 0.5, "before first")],
        vec![all("double")],
    );

    let rendered = doubling_registry().render(&project).unwrap();

    assert_eq!(chapter_times(&rendered), vec![1.0]);
}

#[test]
fn a_marker_after_the_last_event_keeps_its_gap_to_that_event() {
    // The last event goes from 4 to 8. The marker is 2 s after it.
    let project = chapter_project(
        &[1.0, 2.0, 4.0],
        8.0,
        vec![marker(1, 6.0, "after last")],
        vec![all("double")],
    );

    let rendered = doubling_registry().render(&project).unwrap();

    assert_eq!(chapter_times(&rendered), vec![10.0]);
}

#[test]
fn a_range_filter_moves_the_markers_after_it_and_not_those_before_it() {
    // The range 2..5 holds the events 2 and 4. They go to 2 and 6, so the shift is 2.
    let project = chapter_project(
        &[1.0, 2.0, 4.0, 6.0],
        8.0,
        vec![
            marker(1, 1.0, "before"),
            marker(2, 5.0, "at range end"),
            marker(3, 7.0, "after"),
        ],
        vec![range("double", 2.0, 5.0)],
    );

    let rendered = doubling_registry().render(&project).unwrap();

    assert_eq!(chapter_times(&rendered), vec![1.0, 7.0, 9.0]);
}

fn dropping_registry() -> FilterRegistry {
    let mut registry = FilterRegistry::new();
    registry.register(kind("drop-last"), |events, _| {
        events
            .split_last()
            .map(|(_, rest)| rest.to_vec())
            .unwrap_or_default()
    });
    registry
}

#[test]
fn when_a_filter_drops_an_event_a_marker_inside_the_span_moves_in_proportion() {
    // The events 2, 4, 6 become 2, 4. The span 6 becomes 4, so 3 goes to 2 and 6 goes to 4.
    let project = chapter_project(
        &[2.0, 4.0, 6.0],
        10.0,
        vec![marker(1, 3.0, "inside"), marker(2, 6.0, "at last")],
        vec![all("drop-last")],
    );

    let rendered = dropping_registry().render(&project).unwrap();

    assert_close(&chapter_times(&rendered), &[2.0, 4.0]);
}

#[test]
fn when_a_filter_drops_an_event_a_marker_after_the_span_moves_by_the_shift() {
    // The shift is 4 - 6 = -2, and the end goes from 10 to 8.
    let project = chapter_project(
        &[2.0, 4.0, 6.0],
        10.0,
        vec![marker(1, 8.0, "after")],
        vec![all("drop-last")],
    );

    let rendered = dropping_registry().render(&project).unwrap();

    assert_close(&chapter_times(&rendered), &[6.0]);
    assert_close(&[rendered.duration], &[8.0]);
}

#[test]
fn a_marker_moves_through_two_filters_in_a_row() {
    // The first filter takes the events 1, 2, 4 to 2, 4, 8 and the marker 3 to 6.
    // The second adds 1 to each time: 3, 5, 9. The marker 6 is halfway
    // between 4 and 8, thus it goes to halfway between 5 and 9, which is 7.
    let mut registry = doubling_registry();
    registry.register(kind("plus-one"), mapper(|t| t + 1.0));
    let project = chapter_project(
        &[1.0, 2.0, 4.0],
        8.0,
        vec![marker(1, 3.0, "both")],
        vec![all("double"), all("plus-one")],
    );

    let rendered = registry.render(&project).unwrap();

    assert_eq!(chapter_times(&rendered), vec![7.0]);
}

#[test]
fn two_markers_at_one_time_keep_the_order_of_their_ids() {
    let project = chapter_project(
        &[0.0, 1.0],
        5.0,
        vec![marker(9, 2.0, "nine"), marker(3, 2.0, "three")],
        Vec::new(),
    );

    let rendered = FilterRegistry::new().render(&project).unwrap();

    let labels: Vec<&str> = rendered.chapters.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(labels, vec!["three", "nine"]);
}

#[test]
fn when_a_filter_returns_no_event_a_marker_up_to_the_last_seen_event_goes_to_the_region_start() {
    // The range 2..7 holds the events 3 and 5, at 1 and 3 from its start. The
    // filter returns no event, thus the shift is 0 - 3 = -3. The markers 4 and
    // 5 go to the region start. The marker 6 is after the last seen event and
    // moves by the shift, as do the marker 7 at the range end and the marker 9.
    let mut registry = FilterRegistry::new();
    registry.register(kind("drop-all"), |_, _| Vec::new());
    let project = chapter_project(
        &[1.0, 3.0, 5.0, 8.0],
        10.0,
        vec![
            marker(1, 1.5, "before"),
            marker(2, 4.0, "inside"),
            marker(3, 5.0, "at last seen"),
            marker(4, 6.0, "after last seen"),
            marker(5, 7.0, "at range end"),
            marker(6, 9.0, "after"),
        ],
        vec![range("drop-all", 2.0, 7.0)],
    );

    let rendered = registry.render(&project).unwrap();

    assert_close(&chapter_times(&rendered), &[1.5, 2.0, 2.0, 3.0, 4.0, 6.0]);
    assert_close(&[rendered.duration], &[7.0]);
}

#[test]
fn in_a_range_that_holds_no_event_a_marker_after_the_region_start_moves_by_the_shift() {
    // The range 2..6 holds no event. The filter adds one event 1 s after the
    // region start, thus the shift is 1 - 0 = 1. A marker at the region start
    // stays. The marker 4 is after it and moves by the shift, as does the
    // marker 7 after the range.
    let mut registry = FilterRegistry::new();
    registry.register(kind("insert"), |_, _| vec![ev(1.0)]);
    let project = chapter_project(
        &[1.0, 7.0],
        10.0,
        vec![
            marker(1, 1.5, "before"),
            marker(2, 2.0, "at range start"),
            marker(3, 4.0, "inside"),
            marker(4, 7.0, "after"),
        ],
        vec![range("insert", 2.0, 6.0)],
    );

    let rendered = registry.render(&project).unwrap();

    assert_close(&chapter_times(&rendered), &[1.5, 2.0, 5.0, 8.0]);
    assert_close(&[rendered.duration], &[11.0]);
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

// Tests of the chapters through the in-place form and the span form.

#[test]
fn an_in_place_filter_that_adds_events_moves_no_chapter() {
    // The spy adds a mark at the end of its region, as the cursor filter does
    // when it sets the state of the recording back.
    let (registry, _seen) = in_place_spy(|view| view.length);
    let project = chapter_project(
        &[1.0, 2.0, 5.0, 8.0, 12.0],
        15.0,
        vec![marker(1, 4.0, "inside"), marker(2, 9.0, "late")],
        vec![range("spot", 0.0, 10.0)],
    );

    let rendered = registry.render(&project).unwrap();

    assert_close(&chapter_times(&rendered), &[4.0, 9.0]);
}

#[test]
fn a_span_filter_scales_a_chapter_in_a_region_with_no_event() {
    // A silent build step from 10 s to 20 s, at twice the speed.
    let mut registry = FilterRegistry::new();
    registry.register_span(kind("halve"), scale_span(0.5));
    let project = chapter_project(
        &[1.0, 25.0],
        30.0,
        vec![
            marker(1, 12.0, "build"),
            marker(2, 15.0, "link"),
            marker(3, 22.0, "deploy"),
        ],
        vec![range("halve", 10.0, 20.0)],
    );

    let rendered = registry.render(&project).unwrap();

    assert_close(&chapter_times(&rendered), &[11.0, 12.5, 17.0]);
}

#[test]
fn a_span_filter_scales_a_chapter_after_the_last_event_of_its_region() {
    let mut registry = FilterRegistry::new();
    registry.register_span(kind("halve"), scale_span(0.5));
    let project = chapter_project(
        &[1.0, 11.0, 25.0],
        30.0,
        vec![marker(1, 10.5, "start"), marker(2, 16.0, "middle")],
        vec![range("halve", 10.0, 20.0)],
    );

    let rendered = registry.render(&project).unwrap();

    assert_close(&chapter_times(&rendered), &[10.25, 13.0]);
}
